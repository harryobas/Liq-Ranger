use std::sync::Arc;
use ethers::providers::Middleware;
use tokio::sync::{broadcast::Receiver, watch, Semaphore};
use tracing::{debug, error, info, warn};

use crate::core::services::liquidation_engine::LiquidationEngine;

pub struct LiqExecutor<M> {
    engine: LiquidationEngine,
    client: Arc<M>,
    receiver: Receiver<u64>,
    shutdown: watch::Receiver<bool>,
    concurrency_limit: Arc<Semaphore>,
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
            receiver,
            shutdown,
            concurrency_limit: Arc::new(Semaphore::new(2)),
        }
    }

    pub async fn start(&mut self) -> anyhow::Result<()> {
        info!("🚀 Liquidation executor active — processing incoming block stream");

        let mut last_processed_block: u64 = 0;

        loop {
            tokio::select! {
                // Graceful Shutdown
                _ = self.shutdown.changed() => {
                    info!("🛑 Liquidation executor shutting down");
                    break;
                }

                // New Block Intake
                recv = self.receiver.recv() => {
                    let block_number = match recv {
                        Ok(b) => b,

                        // Fallback query to RPC client on channel lag to recover the true tip
                        Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                            warn!("⚠️ Block receiver lagged ({} blocks dropped), querying client tip...", n);
                            match self.client.get_block_number().await {
                                Ok(b) => b.as_u64(),
                                Err(e) => {
                                    error!("❌ Fallback RPC block query failed: {:?}", e);
                                    continue;
                                }
                            }
                        }

                        Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                            warn!("📴 Block channel closed");
                            break;
                        }
                    };

                    // Prevent duplicate block processing
                    if block_number <= last_processed_block {
                        continue;
                    }
                    last_processed_block = block_number;

                    // Try acquiring concurrency permit without blocking
                    let permit = match self.concurrency_limit.clone().try_acquire_owned() {
                        Ok(p) => p,
                        Err(_) => {
                            debug!(
                                block = block_number,
                                "⏳ Engine at max concurrency. Skipping block to maintain tip state."
                            );
                            continue;
                        }
                    };

                    let engine = self.engine.clone();

                    // Spawn non-blocking task so block intake loop remains real-time
                    tokio::spawn(async move {
                        debug!(block = block_number, "⚡ Executing liquidation cycle");
                        if let Err(e) = engine.run_liquidation_cycle(block_number).await {
                            error!(block = block_number, "❌ Unified engine execution failure: {:?}", e);
                        }
                        drop(permit);
                    });
                }
            }
        }

        info!("✅ Liquidation executor stopped cleanly");
        Ok(())
    }
}