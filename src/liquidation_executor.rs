use std::sync::Arc;

use crate::constants;
use crate::core::services::liquidation_engine::LiquidationEngine;
use ethers::providers::Middleware;
use tokio::sync::{broadcast::Receiver, watch, Mutex};

pub struct LiqExecutor<M> {
    engine: LiquidationEngine,
    client: Arc<M>,
    lock: Mutex<()>,
    receiver: Receiver<u64>,
    shutdown: watch::Receiver<bool>,
    interval: u64,
}

impl<M: Middleware + 'static> LiqExecutor<M> {
    pub fn new(
        engine: LiquidationEngine,
        client: Arc<M>,
        receiver: Receiver<u64>,
        shutdown: watch::Receiver<bool>,
    ) -> Self {
        Self {
            engine,
            client,
            lock: Mutex::new(()),
            receiver,
            shutdown,
            interval: constants::LIQ_EXECUTOR_INTERVAL,
        }
    }

    pub async fn start(&mut self) -> anyhow::Result<()> {
        tracing::info!(
            " Liquidation executor started (running inline, every {} blocks)",
            self.interval
        );

        let mut last_run_block: Option<u64> = None;

        loop {
            tokio::select! {
                // Graceful Shutdown
                _ = self.shutdown.changed() => {
                    tracing::info!("🛑 Liquidation executor shutting down");
                    break;
                }

                //  New Block Intake
                recv = self.receiver.recv() => {
                    let block_number = match recv {
                        Ok(b) => Some(b),

                        Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                            tracing::warn!("⚠️  Block receiver lagged ({} messages dropped)", n);
                            match self.client.get_block_number().await{
                                Ok(b) => Some(b.as_u64()),
                                Err(e) => {
                                    tracing::error!("❌ Fallback synchronization query failed: {:?}", e);
                                    None
                                }
                            }
                        }

                        Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                            tracing::warn!("📴 Block channel closed");
                            break;
                        }
                    };

                    let block_number = match block_number {
                        Some(b) => b,
                        None => continue,
                    };


                    // Interval sequence boundaries validation
                    if let Some(last_block) = last_run_block {
                        if block_number <= last_block || block_number < last_block + self.interval {
                            continue;
                        }
                    }

                    last_run_block = Some(block_number);

                    tracing::info!(
                        "🚀 Triggering inline multi-protocol cycle at block {}",
                        block_number
                    );

                    // 💡 Try to acquire the execution guard inline.
                    // If a previous block's async requests are still hanging, skip immediately.
                    let guard = match self.lock.try_lock() {
                        Ok(g) => g,
                        Err(_) => {
                            tracing::warn!(
                                "⏳ Previous cycle at block {} is still processing! Skipping to maintain sync.",
                                block_number
                            );
                            continue;
                        }
                    };

                    // 💡 RUN INLINE: Awaiting directly here kills all 'static / thread-transfer lifetime constraints.
                    if let Err(e) = self.engine.run_liquidation_cycle(block_number).await {
                        tracing::error!("❌ Unified engine execution failure: {:?}", e);
                    }

                    drop(guard);
                }
            }
        }

        tracing::info!("✅ Liquidation executor stopped cleanly");
        Ok(())
    }
}
