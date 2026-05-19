use crate::common::AdminCmd;
use tokio::sync::broadcast::error::RecvError;
use tokio::sync::{broadcast, mpsc, watch};

pub struct WatchListPruner {
    aave_cmd: mpsc::Sender<AdminCmd>,
    morpho_cmd: mpsc::Sender<AdminCmd>,
    comet_cmd: mpsc::Sender<AdminCmd>,
    block_rx: broadcast::Receiver<u64>,
    shutdown: watch::Receiver<bool>,
    interval: u64,
}

impl WatchListPruner {
    pub fn new(
        aave_cmd: mpsc::Sender<AdminCmd>,
        morpho_cmd: mpsc::Sender<AdminCmd>,
        comet_cmd: mpsc::Sender<AdminCmd>,
        block_rx: broadcast::Receiver<u64>,
        shutdown: watch::Receiver<bool>,
        interval: u64,
    ) -> Self {
        Self {
            aave_cmd,
            morpho_cmd,
            comet_cmd,
            block_rx,
            shutdown,
            interval,
        }
    }

    pub async fn start(&mut self) -> anyhow::Result<()> {
        tracing::info!(
            "📡 WatchListPruner started (every {} blocks)",
            self.interval
        );
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
                                let comet = self.comet_cmd.clone();

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
                                    if let Err(e) = comet.send(AdminCmd::Prune).await {
                                        tracing::error!("Failed to send prune to Comet: {:?}", e);
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

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::time::{timeout, Duration};

    fn cmd_channels() -> (
        mpsc::Sender<AdminCmd>,
        mpsc::Receiver<AdminCmd>,
        mpsc::Sender<AdminCmd>,
        mpsc::Receiver<AdminCmd>,
        mpsc::Sender<AdminCmd>,
        mpsc::Receiver<AdminCmd>,
    ) {
        let (aave_tx, aave_rx) = mpsc::channel(4);
        let (morpho_tx, morpho_rx) = mpsc::channel(4);
        let (comet_tx, comet_rx) = mpsc::channel(4);
        (aave_tx, aave_rx, morpho_tx, morpho_rx, comet_tx, comet_rx)
    }

    #[tokio::test]
    async fn sends_prune_on_interval_block() {
        let (aave_tx, mut aave_rx, morpho_tx, mut morpho_rx, comet_tx, mut comet_rx) =
            cmd_channels();
        let (block_tx, block_rx) = broadcast::channel(4);
        let (shutdown_tx, shutdown_rx) = watch::channel(false);
        let mut pruner =
            WatchListPruner::new(aave_tx, morpho_tx, comet_tx, block_rx, shutdown_rx, 10);
        let handle = tokio::spawn(async move { pruner.start().await });

        block_tx.send(10).expect("send block");

        assert!(matches!(
            timeout(Duration::from_secs(2), aave_rx.recv())
                .await
                .expect("aave prune"),
            Some(AdminCmd::Prune)
        ));
        assert!(matches!(
            timeout(Duration::from_secs(2), morpho_rx.recv())
                .await
                .expect("morpho prune"),
            Some(AdminCmd::Prune)
        ));
        assert!(matches!(
            timeout(Duration::from_secs(2), comet_rx.recv())
                .await
                .expect("comet prune"),
            Some(AdminCmd::Prune)
        ));

        shutdown_tx.send(true).expect("shutdown");
        handle.await.expect("join").expect("pruner");
    }

    #[tokio::test]
    async fn does_not_send_on_non_interval_block() {
        let (aave_tx, mut aave_rx, morpho_tx, _morpho_rx, comet_tx, _comet_rx) = cmd_channels();
        let (block_tx, block_rx) = broadcast::channel(4);
        let (shutdown_tx, shutdown_rx) = watch::channel(false);
        let mut pruner =
            WatchListPruner::new(aave_tx, morpho_tx, comet_tx, block_rx, shutdown_rx, 10);
        let handle = tokio::spawn(async move { pruner.start().await });

        block_tx.send(9).expect("send block");
        assert!(timeout(Duration::from_millis(100), aave_rx.recv())
            .await
            .is_err());

        shutdown_tx.send(true).expect("shutdown");
        handle.await.expect("join").expect("pruner");
    }

    #[tokio::test]
    async fn returns_error_when_block_channel_closed() {
        let (aave_tx, _aave_rx, morpho_tx, _morpho_rx, comet_tx, _comet_rx) = cmd_channels();
        let (block_tx, block_rx) = broadcast::channel(4);
        let (_shutdown_tx, shutdown_rx) = watch::channel(false);
        let mut pruner =
            WatchListPruner::new(aave_tx, morpho_tx, comet_tx, block_rx, shutdown_rx, 10);

        drop(block_tx);

        let result = timeout(Duration::from_secs(2), pruner.start())
            .await
            .expect("pruner returns");
        assert!(result.is_err());
    }
}
