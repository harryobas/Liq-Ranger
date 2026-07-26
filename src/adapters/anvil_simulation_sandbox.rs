use ethers::{
    abi::{decode, ParamType},
    providers::{Http, Middleware, Provider},
    signers::Signer,
    types::{Address, Bytes, TransactionRequest, U256},
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

    /// One-time setup during engine initialization (avoid running this per-tx)
    pub async fn setup_contracts(&self) -> anyhow::Result<()> {
        let target_address = *constants::FLASH_LIQUIDATOR;
        let keeper_address = constants::WALLET.address();
        let bytecode = constants::LIQ_BYTECODE.clone();

        // Set deployment code
        self.provider
            .request::<[serde_json::Value; 2], ()>(
                "anvil_setCode",
                [json!(target_address), json!(bytecode)],
            )
            .await?;

        // Pre-fund keeper with high balance to eliminate "Insufficient funds for gas * price + value"
        self.provider
            .request::<[serde_json::Value; 2], ()>(
                "anvil_setBalance",
                [
                    json!(keeper_address),
                    json!("0x38D7EA4C6800000000"), // 1,000 ETH
                ],
            )
            .await?;

        Ok(())
    }

    /// Zero-mutation Stateless Simulation via eth_call + eth_estimateGas
    pub async fn simulate_tx_stateless(
        &self,
        from: Address,
        to: Address,
        calldata: Bytes,
        value: U256,
        block_number: u64,
    ) -> anyhow::Result<SimResult> {
        let tx = TransactionRequest::new()
            .from(from)
            .to(to)
            .data(calldata)
            .value(value);

        let tx_typed = tx.clone().into();
        let block_num = Some(block_number.into());

        // 1. Dry run execution via eth_call (no tx pool, no receipt needed)
        let call_res = self.provider.call(&tx_typed, block_num).await;

        match call_res {
            Ok(return_data) => {
                // 2. Gas estimation for accurate cost accounting
                let gas_used = self
                    .provider
                    .estimate_gas(&tx_typed, block_num)
                    .await
                    .unwrap_or_else(|_| U256::from(500_000)); // Default fallback if gas estimation fails

                Ok(SimResult {
                    success: true,
                    return_data,
                    gas_used,
                    revert_reason: None,
                })
            }
            Err(err) => {
                let revert_reason = parse_revert_message(&err.to_string());
                Ok(SimResult {
                    success: false,
                    return_data: Bytes::new(),
                    gas_used: U256::zero(),
                    revert_reason: Some(revert_reason),
                })
            }
        }
    }
}

fn parse_revert_message(err_str: &str) -> String {
    // Attempt to extract standard ABI-encoded Revert(string)
    if let Some(hex_start) = err_str.find("0x") {
        let raw_hex = &err_str[hex_start..];
        let clean_hex = raw_hex
            .split(|c: char| !c.is_ascii_hexdigit())
            .next()
            .unwrap_or(raw_hex);

        if let Ok(bytes) = hex::decode(clean_hex.trim_start_matches("0x")) {
            if bytes.starts_with(&[0x08, 0xc3, 0x79, 0xa0]) && bytes.len() >= 4 {
                if let Ok(decoded) = decode(&[ParamType::String], &bytes[4..]) {
                    if let Some(msg) = decoded.first() {
                        return msg.to_string();
                    }
                }
            }
        }
    }
    err_str.to_string()
}

#[async_trait::async_trait]
impl<M: Middleware + Send + Sync + 'static> EvmSimulator for AnvilSandbox<M> {
    async fn simulate_liquidation(
        &self,
        block_number: u64,
        job: &LiquidationJob,
    ) -> anyhow::Result<u64> {
        let params = LiquidationParams::from(job.clone());

        let calldata = self
            .contract
            .execute_flash_liquidation(job.debt_to_cover, params)
            .calldata()
            .ok_or_else(|| {
                anyhow::anyhow!("Failed to synthesize router transaction calldata payload")
            })?;

        let keeper_address = constants::WALLET.address();
        let target_router = *constants::FLASH_LIQUIDATOR;

        // Execute stateless call without global reset or receipts
        let sim_data = self
            .simulate_tx_stateless(
                keeper_address,
                target_router,
                calldata,
                U256::zero(),
                block_number,
            )
            .await?;

        if !sim_data.success {
            let reason = sim_data
                .revert_reason
                .unwrap_or_else(|| "Execution Reverted".to_string());
            anyhow::bail!("On-chain execution simulation aborted: {}", reason);
        }

        Ok(sim_data.gas_used.as_u64())
    }
}
