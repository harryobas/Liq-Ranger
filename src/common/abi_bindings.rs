use ethers::{
    prelude::abigen, providers::Middleware, types::{Address, Bytes, H256, U256, transaction::{eip1559::Eip1559TransactionRequest, eip2718::TypedTransaction}}};
use crate::common::LiquidationContract;

abigen!(
    IERC20,
    r#"[
        function balanceOf(address account) external view returns (uint256)
        function decimals() external view returns (uint8)
        function symbol() external view returns (string)
    ]"#
);

abigen!(
    IFlashLiquidator,
     "abis/liquidator/flash_liquidator.json",
     event_derives(serde::Deserialize, serde::Serialize)
);

#[async_trait::async_trait]
impl<M> LiquidationContract<M> for IFlashLiquidator<M>
where
    M: Middleware + 'static,
{
    fn address(&self) -> Address {
        // Use the trait-based method to get the contract address
        ethers::contract::Contract::address(self)
    }

    /// Generates the calldata for the execute function
    /// Note: Added debt_asset to match the ABI
    fn extract_calldata(&self, flash_amt: U256, liq_params: LiquidationParams) -> anyhow::Result<Bytes> {
        self.execute_flash_liquidation(flash_amt, liq_params)
            .calldata()
            .ok_or_else(|| anyhow::anyhow!("Failed to generate calldata"))
    }

    async fn execute_tx(&self, flash_amt: U256,  liq_params: LiquidationParams) -> anyhow::Result<H256> {
        // 1. Create the call object
        //let call = self.execute(loan_amt, debt_asset, liq_data);
        let provider = self.client().clone();

        let calldata = self.extract_calldata(flash_amt, liq_params.clone())?;
        let call = self.execute_flash_liquidation(flash_amt, liq_params);


          // 1. Get current EIP‑1559 fee suggestions (base fee + priority fee)
        let (max_fee, priority_fee) = provider
            .estimate_eip1559_fees(None).await
            .unwrap_or_else(|_| {
            // Fallback values if the provider fails (200 Gwei max, 50 Gwei priority)
                (U256::from(200_000_000_000u64), U256::from(50_000_000_000u64))
            });

        // 2. Estimate gas + 20% buffer
        let gas = match call.estimate_gas().await {
            Ok(g) => g,
            Err(e) => {
                tracing::warn!("⚠️ Gas estimation failed - likely front-run or state changed: {:?}", e);
                return Err(anyhow::anyhow!("Opportunity no longer valid"));
            }
        };

        // Build transaction (nonce left empty for middleware to fill)
        let tx = Eip1559TransactionRequest::new()
            .to(self.address())
            .data(calldata)
            .gas(gas * 120/100)
            .max_fee_per_gas(max_fee)
            .max_priority_fee_per_gas(priority_fee * 150 / 100);

        // 3. Send transaction
        let tx_request: TypedTransaction = tx.into();
        let pending_tx = provider.send_transaction(tx_request, None).await
            .map_err(|e| anyhow::anyhow!("Failed to send tx: {:?}", e))?;
        
        let tx_hash = *pending_tx;
        let provider = provider.clone();

        tokio::spawn(async move {
            match provider.get_transaction_receipt(tx_hash).await {
                Ok(Some(receipt)) => {
                    if receipt.status != Some(1.into()) {

                        tracing::error!("❌ Liquidation tx reverted: {:?}", tx_hash);
                    }else {
                             
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
