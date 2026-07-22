use ethers::{
    providers::Middleware,
    types::{Address, H256},
};
use std::sync::Arc;
use tokio::sync::{mpsc, watch};

use futures_util::{stream, StreamExt};

use crate::common::{abi_bindings::IMorphoBlue, has_outstanding_debt_morpho, AdminCmd};

use crate::{
    config::MorphoConfig,
    core::{ports::ProtocolWatchList, types::TrackerIdentity},
    watchlists::morpho_watchlist::MorphoWatchList,
};

use std::str::FromStr;

pub struct MorphoWatchListUpdater<M: Middleware + 'static> {
    watch_list: Arc<MorphoWatchList>,
    morpho: Arc<IMorphoBlue<M>>,
    config: Arc<MorphoConfig>,
    shutdown: watch::Receiver<bool>,
    cmd_rx: mpsc::Receiver<AdminCmd>,
}

impl<M: Middleware + Send + Sync + 'static> MorphoWatchListUpdater<M> {
    pub fn new(
        list: Arc<MorphoWatchList>,
        morpho: Arc<IMorphoBlue<M>>,
        config: Arc<MorphoConfig>,
        shutdown: watch::Receiver<bool>,
        cmd_rx: mpsc::Receiver<AdminCmd>,
    ) -> Self {
        Self {
            watch_list: list,
            morpho,
            config,
            shutdown,
            cmd_rx,
        }
    }

    pub async fn start(mut self) -> anyhow::Result<()> {
        tracing::info!("📡 MorphoWatchListUpdater started");
        let borrow_filter = self.morpho.borrow_filter();
        let repay_filter = self.morpho.repay_filter();
        let liquidate_filter = self.morpho.liquidate_filter();

        let mut borrow_stream = borrow_filter.stream().await?;
        let mut repay_stream = repay_filter.stream().await?;
        let mut liquidate_stream = liquidate_filter.stream().await?;

        loop {
            tokio::select! {
                // 🔴 Shutdown
                _ = self.shutdown.changed() => {
                    tracing::info!("🛑 MorphoWatchListUpdater shutting down");
                    break;
                }

                // 🛠 Admin Commands (Prune trigger)
                cmd = self.cmd_rx.recv() => {
                    match cmd {
                        Some(AdminCmd::Prune) => {
                            tracing::info!("🧹 Prune command received");
                            self.prune_watchlist().await?;
                        }
                        Some(AdminCmd::StatusCheck) => {}

                        None => {
                            tracing::warn!("⚠ Admin channel closed");
                            break;
                        }
                    }
                }

                // 📥 Borrow
                evt = borrow_stream.next() => {
                    if let Some(Ok(f)) = evt {
                        tracing::debug!("📥 Borrow event received: {:?}", f);
                        let market_id = H256::from(f.id);

                        if self.config.morpho_markets.contains(&market_id) {
                            self.add_borrow(f.on_behalf, market_id).await?;
                        }
                    }
                }

                // 💰 Repay
                evt = repay_stream.next() => {
                    if let Some(Ok(f)) = evt {
                        tracing::debug!("💰 Repay event received: {:?}", f);
                        let market_id = H256::from(f.id);

                        if self.config.morpho_markets.contains(&market_id) {
                            self.remove_if_cleared(f.on_behalf, market_id).await?;
                        }
                    }
                }

                // 🔥 Liquidation
                evt = liquidate_stream.next() => {
                    if let Some(Ok(f)) = evt {
                        tracing::info!("🔥 Liquidation event received: {:?}", f);
                        let market_id = H256::from(f.id);

                        if self.config.morpho_markets.contains(&market_id) {
                            self.remove_if_cleared(f.borrower, market_id).await?;
                        }
                    }
                }
            }
        }

        tracing::info!("✅ MorphoWatchListUpdater stopped cleanly");
        Ok(())
    }

    async fn add_borrow(&self, borrower: Address, market: H256) -> anyhow::Result<()> {
        let identity = TrackerIdentity::MorphoBlue {
            borrower,
            market_id: market,
        }
        .to_string_id();

        tracing::info!(
            "👀 Tracking borrow: borrower={:?}, market={:?}",
            borrower,
            market
        );

        self.watch_list.add(&identity).await?;
        Ok(())
    }

    async fn remove_if_cleared(&self, borrower: Address, market: H256) -> anyhow::Result<()> {
        let identity = TrackerIdentity::MorphoBlue {
            borrower,
            market_id: market,
        }
        .to_string_id();

        Self::check_and_remove_by_identity(&identity, &self.morpho, &self.watch_list).await
    }

    async fn prune_watchlist(&self) -> anyhow::Result<()> {
        tracing::info!("🔍 Pruning Morpho watchlist...");

        // Snapshot yields Vec<String> of tracked identities
        let snapshot: Vec<String> = self.watch_list.snapshot();

        let morpho = self.morpho.clone();
        let watch_list = self.watch_list.clone();

        stream::iter(snapshot)
            .for_each_concurrent(4, |identity| {
                let morpho = morpho.clone();
                let watch_list = watch_list.clone();

                async move {
                    if let Err(e) =
                        Self::check_and_remove_by_identity(&identity, &morpho, &watch_list).await
                    {
                        tracing::error!("Failed to prune Morpho identity {}: {:?}", identity, e);
                    }
                }
            })
            .await;

        tracing::info!("✅ Morpho watchlist prune complete");
        Ok(())
    }

    // Associated helper method isolates clones and allows direct call reuse
    async fn check_and_remove_by_identity(
        identity: &str,
        morpho: &Arc<IMorphoBlue<M>>,
        watch_list: &Arc<MorphoWatchList>,
    ) -> anyhow::Result<()> {
        let parsed = TrackerIdentity::from_str(identity)?;

        if let TrackerIdentity::MorphoBlue {
            borrower,
            market_id,
        } = parsed
        {
            match has_outstanding_debt_morpho(morpho.clone(), borrower, market_id).await {
                Ok(false) => {
                    watch_list.remove(identity).await?;
                    tracing::info!("🧹 Removed cleared position identity: {}", identity);
                }
                Ok(true) => {}
                Err(e) => {
                    tracing::warn!("Debt check failed for identity {}: {:?}", identity, e);
                }
            }
        }

        Ok(())
    }
}
