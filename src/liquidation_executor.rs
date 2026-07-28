use ethers::providers::Middleware;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{broadcast::Receiver, watch};
use tracing::{debug, error, info, warn};

use crate::core::services::liquidation_engine::LiquidationEngine;

/// Generous safety timeout to clean up hung RPC sockets without dropping candidate discovery prematurely
const DISCOVERY_SAFETY_TIMEOUT_MS: u64 = 10000;

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
            safety_timeout_ms = DISCOVERY_SAFETY_TIMEOUT_MS,
            "🚀 Liquidation executor active — Persistent candidate scanning mode"
        );

        let mut last_processed_block: u64 = 0;
        // Use a JoinSet or spawn detached discovery tasks to avoid killing Phase 1 evaluations
        let mut tracker = tokio::task::JoinSet::new();

        loop {
            tokio::select! {
                // Graceful Shutdown: Drain and abort all background tasks
                _ = self.shutdown.changed() => {
                    info!("🛑 Liquidation executor shutting down — aborting background tasks");
                    tracker.shutdown().await;
                    break;
                }

                // Clean up finished task handles to prevent memory accumulation in the JoinSet
                Some(res) = tracker.join_next() => {
                    if let Err(e) = res {
                        if !e.is_cancelled() {
                            error!(error = ?e, "❌ Discovery task panicked");
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

                    // Ignore older or duplicate block triggers
                    if block_number <= last_processed_block {
                        continue;
                    }
                    last_processed_block = block_number;

                    let engine = self.engine.clone();

                    // Spawn background discovery cycle. It runs to completion independently
                    // and registers found candidates into the persistent Candidate Queue.
                    tracker.spawn(async move {
                        debug!(block = block_number, "⚡ Spawning candidate discovery cycle");

                        let timeout_duration = Duration::from_millis(DISCOVERY_SAFETY_TIMEOUT_MS);
                        let cycle_future = engine.run_liquidation_cycle(block_number);

                        match tokio::time::timeout(timeout_duration, cycle_future).await {
                            Ok(Ok(_)) => {
                                debug!(block = block_number, "✅ Discovery completed successfully");
                            }
                            Ok(Err(e)) => {
                                error!(
                                    block = block_number,
                                    error = ?e,
                                    "❌ Engine discovery failure"
                                );
                            }
                            Err(_) => {
                                warn!(
                                    block = block_number,
                                    timeout_ms = DISCOVERY_SAFETY_TIMEOUT_MS,
                                    "⏱️ Discovery timed out! Sockets auto-cleaned."
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