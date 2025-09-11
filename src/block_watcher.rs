use ethers::providers::{Middleware, PubsubClient};
use futures_util::StreamExt;

use tokio::sync::mpsc::UnboundedSender;
use crate::models::LiquidationCommand;
use anyhow::Context;
#[derive(Clone, Debug)]
pub struct BlockWatcher<M> {
    provider: std::sync::Arc<M>,
    tx: UnboundedSender<LiquidationCommand>,
}

impl<M> BlockWatcher<M>
where
    M: Middleware + 'static,
    <M as Middleware>::Provider: PubsubClient,  // 🔑 Restrict provider type
{
    pub fn new(provider: std::sync::Arc<M>, tx: UnboundedSender<LiquidationCommand>) -> Self {
        Self { provider, tx }
    }

    pub async fn start(&self) -> anyhow::Result<()> {
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
                    continue;
                    
                }
            }
        }

        Ok(())
    }
}
