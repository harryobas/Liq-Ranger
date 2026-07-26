use std::sync::Arc;
use std::time::Duration;

use ethers::{
    providers::Middleware,
    types::{
        transaction::{eip1559::Eip1559TransactionRequest, eip2718::TypedTransaction},
        Address, Bytes, H256, U256,
    },
};

use crate::common::abi_bindings::IFlashLiquidator;
use crate::core::ports::LiquidationContract;
use crate::core::types::{LiquidationJob, LiquidationParams};

pub struct FlashLiquidatorAdapter<M> {
    pub contract: Arc<IFlashLiquidator<M>>,
}

impl<M: Middleware> FlashLiquidatorAdapter<M> {
    pub fn new(contract: Arc<IFlashLiquidator<M>>) -> Self {
        Self { contract }
    }

    pub fn address(&self) -> Address {
        self.contract.address()
    }

    pub fn extract_calldata(
        &self,
        flash_amt: U256,
        params: LiquidationParams,
    ) -> anyhow::Result<Bytes> {
        self.contract
            .execute_flash_liquidation(flash_amt, params)
            .calldata()
            .ok_or_else(|| anyhow::anyhow!("Failed to generate calldata"))
    }
}

#[async_trait::async_trait]
impl<M: Middleware + 'static> LiquidationContract for FlashLiquidatorAdapter<M> {
    async fn execute_liquidation(
        &self,
        job: LiquidationJob,
        gas_limit: U256,
    ) -> anyhow::Result<H256> {
        let debt = job.debt_to_cover;
        let params = LiquidationParams::from(job);
        let provider = self.contract.client().clone();
        let calldata = self.extract_calldata(debt, params)?;

        let (base_max_fee, base_priority_fee) = provider
            .estimate_eip1559_fees(None)
            .await
            .unwrap_or_else(|_| {
                (
                    U256::from(200_000_000_000u64), // 200 Gwei
                    U256::from(50_000_000_000u64),  // 50 Gwei
                )
            });

        // Boost priority fee by 50% for competitive inclusion
        let priority_fee = base_priority_fee * 150 / 100;
        // Ensure max_fee_per_gas covers base fee + priority fee
        let max_fee = (base_max_fee * 120 / 100) + priority_fee;

        let adjusted_gas_limit = gas_limit * 120 / 100;

        let tx = Eip1559TransactionRequest::new()
            .to(self.address())
            .data(calldata)
            .gas(adjusted_gas_limit)
            .max_fee_per_gas(max_fee)
            .max_priority_fee_per_gas(priority_fee);

        let tx_request: TypedTransaction = tx.into();

        let pending_tx = provider
            .send_transaction(tx_request, None)
            .await
            .map_err(|e| anyhow::anyhow!("Failed to send tx: {:?}", e))?;

        let tx_hash = *pending_tx;

        // ✅ Deterministic interval-based polling task
        tokio::spawn({
            let provider = provider.clone();

            async move {
                let mut interval = tokio::time::interval(Duration::from_secs(1));

                for _ in 0..60 {
                    interval.tick().await;

                    match provider.get_transaction_receipt(tx_hash).await {
                        Ok(Some(receipt)) => {
                            if receipt.status == Some(1.into()) {
                                tracing::info!("✅ Liquidation confirmed: {:?}", tx_hash);
                            } else {
                                tracing::error!(
                                    "❌ Liquidation tx reverted on-chain: {:?}",
                                    tx_hash
                                );
                            }
                            return;
                        }
                        Ok(None) => {
                            // Still pending in mempool, tick again
                        }
                        Err(e) => {
                            tracing::error!("❌ Error fetching receipt for {:?}: {:?}", tx_hash, e);
                            return;
                        }
                    }
                }

                tracing::warn!(
                    "⚠️ Polling timed out after 60 seconds for tx: {:?}",
                    tx_hash
                );
            }
        });

        Ok(tx_hash)
    }
}
