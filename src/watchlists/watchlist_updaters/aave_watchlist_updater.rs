use anyhow::Result;
use ethers::{providers::Middleware, types::Address};
use futures_util::{stream, StreamExt};
use std::{str::FromStr, sync::Arc};
use tokio::sync::{mpsc, watch};

use crate::{
    common::{
        abi_bindings::{IAaveV3Pool, IAaveV3PoolEvents},
        has_outstanding_debt, AdminCmd,
    },
    config::AaveConfig,
    core::{ports::ProtocolWatchList, types::TrackerIdentity},
    watchlists::aave_watchlist::AaveWatchList,
};

pub struct AaveWatchListUpdater<M: Middleware + 'static> {
    watch_list: Arc<AaveWatchList>,
    pool: Arc<IAaveV3Pool<M>>,
    config: Arc<AaveConfig>,
    shutdown: watch::Receiver<bool>,
    cmd_rx: mpsc::Receiver<AdminCmd>,
}

impl<M: Middleware + Send + Sync + 'static> AaveWatchListUpdater<M> {
    pub fn new(
        watch_list: Arc<AaveWatchList>,
        pool: Arc<IAaveV3Pool<M>>,
        config: Arc<AaveConfig>,
        shutdown: watch::Receiver<bool>,
        cmd_rx: mpsc::Receiver<AdminCmd>,
    ) -> Self {
        Self {
            watch_list,
            pool,
            config,
            shutdown,
            cmd_rx,
        }
    }

    pub async fn start(&mut self) -> Result<()> {
        tracing::info!("AaveWatchListUpdater started...");

        let events = self.pool.events();
        let mut event_stream = events.stream().await?;

        loop {
            tokio::select! {
                // 🔴 Shutdown
                _ = self.shutdown.changed() => {
                    tracing::info!("🛑 AaveWatchListUpdater shutting down");
                    break;
                }

                // 📥 Aave Events
                evt = event_stream.next() => {
                    match evt {
                        Some(Ok(event)) => {
                            self.handle_event(event).await?;
                        }
                        Some(Err(e)) => {
                            tracing::error!("Event stream error: {:?}", e);
                        }
                        None => {
                            tracing::warn!("Aave event stream ended");
                            break;
                        }
                    }
                }

                // 🧹 Admin Commands
                cmd = self.cmd_rx.recv() => {
                    match cmd {
                        Some(AdminCmd::Prune) => {
                            tracing::info!("🧹 Received prune command");
                            self.prune_watchlist().await?;
                        }
                        Some(AdminCmd::StatusCheck) => {}
                        None => {
                            tracing::warn!("Admin channel closed");
                            break;
                        }
                    }
                }
            }
        }

        tracing::info!("✅ AaveWatchListUpdater stopped cleanly");
        Ok(())
    }

    async fn handle_event(&self, event: IAaveV3PoolEvents) -> Result<()> {
        match event {
            IAaveV3PoolEvents::BorrowFilter(f) => {
                if !self.config.reserves.contains(&f.reserve) {
                    return Ok(());
                }
                let identity = TrackerIdentity::AaveV3 {
                    borrower: f.on_behalf_of,
                    reserve: f.reserve,
                }
                .to_string_id();

                self.watch_list.add(&identity).await?;

                tracing::debug!(
                    "Added borrower {:?} on reserve {:?}",
                    f.on_behalf_of,
                    f.reserve
                );
            }

            IAaveV3PoolEvents::RepayFilter(f) => {
                if !self.config.reserves.contains(&f.reserve) {
                    return Ok(());
                }

                self.remove_if_no_debt(f.user, f.reserve).await?;
            }
            IAaveV3PoolEvents::LiquidationCallFilter(f) => {
                if !self.config.reserves.contains(&f.debt_asset) {
                    return Ok(());
                }
                self.remove_if_no_debt(f.user, f.debt_asset).await?;
            }
            _ => {}
        }

        Ok(())
    }

    async fn prune_watchlist(&self) -> Result<()> {
        // Returns Vec<String> of tracked identities
        let snapshot = self.watch_list.snapshot();

        tracing::info!("🧹 Pruning {} tracked identities", snapshot.len());

        let pool = self.pool.clone();
        let config = self.config.clone();
        let watch_list = self.watch_list.clone();

        stream::iter(snapshot)
            .for_each_concurrent(4, |identity| {
                let pool = pool.clone();
                let config = config.clone();
                let watch_list = watch_list.clone();

                async move {
                    if let Err(e) =
                        Self::check_and_remove_by_identity(&identity, &pool, &config, &watch_list)
                            .await
                    {
                        tracing::error!("Failed to prune identity {}: {:?}", identity, e);
                    }
                }
            })
            .await;

        Ok(())
    }

    async fn remove_if_no_debt(&self, borrower: Address, reserve: Address) -> Result<()> {
        let identity = TrackerIdentity::AaveV3 { borrower, reserve }.to_string_id();
        Self::check_and_remove_by_identity(&identity, &self.pool, &self.config, &self.watch_list)
            .await
    }

    async fn check_and_remove_by_identity(
        identity: &str,
        pool: &Arc<IAaveV3Pool<M>>,
        config: &Arc<AaveConfig>,
        watch_list: &Arc<AaveWatchList>,
    ) -> Result<()> {
        let parsed = TrackerIdentity::from_str(identity)?;

        if let TrackerIdentity::AaveV3 { borrower, reserve } = parsed {
            let has_debt = has_outstanding_debt(borrower, reserve, pool, config).await?;

            if !has_debt {
                watch_list.remove(identity).await?;
                tracing::debug!("Removed identity {} from watchlist (no debt)", identity);
            }
        }

        Ok(())
    }
}
