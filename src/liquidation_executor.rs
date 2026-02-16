use std::sync::Arc;

use tokio::sync::{
    broadcast::Receiver,
    watch,
    Mutex,
};

use crate::common::Liquidator;

pub struct LiqExecutor {
    liquidators: Vec<Arc<dyn Liquidator>>,
    locks: Vec<Arc<Mutex<()>>>,
    receiver: Receiver<u64>,
    shutdown: watch::Receiver<bool>,
    interval: u64,
}

impl LiqExecutor {
    pub fn new(
        liquidators: Vec<Arc<dyn Liquidator>>,
        receiver: Receiver<u64>,
        shutdown: watch::Receiver<bool>,
        interval: u64,
    ) -> Self {
        let locks = liquidators
            .iter()
            .map(|_| Arc::new(Mutex::new(())))
            .collect();

        Self {
            liquidators,
            locks,
            receiver,
            shutdown,
            interval,
        }
    }

    pub async fn start(mut self) -> anyhow::Result<()> {
        tracing::info!(
            "📡 Liquidation executor started (every {} blocks)",
            self.interval
        );

        let mut last_run_block = 0u64;

        loop {
            tokio::select! {
                // 🔴 Shutdown signal
                _ = self.shutdown.changed() => {
                    tracing::info!("🛑 Liquidation executor shutting down");
                    break;
                }

                // 🧱 New block
                recv = self.receiver.recv() => {
                    let block_number = match recv {
                        Ok(b) => b,
                        Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                            tracing::warn!("⚠️  Block receiver lagged ({} messages dropped)", n);
                            continue;
                        }
                        Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                            tracing::warn!("📴 Block channel closed");
                            break;
                        }
                    };

                    // Deterministic debouncing
                    if block_number <= last_run_block {
                        continue;
                    }

                    if block_number < last_run_block + self.interval {
                        tracing::trace!(
                            "⏭️  Skipping block {} (last run {})",
                            block_number,
                            last_run_block
                        );
                        continue;
                    }

                    last_run_block = block_number;

                    tracing::info!(
                        "🚀 Liquidation cycle triggered at block {}",
                        block_number
                    );

                    for (liq, lock) in self
                        .liquidators
                        .iter()
                        .cloned()
                        .zip(self.locks.iter().cloned())
                    {
                        tokio::spawn(async move {
                            let guard = match lock.try_lock() {
                                Ok(g) => g,
                                Err(_) => {
                                    tracing::debug!(
                                        "⏳ Liquidator already running, skipping this cycle"
                                    );
                                    return;
                                }
                            };

                            if let Err(e) = liq.run().await {
                                tracing::error!("❌ Liquidator failed: {:?}", e);
                            }

                            drop(guard);
                        });
                    }
                }
            }
        }

        tracing::info!("✅ Liquidation executor stopped cleanly");
        Ok(())
    }
}
