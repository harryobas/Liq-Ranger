use std::sync::Arc;
use anyhow::Result;
use ethers::{providers::Middleware};
use futures_util::StreamExt;
use tokio::sync::{mpsc, watch};

use super::{
    abi_bindings::{IComet, ICometEvents},
    compound_watchlist::CompoundWatchList,
};

use crate::common::{WatchList, AdminCmd};

pub struct CompoundWatchListUpdater<M: Middleware + 'static> {
    watch_list: Arc<CompoundWatchList>,
    comet: Arc<IComet<M>>,
    shutdown: watch::Receiver<bool>,
    cmd_rx: mpsc::Receiver<AdminCmd>,
}

impl<M: Middleware + Send + Sync + 'static> CompoundWatchListUpdater<M> {

    pub fn new(
        watch_list: Arc<CompoundWatchList>,
        comet: Arc<IComet<M>>,
        shutdown: watch::Receiver<bool>,
        cmd_rx: mpsc::Receiver<AdminCmd>,
    ) -> Self {
        Self {
            watch_list,
            comet,
            shutdown,
            cmd_rx,
        }
    }

    pub fn start(self) -> tokio::task::JoinHandle<Result<()>> {
        tokio::spawn(async move {
            self.run().await
        })
    }

    async fn run(mut self) -> Result<()> {
        tracing::info!("📡 CompoundWatchListUpdater starting...");

        let events = self.comet.events();
        let mut event_stream = events.stream().await?;

        loop {
            tokio::select! {

                //  Shutdown
                _ = self.shutdown.changed() => {
                    tracing::info!("🛑 CompoundWatchListUpdater shutting down");
                    break;
                }

                // 📥 Comet Events
                evt = event_stream.next() => {
                    match evt {
                        Some(Ok(event)) => {
                            self.handle_event(event).await?;
                        }
                        Some(Err(e)) => {
                            tracing::error!("Event stream error: {:?}", e);
                        }
                        None => {
                            tracing::warn!("Comet event stream ended");
                            break;
                        }
                    }
                }

                // 🧹 Admin Commands (optional)
                cmd = self.cmd_rx.recv() => {
                    match cmd {
                        Some(AdminCmd::StatusCheck) => {
                            tracing::info!(" CompoundWatchList size: {}", 
                                self.watch_list.snapshot().len()
                            );
                        }
                        Some(AdminCmd::Prune) => {
                            // No pruning needed for Compound
                            tracing::info!("🧹 Prune ignored (not applicable for Compound)");
                        }
                        None => {
                            tracing::warn!("Admin channel closed");
                            break;
                        }
                    }
                }
            }
        }

        tracing::info!("✅ CompoundWatchListUpdater stopped cleanly");
        Ok(())
    }

    async fn handle_event(&self, event: ICometEvents) -> Result<()> {

        match event {

            // 🔥 AbsorbCollateral → increase reserve
            ICometEvents::AbsorbCollateralFilter(f) => {

                self.watch_list
                    .add((f.asset, f.collateral_absorbed))
                    .await?;

                tracing::debug!(
                    "Absorbed {:?} amount {:?}",
                    f.asset,
                    f.collateral_absorbed
                );
            }

            // 💰 BuyCollateral → decrease reserve
            ICometEvents::BuyCollateralFilter(f) => {

                self.watch_list
                    .remove((f.asset, f.collateral_amount))
                    .await?;

                tracing::debug!(
                    "Bought {:?} amount {:?}",
                    f.asset,
                    f.collateral_amount
                );
            }

            _ => {}
        }

        Ok(())
    }
}