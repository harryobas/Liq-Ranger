use std::sync::Arc;

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

        let (max_fee, priority_fee) =
            provider
                .estimate_eip1559_fees(None)
                .await
                .unwrap_or_else(|_| {
                    // Fallback values if the provider fails (200 Gwei max, 50 Gwei priority)
                    (
                        U256::from(200_000_000_000u64),
                        U256::from(50_000_000_000u64),
                    )
                });

        // Build transaction (nonce left empty for middleware to fill)
        let tx = Eip1559TransactionRequest::new()
            .to(self.address())
            .data(calldata)
            .gas(gas_limit * 120 / 100)
            .max_fee_per_gas(max_fee)
            .max_priority_fee_per_gas(priority_fee * 150 / 100);

        // 3. Send transaction
        let tx_request: TypedTransaction = tx.into();
        let pending_tx = provider
            .send_transaction(tx_request, None)
            .await
            .map_err(|e| anyhow::anyhow!("Failed to send tx: {:?}", e))?;

        let tx_hash = *pending_tx;
        let provider_clone = provider.clone();

        tokio::spawn(async move {
            match provider_clone.get_transaction_receipt(tx_hash).await {
                Ok(Some(receipt)) => {
                    if receipt.status != Some(1.into()) {
                        tracing::error!("❌ Liquidation tx reverted: {:?}", tx_hash);
                    } else {
                        tracing::info!("✅ Liquidation confirmed: {:?}", tx_hash);
                    }
                }
                Ok(None) => {
                    tracing::error!("❌ Tx dropped from mempool: {:?}", tx_hash);
                }
                Err(e) => {
                    tracing::error!("❌ Confirmation error for {:?}: {:?}", tx_hash, e);
                }
            }
        });

        Ok(tx_hash)
    }
}
