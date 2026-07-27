use std::sync::Arc;
use std::time::Duration;
use ethers::providers::Middleware;
use tokio::task::JoinHandle;
use tokio::sync::{broadcast::Receiver, watch};
use tracing::{debug, error, info, warn};

use crate::core::services::liquidation_engine::LiquidationEngine;

/// Maximum allowable execution time per block cycle before forced timeout
const CYCLE_TIMEOUT_MS: u64 = 3000;

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
            timeout_ms = CYCLE_TIMEOUT_MS,
            "🚀 Liquidation executor active — strict tip-state cancellation mode"
        );

        let mut last_processed_block: u64 = 0;
        let mut active_task: Option<JoinHandle<()>> = None;

        loop {
            tokio::select! {
                // Graceful Shutdown: Cancel running task immediately and exit
                _ = self.shutdown.changed() => {
                    info!("🛑 Liquidation executor shutting down");
                    if let Some(task) = active_task.take() {
                        task.abort();
                    }
                    break;
                }

                // New Block Intake
                recv = self.receiver.recv() => {
                    let block_number = match recv {
                        Ok(b) => b,

                        // Fallback query to RPC client on channel lag
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
                            break;
                        }
                    };

                    // Ignore older or duplicate block triggers
                    if block_number <= last_processed_block {
                        continue;
                    }
                    last_processed_block = block_number;

                    // 1. INSTANT ABORT: If a cycle from block N-1 is still running, kill it!
                    if let Some(previous_task) = active_task.take() {
                        if !previous_task.is_finished() {
                            warn!(
                                block = block_number,
                                "⚡ New block tip arrived. Aborting stale cycle from previous block."
                            );
                            previous_task.abort();
                        }
                    }

                    let engine = self.engine.clone();

                    // 2. Spawn the new cycle for the freshest block tip
                    let handle = tokio::spawn(async move {
                        debug!(block = block_number, "⚡ Executing liquidation cycle");

                        let timeout_duration = Duration::from_millis(CYCLE_TIMEOUT_MS);
                        let cycle_future = engine.run_liquidation_cycle(block_number);

                        match tokio::time::timeout(timeout_duration, cycle_future).await {
                            Ok(Ok(_)) => {
                                debug!(block = block_number, "✅ Cycle completed within budget");
                            }
                            Ok(Err(e)) => {
                                error!(
                                    block = block_number,
                                    error = ?e,
                                    "❌ Engine execution failure"
                                );
                            }
                            Err(_) => {
                                warn!(
                                    block = block_number,
                                    timeout_ms = CYCLE_TIMEOUT_MS,
                                    "⏱️ Cycle timed out! Dropped stale task."
                                );
                            }
                        }
                    });

                    active_task = Some(handle);
                }
            }
        }

        info!("✅ Liquidation executor stopped cleanly");
        Ok(())
    }
}