use ethers::{
    types::{U256, Address, H256, Bytes}, 
    providers::Middleware, prelude::abigen};
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
    r#"[
        function execute(uint256 flashAmt, address debtAsset, bytes calldata data) external 
    ]"#
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
    fn calldata(&self, loan_amt: U256, debt_asset: Address, liq_data: Bytes) -> anyhow::Result<Bytes> {
        self.execute(loan_amt, debt_asset, liq_data)
            .calldata()
            .ok_or_else(|| anyhow::anyhow!("Failed to generate calldata"))
    }

    async fn execute_tx(&self, loan_amt: U256, debt_asset: Address, liq_data: Bytes) -> anyhow::Result<H256> {
        // 1. Create the call object
        let call = self.execute(loan_amt, debt_asset, liq_data);

        // 2. Estimate gas + 20% buffer
        let gas = match call.estimate_gas().await {
            Ok(g) => g,
            Err(e) => {
                tracing::warn!("⚠️ Gas estimation failed - likely front-run or state changed: {:?}", e);
                return Err(anyhow::anyhow!("Opportunity no longer valid"));
            }
        };

        let configured_call = call.gas(gas * 120 / 100);

        // 3. Send transaction
        let pending_tx = configured_call.send().await
            .map_err(|e| anyhow::anyhow!("Failed to send tx: {:?}", e))?;
        
        let tx_hash = *pending_tx;
        let provider = self.client().clone();

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
