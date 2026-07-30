use crate::core::ports::LendingProtocolReader;
use crate::core::types::LiqPayload;
use ethers::providers::Middleware;
use futures_util::future::join_all;
use std::sync::Arc;
use tokio::sync::{broadcast::Receiver, mpsc, watch};
use tracing::{debug, error, info, warn};

/// Execution interval in blocks (e.g., scan every 3 blocks)
const BLOCK_INTERVAL: u64 = 3;

pub struct ProtocolScanner<M> {
    readers: Vec<Arc<dyn LendingProtocolReader + Send + Sync>>,
    payload_tx: mpsc::Sender<LiqPayload>,
    client: Arc<M>,
    receiver: Receiver<u64>,
    shutdown: watch::Receiver<bool>,
}

impl<M: Middleware + 'static> ProtocolScanner<M> {
    pub fn new(
        readers: Vec<Arc<dyn LendingProtocolReader + Send + Sync>>,
        payload_tx: mpsc::Sender<LiqPayload>,
        client: Arc<M>,
        receiver: Receiver<u64>,
        shutdown: watch::Receiver<bool>,
    ) -> Self {
        Self {
            readers,
            payload_tx,
            client,
            receiver,
            shutdown,
        }
    }

    pub async fn start(&mut self) -> anyhow::Result<()> {
        info!(
            block_interval = BLOCK_INTERVAL,
            readers_count = self.readers.len(),
            "🚀 ProtocolScanner active — scanning protocols every 3 blocks"
        );

        let mut last_processed_block: u64 = 0;
        let mut tracker = tokio::task::JoinSet::new();

        loop {
            tokio::select! {
                // Graceful Shutdown: Drain and abort all background reader tasks
                _ = self.shutdown.changed() => {
                    info!("🛑 ProtocolScanner shutting down — aborting active reader scans");
                    tracker.shutdown().await;
                    break;
                }

                // Clean up finished scan task handles to prevent memory accumulation
                Some(res) = tracker.join_next() => {
                    if let Err(e) = res {
                        if !e.is_cancelled() {
                            error!(error = ?e, "❌ Protocol scanner task panicked");
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

                    // Ignore older or already-handled blocks, or blocks that haven't reached the interval threshold
                    if block_number <= last_processed_block || block_number - last_processed_block < BLOCK_INTERVAL {
                        continue;
                    }

                    // Overlap Guard: Skip cycle if previous scan is still querying RPCs
                    if tracker.len() > 0 {
                        warn!(
                            block = block_number,
                            in_flight_tasks = tracker.len(),
                            "⚠️ Previous 3-block scan still running! Skipping interval to prevent RPC accumulation."
                        );
                        continue;
                    }

                    last_processed_block = block_number;
                    let readers = self.readers.clone();
                    let payload_tx = self.payload_tx.clone();

                    // Spawn concurrent protocol scanning task
                    tracker.spawn(async move {
                        info!(
                            block = block_number,
                            interval = BLOCK_INTERVAL,
                            "⚡ Initiating candidate discovery across protocols"
                        );

                        let scan_futures = readers.iter().cloned().map(|reader| {
                            let payload_tx = payload_tx.clone();
                            async move {
                                match reader.fetch_liquidation_candidates().await {
                                    Ok(profiles) => {
                                        if profiles.is_empty() {
                                            debug!(
                                                protocol = reader.name(),
                                                block = block_number,
                                                "🔍 Reader scan completed: No unhealthy borrowers found"
                                            );
                                            return;
                                        }

                                        info!(
                                            protocol = reader.name(),
                                            block = block_number,
                                            count = profiles.len(),
                                            "⚡ Found candidate profiles — streaming to execution pipeline"
                                        );

                                        for profile in profiles {
                                            let payload = LiqPayload {
                                                profile,
                                                reader: Arc::clone(&reader),
                                                block_number,
                                            };

                                            info!(
                                                protocol = reader.name(),
                                                borrower = %payload.profile.address,
                                                block = payload.block_number,
                                                "📥 Pushing LiqPayload to pipeline channel"
                                            );

                                            if let Err(e) = payload_tx.send(payload).await {
                                                error!(error = ?e, "❌ Pipeline channel closed downstream");
                                                return;
                                            }
                                        }
                                    }
                                    Err(e) => {
                                        error!(
                                            protocol = reader.name(),
                                            block = block_number,
                                            error = ?e,
                                            "❌ Reader failed to fetch unhealthy borrowers"
                                        );
                                    }
                                }
                            }
                        });

                        // Concurrently query all protocol readers
                        join_all(scan_futures).await;

                        debug!(block = block_number, "✅ Reader scan cycle finished");
                    });
                }
            }
        }

        info!("✅ ProtocolScanner stopped cleanly");
        Ok(())
    }
}