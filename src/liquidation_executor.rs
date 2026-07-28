use ethers::providers::Middleware;
use std::sync::Arc;
use tokio::sync::{broadcast::Receiver, watch};
use tracing::{debug, error, info, warn};

use crate::core::services::liquidation_engine::LiquidationEngine;

/// Execution interval in blocks (e.g., run every 5 blocks)
const BLOCK_INTERVAL: u64 = 3;

pub struct LiqExecutor<M> {
    engine: LiquidationEngine,
    client: Arc<M>,
    receiver: Receiver<u64>,
    shutdown: watch::Receiver<bool>,
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
        }
    }

    pub async fn start(&mut self) -> anyhow::Result<()> {
        info!(
            block_interval = BLOCK_INTERVAL,
            "🚀 Liquidation executor active — 3-block interval execution mode (No Timeouts)"
        );

        let mut last_processed_block: u64 = 0;
        let mut tracker = tokio::task::JoinSet::new();

        loop {
            tokio::select! {
                // Graceful Shutdown: Drain and abort all background tasks
                _ = self.shutdown.changed() => {
                    info!("🛑 Liquidation executor shutting down — aborting background tasks");
                    tracker.shutdown().await;
                    break;
                }

                // Clean up finished task handles to prevent memory accumulation in JoinSet
                Some(res) = tracker.join_next() => {
                    if let Err(e) = res {
                        if !e.is_cancelled() {
                            error!(error = ?e, "❌ 5-block cycle task panicked");
                        }
                    }
                }

                // Block Stream Processing
                recv = self.receiver.recv() => {
                    let block_number = match recv {
                        Ok(b) => b,
                        Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                            warn!(
                                dropped_blocks = n,
                                "⚠️ Block receiver lagged, querying client tip..."
                            );
                            match self.client.get_block_number().await {
                                Ok(b) => b.as_u64(),
                                Err(e) => {
                                    error!(error = ?e, "❌ Fallback RPC block query failed");
                                    continue;
                                }
                            }
                        }
                        Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                            warn!("📴 Block channel closed");
                            tracker.shutdown().await;
                            break;
                        }
                    };

                    // Ignore older or already-handled blocks
                    if block_number <= last_processed_block {
                        continue;
                    }

                    // 1. Check if we hit the 5-block boundary
                    let blocks_elapsed = block_number.saturating_sub(last_processed_block);
                    let is_interval_block = block_number % BLOCK_INTERVAL == 0;

                    if !is_interval_block && blocks_elapsed < BLOCK_INTERVAL {
                        continue;
                    }

                    // 2. Overlap Guard: Skip cycle if previous 3-block cycle is still active
                    if tracker.len() > 0 {
                        warn!(
                            block = block_number,
                            in_flight_tasks = tracker.len(),
                            "⚠️ Previous 3-block cycle still running! Skipping interval to prevent task accumulation."
                        );
                        continue;
                    }

                    last_processed_block = block_number;
                    let engine = self.engine.clone();

                    // Spawn 5-block background discovery & execution cycle
                    tracker.spawn(async move {
                        info!(
                            block = block_number,
                            interval = BLOCK_INTERVAL,
                            "⚡ Triggering 3-block batched liquidation cycle"
                        );

                        // Direct execution without tokio::time::timeout wrapper
                        match engine.run_liquidation_cycle(block_number).await {
                            Ok(_) => {
                                debug!(block = block_number, "✅ 5-block cycle completed successfully");
                            }
                            Err(e) => {
                                error!(
                                    block = block_number,
                                    error = ?e,
                                    "❌ 3-block cycle execution failure"
                                );
                            }
                        }
                    });
                }
            }
        }

        info!("✅ Liquidation executor stopped cleanly");
        Ok(())
    }
}