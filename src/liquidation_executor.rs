use std::sync::Arc;
use std::time::Duration;
use ethers::providers::Middleware;
use tokio::sync::{broadcast::Receiver, watch, Semaphore};
use tracing::{debug, error, info, warn};

use crate::core::services::liquidation_engine::LiquidationEngine;

/// Maximum allowable execution time per block cycle before forced abort
const CYCLE_TIMEOUT_MS: u64 = 2000;
/// Strict Single-Flight: Only 1 active block cycle allowed at any given time
const MAX_CONCURRENT_PERMITS: usize = 1;

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
            // Single permit guarantees zero overlapping block executions
            concurrency_limit: Arc::new(Semaphore::new(MAX_CONCURRENT_PERMITS)),
        }
    }

    pub async fn start(&mut self) -> anyhow::Result<()> {
        info!(
            "🚀 Liquidation executor active — running single-flight pipeline (timeout: {}ms)",
            CYCLE_TIMEOUT_MS
        );

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

                        // Fallback query to RPC client on channel lag
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

                    // Prevent processing older or duplicate block numbers
                    if block_number <= last_processed_block {
                        continue;
                    }
                    last_processed_block = block_number;

                    // Non-blocking try_acquire: if the current cycle is busy, skip this block
                    let permit = match self.concurrency_limit.clone().try_acquire_owned() {
                        Ok(p) => p,
                        Err(_) => {
                            debug!(
                                block = block_number,
                                "⏳ Engine busy with previous block. Skipping block to maintain tip state."
                            );
                            continue;
                        }
                    };

                    let engine = self.engine.clone();

                    // Spawn single-flight task bounded strictly by the 2-second timeout
                    tokio::spawn(async move {
                        debug!(block = block_number, "⚡ Executing liquidation cycle");

                        let timeout_duration = Duration::from_millis(CYCLE_TIMEOUT_MS);
                        let cycle_future = engine.run_liquidation_cycle(block_number);

                        match tokio::time::timeout(timeout_duration, cycle_future).await {
                            Ok(Ok(_)) => {
                                debug!(block = block_number, "✅ Cycle completed within budget");
                            }
                            Ok(Err(e)) => {
                                error!(block = block_number, "❌ Engine execution failure: {:?}", e);
                            }
                            Err(_) => {
                                warn!(
                                    block = block_number,
                                    "⏱️ Cycle timed out after {}ms! Dropped stale task to unblock engine.",
                                    CYCLE_TIMEOUT_MS
                                );
                            }
                        }

                        // Explicit drop releases permit slot back to semaphore immediately
                        drop(permit);
                    });
                }
            }
        }

        info!("✅ Liquidation executor stopped cleanly");
        Ok(())
    }
}