use ethers::{
    providers::{Http, Middleware, Provider},
    signers::Signer,
    types::{Address, Bytes, TransactionRequest, H256, U256, U64},
    utils::{hex, Anvil, AnvilInstance},
};
use serde_json::json;
use std::sync::Arc;

use crate::common::abi_bindings::IFlashLiquidator;
use crate::constants;
use crate::core::{
    ports::EvmSimulator,
    types::{LiquidationJob, LiquidationParams},
};

#[derive(Debug, Clone)]
pub struct SimResult {
    pub success: bool,
    pub return_data: Bytes,
    pub gas_used: U256,
    pub revert_reason: Option<String>,
}

pub struct AnvilSandbox<M> {
    _anvil: AnvilInstance,
    pub provider: Arc<Provider<Http>>,
    pub contract: Arc<IFlashLiquidator<M>>,
}

impl<M: Middleware + 'static> AnvilSandbox<M> {
    pub fn new(
        rpc_url: &str,
        block_number: u64,
        contract: Arc<IFlashLiquidator<M>>,
    ) -> anyhow::Result<Self> {
        let anvil = Anvil::new()
            .fork(rpc_url)
            .fork_block_number(block_number)
            .chain_id(constants::CHAIN_ID)
            .spawn();

        let provider = Arc::new(Provider::<Http>::try_from(anvil.endpoint())?);

        Ok(Self {
            _anvil: anvil,
            provider,
            contract,
        })
    }

    /// Snapshot current state (fast revert later)
    pub async fn snapshot(&self) -> anyhow::Result<U256> {
        let id: U256 = self.provider.request("evm_snapshot", ()).await?;
        Ok(id)
    }

    /// Revert to a snapshot
    pub async fn revert(&self, snapshot_id: U256) -> anyhow::Result<()> {
        let hex_id = format!("0x{:x}", snapshot_id);
        self.provider
            .request::<_, bool>("evm_revert", [hex_id])
            .await?;
        Ok(())
    }

    /// Inject contract bytecode at a specific address
    pub async fn set_code(&self, address: Address, bytecode: Bytes) -> anyhow::Result<()> {
        self.provider
            .request::<[serde_json::Value; 2], ()>(
                "anvil_setCode",
                [json!(address), json!(bytecode)],
            )
            .await?;
        Ok(())
    }

    /// Fund contract or account
    pub async fn set_balance(&self, address: Address, wei: U256) -> anyhow::Result<()> {
        self.provider
            .request::<[serde_json::Value; 2], ()>(
                "anvil_setBalance",
                [json!(address), json!(format!("0x{:x}", wei))],
            )
            .await?;
        Ok(())
    }

    /// Impersonate an address
    pub async fn impersonate(&self, address: Address) -> anyhow::Result<()> {
        self.provider
            .request::<_, ()>("anvil_impersonateAccount", [json!(address)])
            .await?;
        Ok(())
    }

    /// Simulate a liquidation call (Zero-polling latency execution)
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

        // 1. Send transaction - Anvil auto-mines instantly on CPU
        let hash: H256 = self
            .provider
            .request("eth_sendTransaction", [tx.clone()])
            .await?;

        // 2. Query receipt directly without polling latency
        let receipt = self
            .provider
            .get_transaction_receipt(hash)
            .await?
            .ok_or_else(|| {
                anyhow::anyhow!("CRITICAL: Immediate receipt retrieval failed on local node")
            })?;

        result.success = receipt.status == Some(U64::one());
        result.gas_used = receipt.gas_used.unwrap_or_default();

        // 3. Trace transaction to parse outputs and revert reason
        let trace: serde_json::Value = self
            .provider
            .request(
                "debug_traceTransaction",
                (hash, json!({ "tracer": "callTracer" })),
            )
            .await?;

        let trace_data = trace.get("result").unwrap_or(&trace);

        if let Some(return_data) = trace_data["output"].as_str() {
            if let Ok(bytes) = hex::decode(return_data.trim_start_matches("0x")) {
                result.return_data = Bytes::from(bytes);
            }
        }

        if !result.success {
            let err_msg = trace_data["error"]
                .as_str()
                .or_else(|| trace_data["revertReason"].as_str());

            if let Some(msg) = err_msg {
                result.revert_reason = Some(msg.to_string());
            } else if !result.return_data.is_empty() {
                result.revert_reason = Some(self.decode_revert_from_data(&result.return_data));
            } else {
                result.revert_reason = Some("Unknown Revert (No data)".to_string());
            }
        }

        Ok(result)
    }

    fn decode_revert_from_data(&self, data: &[u8]) -> String {
        if data.is_empty() {
            return "Empty revert data".to_string();
        }

        if data.starts_with(&[0x08, 0xc3, 0x79, 0xa0]) && data.len() >= 4 {
            if let Ok(decoded) = ethers::abi::decode(&[ethers::abi::ParamType::String], &data[4..])
            {
                if let Some(first) = decoded.first() {
                    return first.to_string();
                }
            }
        }
        format!("0x{}", hex::encode(data))
    }

    async fn prepare_reset_fork(&self, block_number: u64) -> anyhow::Result<()> {
        let rpc_url = constants::RPC_URL_HTTP.as_str();
        let target_address = *constants::FLASH_LIQUIDATOR;
        let keeper_address = constants::WALLET.address();
        let bytecode = constants::LIQ_BYTECODE.clone();

        self.provider
            .request::<_, ()>(
                "anvil_reset",
                [json!({
                    "forking": {
                        "jsonRpcUrl": rpc_url,
                        "blockNumber": format!("0x{:x}", block_number)
                    }
                })],
            )
            .await?;

        self.set_code(target_address, bytecode).await?;
        self.impersonate(keeper_address).await?;
        self.set_balance(keeper_address, U256::exp10(18) * 50)
            .await?;

        Ok(())
    }
}

