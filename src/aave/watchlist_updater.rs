use std::sync::Arc;
use anyhow::Result;
use ethers::{providers::Middleware, types::Address};
use futures_util::{self, StreamExt, stream};
use tokio::sync::{mpsc, watch};

use super::{
    abi_bindings::{IAaveV3Pool, IAaveV3PoolEvents},
    aave_config::AaveConfig,
    aave_watchlist::AaveWatchList,
    helpers,
};

use crate::common::{WatchList, AdminCmd};

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

    /// Spawn the actor as a background task
    pub fn start(self) -> tokio::task::JoinHandle<Result<()>> {
        tokio::spawn(async move {
            self.run().await
        })
    }

    async fn run(mut self) -> Result<()> {
        tracing::info!("📡 AaveWatchListUpdater starting...");

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

                self.watch_list
                    .add((f.on_behalf_of, f.reserve))
                    .await?;

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
        
    let snapshot = self.watch_list.snapshot();

    tracing::info!("🧹 Pruning {} entries", snapshot.len());

    stream::iter(snapshot)
        .for_each_concurrent(4, |(borrower, reserve)| async move {
            if let Err(e) = self.remove_if_no_debt(borrower, reserve).await {
                tracing::error!(
                    "Failed to prune borrower {:?} reserve {:?}: {:?}",
                    borrower,
                    reserve,
                    e
                );
            }
        })
        .await;

    Ok(())
    }

    async fn remove_if_no_debt(
        &self,
        borrower: Address,
        reserve: Address,
    ) -> Result<()> {

        let has_debt = helpers::has_outstanding_debt(
            borrower,
            reserve,
            &self.pool,
            &self.config,
        ).await?;

        if !has_debt {
            self.watch_list.remove((borrower, reserve)).await?;
            tracing::debug!(
                "Removed {:?} from watchlist (no debt)",
                borrower
            );
        }

        Ok(())
    }
}
