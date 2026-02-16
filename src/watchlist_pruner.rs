use tokio::sync::{mpsc, broadcast, watch};
use tokio::sync::broadcast::error::RecvError;
use crate::common::AdminCmd;

pub struct WatchListPruner {
    aave_cmd: mpsc::Sender<AdminCmd>,
    morpho_cmd: mpsc::Sender<AdminCmd>,
    block_rx: broadcast::Receiver<u64>,
    shutdown: watch::Receiver<bool>,
    interval: u64,
}

impl WatchListPruner {
    pub fn new(
        aave_cmd: mpsc::Sender<AdminCmd>,
        morpho_cmd: mpsc::Sender<AdminCmd>,
        block_rx: broadcast::Receiver<u64>,
        shutdown: watch::Receiver<bool>,
        interval: u64,
    ) -> Self {
        Self {
            aave_cmd,
            morpho_cmd,
            block_rx,
            shutdown,
            interval,
        }
    }

    pub async fn start(&mut self) -> anyhow::Result<()> {
        loop {
            tokio::select! {
                _ = self.shutdown.changed() => {
                    tracing::info!("🛑 WatchListPruner shutting down");
                    break;
                }

                evt = self.block_rx.recv() => {
                    match evt {
                        Ok(block_number) => {
                            if block_number % self.interval == 0 {
                                let aave = self.aave_cmd.clone();
                                let morpho = self.morpho_cmd.clone();

                                tokio::spawn(async move {
                                    tracing::info!(
                                        "🔔 Central Pruner: Triggering maintenance at block {}",
                                        block_number
                                    );

                                    if let Err(e) = aave.send(AdminCmd::Prune).await {
                                        tracing::error!("Failed to send prune to Aave: {:?}", e);
                                    }

                                    if let Err(e) = morpho.send(AdminCmd::Prune).await {
                                        tracing::error!("Failed to send prune to Morpho: {:?}", e);
                                    }
                                });
                            }
                        }

                        Err(RecvError::Lagged(n)) => {
                            tracing::warn!("⚠️ WatchListPruner lagged by {} blocks", n);
                        }

                        Err(RecvError::Closed) => {
                            tracing::error!("❌ Block stream closed");
                            return Err(anyhow::anyhow!("Block stream closed"));
                        }
                    }
                }
            }
        }

        tracing::info!("✅ WatchListPruner stopped cleanly");
        Ok(())
    }
}
