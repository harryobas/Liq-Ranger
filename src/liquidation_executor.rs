use std::sync::Arc;

use tokio::sync::{broadcast::Receiver, watch, Mutex};

use crate::{common::Liquidator, constants};

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
            interval: constants::LIQ_EXECUTOR_INTERVAL,
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

                            if let Err(e) = liq.run(block_number).await {
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use tokio::sync::Notify;
    use tokio::time::{timeout, Duration};

    struct FakeLiquidator {
        calls: AtomicUsize,
        notify: Notify,
    }

    impl FakeLiquidator {
        fn new() -> Arc<Self> {
            Arc::new(Self {
                calls: AtomicUsize::new(0),
                notify: Notify::new(),
            })
        }

        async fn wait_for_calls(&self, expected: usize) {
            timeout(Duration::from_secs(2), async {
                loop {
                    if self.calls.load(Ordering::SeqCst) >= expected {
                        break;
                    }
                    self.notify.notified().await;
                }
            })
            .await
            .expect("expected liquidator calls");
        }
    }

    #[async_trait::async_trait]
    impl Liquidator for FakeLiquidator {
        async fn run(&self, _block_number: u64) -> anyhow::Result<()> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            self.notify.notify_waiters();
            Ok(())
        }
    }

    #[tokio::test]
    async fn runs_only_on_configured_interval() {
        let fake = FakeLiquidator::new();
        let (block_tx, block_rx) = tokio::sync::broadcast::channel(16);
        let (shutdown_tx, shutdown_rx) = watch::channel(false);
        let executor = LiqExecutor::new(vec![fake.clone()], block_rx, shutdown_rx);
        let handle = tokio::spawn(executor.start());

        block_tx.send(9).expect("send block");
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert_eq!(fake.calls.load(Ordering::SeqCst), 0);

        block_tx
            .send(constants::LIQ_EXECUTOR_INTERVAL)
            .expect("send block");
        fake.wait_for_calls(1).await;

        shutdown_tx.send(true).expect("shutdown");
        handle.await.expect("join").expect("executor");
    }

    #[tokio::test]
    async fn ignores_duplicate_and_old_blocks() {
        let fake = FakeLiquidator::new();
        let (block_tx, block_rx) = tokio::sync::broadcast::channel(16);
        let (shutdown_tx, shutdown_rx) = watch::channel(false);
        let executor = LiqExecutor::new(vec![fake.clone()], block_rx, shutdown_rx);
        let handle = tokio::spawn(executor.start());

        block_tx.send(10).expect("send block");
        fake.wait_for_calls(1).await;

        block_tx.send(10).expect("duplicate block");
        block_tx.send(9).expect("old block");
        block_tx.send(19).expect("before interval");
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert_eq!(fake.calls.load(Ordering::SeqCst), 1);

        block_tx.send(20).expect("next interval");
        fake.wait_for_calls(2).await;

        shutdown_tx.send(true).expect("shutdown");
        handle.await.expect("join").expect("executor");
    }

    #[tokio::test]
    async fn stops_on_shutdown() {
        let fake = FakeLiquidator::new();
        let (_block_tx, block_rx) = tokio::sync::broadcast::channel(16);
        let (shutdown_tx, shutdown_rx) = watch::channel(false);
        let executor = LiqExecutor::new(vec![fake], block_rx, shutdown_rx);

        let handle = tokio::spawn(executor.start());
        shutdown_tx.send(true).expect("shutdown");

        timeout(Duration::from_secs(2), handle)
            .await
            .expect("executor stopped")
            .expect("join")
            .expect("executor");
    }
}
