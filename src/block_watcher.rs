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
        tracing::info!("⛓️  Block watcher started");

        let mut stream = self.client.subscribe_blocks().await?;

        loop {
            tokio::select! {
                _ = self.shutdown.changed() => {
                    tracing::info!("🛑 Block watcher shutting down");
                    break;
                }

                maybe_block = stream.next() => {
                    match maybe_block {
                        Some(block) => {
                            if let Some(number) = block.number {
                                let block_number = number.as_u64();

                                // Ignore error if no receivers
                                let _ = self.tx.send(block_number);

                                tracing::trace!("🧱 New block {}", block_number);
                            }
                        }
                        None => {
                            anyhow::bail!("Block subscription ended unexpectedly");
                        }
                    }
                }
            }
        }

        tracing::info!("✅ Block watcher stopped cleanly");
        Ok(())
    }
}
