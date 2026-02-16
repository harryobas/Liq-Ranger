use std::sync::Arc;
use tokio::sync::mpsc;
use super::{
    abi_bindings::IMorphoBlue,
    morpho_watchlist::MorphoWatchList,
    morpho_config::MorphoConfig,
    helpers
};

use ethers::{
    providers::Middleware,
    types::{Address, H256},
};

use futures_util::StreamExt;
use tokio::sync::watch;

use crate::common::{AdminCmd, WatchList};

pub struct WatchListUpdater<M: Middleware + 'static> {
    watch_list: Arc<MorphoWatchList>,
    morpho: Arc<IMorphoBlue<M>>,
    config: Arc<MorphoConfig>,
    shutdown: watch::Receiver<bool>,
    cmd_rx: mpsc::Receiver<AdminCmd>
}

impl<M: Middleware + 'static> WatchListUpdater<M> {
    pub fn new(
        list: Arc<MorphoWatchList>,
        morpho: Arc<IMorphoBlue<M>>,
        config: Arc<MorphoConfig>,
        shutdown: watch::Receiver<bool>,
        cmd_rx: mpsc::Receiver<AdminCmd>
    ) -> Self {
        Self {
            watch_list: list,
            morpho,
            config,
            shutdown,
            cmd_rx
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
                    let market_id = H256::from(f.id);

                    if self.config.morpho_markets.contains(&market_id) {
                        self.add_borrow(f.on_behalf, market_id).await?;
                    }
                }
            }

            // 💰 Repay
            evt = repay_stream.next() => {
                if let Some(Ok(f)) = evt {
                    let market_id = H256::from(f.id);

                    if self.config.morpho_markets.contains(&market_id) {
                        self.remove_if_cleared(f.on_behalf, market_id).await?;
                    }
                }
            }

            // 🔥 Liquidation
            evt = liquidate_stream.next() => {
                if let Some(Ok(f)) = evt {
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


    async fn add_borrow(
        &self,
        borrower: Address,
        market: H256,
    ) -> anyhow::Result<()> {
        tracing::info!(
            "👀 Tracking borrow: borrower={:?}, market={:?}",
            borrower,
            market
        );

        self.watch_list.add((borrower, market)).await?;
        Ok(())
    }

    async fn remove_if_cleared(
        &self,
        borrower: Address,
        market: H256,
    ) -> anyhow::Result<()> {
        // Always verify on-chain debt
        let has_debt = helpers::has_outstanding_debt(self.morpho.clone(), borrower, market).await?;

        if !has_debt {
            tracing::info!(
                "🧹 Removing cleared position: borrower={:?}, market={:?}",
                borrower,
                market
            );

            self.watch_list.remove((borrower, market)).await?;
        }

        Ok(())
    }
    async fn prune_watchlist(&self) -> anyhow::Result<()> {
    tracing::info!("🔍 Pruning Morpho watchlist...");

    let entries = self.watch_list.snapshot();

    for (borrower, market) in entries {
        let has_debt = helpers::has_outstanding_debt(
            self.morpho.clone(),
            borrower,
            market
        ).await?;

        if !has_debt {
            tracing::info!(
                "🧹 Prune removing cleared position: borrower={:?}, market={:?}",
                borrower,
                market
            );

            self.watch_list.remove((borrower, market)).await?;
        }
    }

    tracing::info!("✅ Morpho watchlist prune complete");
    Ok(())
  }
}
