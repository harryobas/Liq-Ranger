
use std::sync::Arc;
use tokio::sync::mpsc::UnboundedReceiver;

use crate::{liquidators::Liquidator, models::LiquidationCommand};

pub struct LiquidationWorker {
    bot: Arc<dyn Liquidator + Sync + Send>,
    rx: UnboundedReceiver<LiquidationCommand>
}

impl LiquidationWorker {
    pub fn new(bot: Arc<dyn Liquidator + Send + Sync>, rx: UnboundedReceiver<LiquidationCommand>) -> Self {

        Self { bot, rx }

    }

    pub async fn start(&mut self) -> anyhow::Result<()> {
        while let Some(cmd) = self.rx.recv().await {
            match cmd {
                LiquidationCommand::RunCycle => {
                    log::info!("Worker received RunCycle command");

                    if let Err(e) = self.bot.run().await{
                        log::error!("Bot cycle failed: {:?}", e);
                        continue;
                    }
                },

                LiquidationCommand::Shutdown => {
                    log::info!("worker shutting down......");
                    break;
                },
            }

        }
        log::warn!("Worker stopped because channel closed");
        Ok(())
    }
}