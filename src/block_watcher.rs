use std::sync::Arc;

use ethers::providers::{Middleware, PubsubClient};
use futures_util::StreamExt;
use tokio::sync::{broadcast::Sender, watch};

#[derive(Clone)]
pub struct BlockWatcher<M> {
    client: Arc<M>,
    tx: Sender<u64>,
    shutdown: watch::Receiver<bool>,
}

impl<M> BlockWatcher<M>
where
    M: Middleware + 'static,
    <M as Middleware>::Provider: PubsubClient,
{
    pub fn new(
        client: Arc<M>,
        tx: Sender<u64>,
        shutdown: watch::Receiver<bool>,
    ) -> Self {
        Self {
            client,
            tx,
            shutdown,
        }
    }

   pub async fn start(mut self) -> anyhow::Result<()> {
    // 1. Log immediately so we know the task is alive
    tracing::info!("⛓️ Block watcher task initialized");

    loop {
        // 2. Check for shutdown before trying to reconnect
        if *self.shutdown.borrow() { 
            break; 
        }

        tracing::info!("📡 Attempting to subscribe to blocks...");
        
        // 3. Attempt subscription with a timeout or error handling
        let mut stream = match self.client.subscribe_blocks().await {
            Ok(s) => {
                tracing::info!("✅ Block subscription active");
                s
            },
            Err(e) => {
                tracing::error!("❌ Failed to subscribe to blocks: {}. Retrying in 5s...", e);
                tokio::time::sleep(tokio::time::Duration::from_secs(5)).await;
                continue;
            }
        };

        // 4. Inner loop for processing blocks
        loop {
            tokio::select! {
                _ = self.shutdown.changed() => {
                    tracing::info!("🛑 Block watcher shutting down");
                    return Ok(());
                }

                maybe_block = stream.next() => {
                    match maybe_block {
                        Some(block) => {
                            if let Some(number) = block.number {
                                let block_number = number.as_u64();
                                let _ = self.tx.send(block_number);
                                tracing::trace!("🧱 New block {}", block_number);
                            }
                        }
                        None => {
                            tracing::warn!("⚠️ Block stream ended. Reconnecting...");
                            break; // Exit inner loop to re-subscribe
                        }
                    }
                }
            }
        }
    }

    Ok(())

    }
}
