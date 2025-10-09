use std::sync::Arc;
use ethers::providers::{Middleware, PubsubClient};
use futures_util::StreamExt;
use tokio::sync::mpsc::UnboundedSender;
use anyhow::{Context, Result};

use crate::models::LiquidationCommand;

#[derive(Clone, Debug)]
pub struct BlockWatcher<M> {
    provider: Arc<M>,
    tx: UnboundedSender<LiquidationCommand>,
}

impl<M> BlockWatcher<M>
where
    M: Middleware + 'static,
    <M as Middleware>::Provider: PubsubClient,  // ✅ restrict to WS-capable
{
    pub fn new(provider: Arc<M>, tx: UnboundedSender<LiquidationCommand>) -> Self {
        Self { provider, tx }
    }

    /// Run a single subscription session (ends if channel closes or WS drops).
    pub async fn start(&self) -> Result<()> {
        let mut stream = self
            .provider
            .subscribe_blocks()
            .await
            .context("Failed to subscribe to blocks")?;

        log::info!("Block watcher started...");

        while let Some(block) = stream.next().await {
            if let Some(block_number) = block.number {
                log::debug!("New block: {}", block_number);

                if let Err(e) = self.tx.send(LiquidationCommand::RunCycle) {
                    log::error!("Failed to send command: {:?}", e);
                    return Err(anyhow::anyhow!("Command channel closed"));
                }
            }
        }

        Err(anyhow::anyhow!("Block subscription stream ended"))
    }
}