#[async_trait::async_trait]
impl<M: Middleware + Send + Sync + 'static> EvmSimulator for AnvilSandbox<M> {
    async fn simulate_liquidation(
        &self,
        block_number: u64,
        job: &LiquidationJob,
    ) -> anyhow::Result<u64> {
        self.prepare_reset_fork(block_number).await.map_err(|e| {
            anyhow::anyhow!(
                "Failed to prepare/reset fork for block {}: {:?}",
                block_number,
                e
            )
        })?;

        let params = LiquidationParams::from(job.clone());

        let snapshot_id = self
            .snapshot()
            .await
            .map_err(|e| anyhow::anyhow!("Failed to acquire clean state snapshot: {:?}", e))?;

        let calldata = self
            .contract
            .execute_flash_liquidation(job.debt_to_cover, params)
            .calldata()
            .ok_or_else(|| {
                anyhow::anyhow!("Failed to synthesize router transaction calldata payload")
            })?;

        let keeper_address = constants::WALLET.address();
        let target_router = *constants::FLASH_LIQUIDATOR;

        // Execute transaction simulation
        let tx_result = self
            .simulate_tx(keeper_address, target_router, calldata, U256::zero())
            .await;

        // Unconditional Reversion Guard
        let revert_status = self.revert(snapshot_id).await;
        if let Err(e) = revert_status {
            tracing::error!(
                "CRITICAL: State reversion guard failed in simulation sandbox: {:?}",
                e
            );
        }

        // Handle transaction execution errors
        let sim_data = tx_result
            .map_err(|e| anyhow::anyhow!("Simulation runner infrastructure failed: {:?}", e))?;

        if !sim_data.success {
            let reason = sim_data
                .revert_reason
                .unwrap_or_else(|| "Unknown Revert".to_string());
            anyhow::bail!("On-chain execution simulation aborted: {}", reason);
        }

        Ok(sim_data.gas_used.as_u64())
    }
}
