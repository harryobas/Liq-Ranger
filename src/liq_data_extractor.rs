use crate::{common::{
    DistributionRecord, LiquidationRecord, abi_bindings::{
        IFlashLiquidator, 
        IFlashLiquidatorEvents, 
        PositionLiquidatedFilter,
        ProfitDistributedFilter}, 
        get_token_symbol,
        get_token_decimals,
    }};
use std::sync::Arc;
use ethers::{
    providers::Middleware,
    types::{Address, U256},
};
use tokio::sync::watch;
use futures_util::{self, StreamExt};

pub struct LiqDataExtractor<M: Middleware + 'static> {
    flash_liquidator: Arc<IFlashLiquidator<M>>,
    db_pool: sqlx::SqlitePool,
    shutdown: watch::Receiver<bool>,
    provider: Arc<M>,
}

impl<M: Middleware + 'static> LiqDataExtractor<M> {
    pub fn new(
        flash_liquidator: Arc<IFlashLiquidator<M>>,
        db_pool: sqlx::SqlitePool,
        shutdown: watch::Receiver<bool>,
        provider: Arc<M>,
    ) -> Self {
        Self {
            flash_liquidator,
            db_pool,
            shutdown,
            provider,
        }
    }

    pub async fn start(mut self) -> anyhow::Result<()> {
        tracing::info!("📡 LiqDataExtractor started");

        // Listen for liquidation events
        let events = self.flash_liquidator.events();
        let mut event_stream = events.stream_with_meta().await?;

        loop {
            tokio::select! {
                // 🔴 Shutdown
                _ = self.shutdown.changed() => {
                    tracing::info!("🛑 LiqDataExtractor shutting down");
                    break;
                }

                // 🟢 New liquidation event
                evt = event_stream.next() => {
                    match evt {
                        Some(Ok((event, meta))) => {
                            tracing::info!("🔔 New liquidation event: {:?}", event);
                            let tx_hash = format!("{:?}", meta.transaction_hash);
                            let block_number = meta.block_number.as_u64() as i64;
                            self.handle_event(event, tx_hash, block_number).await?;
                        }
                        Some(Err(e)) => {
                            tracing::error!("❌ Error processing event: {:?}", e);
                        }
                        None => {
                            tracing::info!("📭 Event stream ended");
                            break;
                        }
                    }
                }
            }
        }
        Ok(())
    }

    async fn handle_event(&self, evt: IFlashLiquidatorEvents, tx_hash: String, block_number: i64) -> anyhow::Result<()> {
        match evt {
            IFlashLiquidatorEvents::PositionLiquidatedFilter(e) => {
                self.handle_liquidation_event(e, &tx_hash, block_number).await
            },
            IFlashLiquidatorEvents::ProfitDistributedFilter(e) => {
                self.handle_distribution_event(e, &tx_hash).await
            },
            _ => Ok(()), 
        }
    }

    async fn handle_liquidation_event(&self, evt: PositionLiquidatedFilter, tx_hash: &str, block_number: i64) -> anyhow::Result<()> {
        let timestamp = evt.timestamp.as_u64() as i64;
        let borrower = evt.borrower;
        let collateral_asset = evt.collateral_asset;

        let collateral_symbol = get_token_symbol(collateral_asset, self.provider.clone())
            .await
            .unwrap_or_else(|_| "UNKNOWN".to_string());

        let profit_asset = evt.profit_asset;

        let profit_symbol = get_token_symbol(profit_asset, self.provider.clone())
            .await
            .unwrap_or_else(|_| "UNKNOWN".to_string());

        let profit_amount = self.compute_amount(evt.profit, profit_asset).await?;

        let protocol = match evt.mode {
            0 => "Aave",
            1 => "Morpho",
            2 => "Compound",
            _ => "Unknown",
        }.to_string();

        let record = LiquidationRecord {
            timestamp,
            tx_hash: tx_hash.to_string(),
            borrower,
            collateral_asset,
            collateral_symbol,
            profit_asset,
            profit_symbol,
            profit_amount,
            protocol,
            block_number,
        };

        record.save(&self.db_pool).await?;

        Ok(())
    }

    async fn handle_distribution_event(&self, evt: ProfitDistributedFilter, tx_hash: &str) -> anyhow::Result<()> {
        let timestamp = evt.timestamp.as_u64() as i64;
        let asset = evt.asset;

        let asset_symbol = get_token_symbol(asset, self.provider.clone())
            .await
            .unwrap_or_else(|_| "UNKNOWN".to_string());

        let owner_share = self.compute_amount(evt.owner_share, asset).await?;
        let breet_share = self.compute_amount(evt.breet_share, asset).await?;

        let amount = owner_share + breet_share;

        let record = DistributionRecord {
            timestamp,
            tx_hash: tx_hash.to_string(),
            asset: asset.to_string(),
            asset_symbol,
            owner_share,
            breet_share,
            amount,
        };

        record.save(&self.db_pool).await?;

        Ok(())
    }

    async fn compute_amount(&self, raw_amount: U256, asset: Address) -> anyhow::Result<f64> {
        let decimals = get_token_decimals(asset, self.provider.clone()).await.unwrap_or_else(|_| 18);
        let amount = ethers::utils::format_units(raw_amount, decimals as usize)?;
        Ok(amount.parse::<f64>().unwrap_or_else(|_| 0.0))
    }

}

   