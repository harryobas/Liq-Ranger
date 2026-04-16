use std::sync::Arc;
use ethers::{
    providers::{Http, Middleware, Provider}, 
    types::{
        Address, 
        Bytes, 
        TransactionRequest, U256, U64, H256, TransactionReceipt}, utils::{Anvil, AnvilInstance, hex}};
use crate::constants;

use serde_json::json;

#[derive(Debug)]
pub struct SimResult{
    pub success: bool,
    pub return_data: Bytes,
    pub gas_used: U256,
    pub revert_reason: Option<String>
}

pub struct AnvilSandbox {
    _anvil: AnvilInstance,
    pub provider: Arc<Provider<Http>>
}

impl AnvilSandbox {
    pub fn new(rpc_url: &str, block_number: u64) -> anyhow::Result<Self> {
        let anvil = Anvil::new()
            .fork(rpc_url)
            .fork_block_number(block_number)
            .chain_id(constants::CHAIN_ID)
            .spawn();

        let provider = Arc::new(Provider::<Http>::try_from(anvil.endpoint())?);

        Ok(Self { _anvil: anvil, provider})
    }

    /// Snapshot current state (fast revert later)
    pub async fn snapshot(&self) -> anyhow::Result<U256> {
        let id: U256 = self.provider.request("evm_snapshot", ()).await?;
        Ok(id)

    }

     /// Revert to a snapshot
    pub async fn revert(&self, snapshot_id: U256) -> anyhow::Result<()> {
        let hex_id = format!("0x{:x}", snapshot_id);
        self.provider.request::<_, ()>("evm_revert", [hex_id]).await?;
        Ok(())
    }

    /// Inject contract bytecode at a specific address
    pub async fn set_code(&self, address: Address, bytecode: Bytes) -> anyhow::Result<()> {
        self.provider.request::<[serde_json::Value; 2], ()>(
            "anvil_setCode",
            [json!(address), json!(bytecode)],
        ).await?;
        Ok(())
    }

     /// Fund contract or account
    pub async fn set_balance(&self, address: Address, wei: U256) -> anyhow::Result<()> {
        self.provider.request::<[serde_json::Value; 2], ()>(
            "anvil_setBalance",
            [json!(address), json!(format!("0x{:x}", wei))],
        ).await?;
        Ok(())
    }

     /// Impersonate an address
    pub async fn impersonate(&self, address: Address) -> anyhow::Result<()> {
        self.provider.request::<_, ()>(
            "anvil_impersonateAccount",
            [json!(address)],
        ).await?;
        Ok(())
    }

    /// Simulate a liquidation call
    pub async fn simulate_tx(

        &self,
        from: Address,
        to: Address,
        calldata: Bytes,
        value: U256,
    ) -> anyhow::Result<SimResult> {
        let tx = TransactionRequest::new()
            .to(to)
            .from(from)
            .data(calldata)
            .value(value);

        let mut result = SimResult {
            success: false,
            return_data: Bytes::new(),
            gas_used: U256::zero(),
            revert_reason: None,
        };

        // Send the transaction
        let hash: H256 = self.provider
            .request("eth_sendTransaction", [tx.clone()])
            .await?;

        // Wait for receipt with timeout
        let receipt = tokio::time::timeout(
            std::time::Duration::from_secs(5),
    self.wait_for_receipt(hash),
        )
        .await??;

        result.success = receipt.status == Some(U64::one());
        result.gas_used = receipt.gas_used.unwrap_or_default();

        // Trace the transaction to get return data and revert reason
        let trace: serde_json::Value = self.provider
            .request(
            "debug_traceTransaction",
        (hash, serde_json::json!({ "tracer": "callTracer" })) 
            )
            .await?;
        let trace_data = trace.get("result").unwrap_or(&trace);

        if let Some(return_data) = trace_data["output"].as_str() {
            if let Ok(bytes) = hex::decode(return_data.trim_start_matches("0x")) {
                result.return_data = Bytes::from(bytes);
            }
        }

        if !result.success {

             // Check all common places for revert strings
            let err_msg = trace_data["error"].as_str()
                .or_else(|| trace_data["revertReason"].as_str());

            if let Some(msg) = err_msg {
                 result.revert_reason = Some(msg.to_string());
            } else if !result.return_data.is_empty() {
                // Decode Solidity Error(string) or Panic
                result.revert_reason = Some(self.decode_revert_from_data(&result.return_data));
            } else {
                result.revert_reason = Some("Unknown Revert (No data)".to_string());
            }
        }

        Ok(result)
    }

    // Helper to wait for receipt
    async fn wait_for_receipt(&self, hash: H256) -> anyhow::Result<TransactionReceipt> {
        if let Some(receipt) = self.provider.get_transaction_receipt(hash).await? {
            return Ok(receipt);
            
        }
        loop {
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
            if let Some(receipt) = self.provider.get_transaction_receipt(hash).await? {

                return Ok(receipt);
            }   
        }
    }
   

    fn decode_revert_from_data(&self, data: &[u8]) -> String {
        if data.is_empty() {
            return "Empty revert data".to_string();
        }

        // Standard Solidity revert: Error(string) -> 0x08c379a0
        if data.starts_with(&[0x08, 0xc3, 0x79, 0xa0]) && data.len() >= 4 {
            if let Ok(decoded) = ethers::abi::decode(
                &[ethers::abi::ParamType::String], 
                &data[4..]) 
            {
                return decoded[0].to_string();
            }
        }
        // Fallback: show the hex for custom errors or PANICs
        format!("0x{}", hex::encode(data))
    }

}
