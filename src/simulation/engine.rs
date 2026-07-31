use ethers::{
    providers::Middleware,
    types::{Address, Bytes as eBytes, U256},
};
use revm::{
    db::CacheDB,
    primitives::{Bytes, ExecutionResult},
    EVM,
};
use std::collections::HashMap;

use super::snapshot::{build_block_env, build_tx_env, BlockSnapshot};
use crate::core::types::SimulationResult;

#[derive(Clone, Debug)]
pub struct SimulationTx {
    pub from: Address,
    pub to: Address,
    pub calldata: eBytes,
    pub value: U256,
}

pub struct SimulationEngine {
    chain_id: u64,
    error_registry: HashMap<[u8; 4], &'static str>,
}

impl SimulationEngine {
    pub fn new(chain_id: u64) -> Self {
        let mut error_registry = HashMap::new();
        error_registry.insert([0x1e, 0x5d, 0x4c, 0x3f], "InsufficientCollateral()");
        error_registry.insert([0x81, 0x1d, 0xcd, 0x3e], "HealthFactorNotBelowThreshold()");
        error_registry.insert([0xed, 0x3d, 0x24, 0x98], "InsufficientFlashLoanLiquidity()");
        error_registry.insert([0x0e, 0x22, 0x1f, 0xc1], "PriceSlippageExceeded()");

        Self {
            chain_id,
            error_registry,
        }
    }

    pub fn simulate<M>(
        &self,
        snapshot: &BlockSnapshot<M>,
        tx: SimulationTx,
    ) -> anyhow::Result<SimulationResult>
    where
        M: Middleware + Send + Sync + 'static,
        <M as Middleware>::Error: 'static,
    {
        // Zero-allocation local overlay referencing snapshot.db directly
        let mut local_db = CacheDB::new(snapshot.db.as_ref());

        let mut evm = EVM::new();
        evm.database(&mut local_db);

        // 1. Sync CfgEnv chain_id with target chain
        evm.env.cfg.chain_id = self.chain_id;

        // 2. Set BlockEnv
        evm.env.block = build_block_env(&snapshot.block);

        // 3. Set TxEnv
        evm.env.tx = build_tx_env(
            tx.from,
            tx.to,
            tx.calldata,
            tx.value,
            &snapshot.block,
            self.chain_id,
        );

        // Optional Bypass: Set tx.chain_id to None if you want to skip EIP-155 strict checks during dry-runs
        evm.env.tx.chain_id = None;

        let exec_result = evm
            .transact()
            .map_err(|e| anyhow::anyhow!("EVM execution failed: {:?}", e))?;

        match exec_result.result {
            ExecutionResult::Success { gas_used, .. } => Ok(SimulationResult {
                success: true,
                gas_used,
                revert_reason: None,
            }),
            ExecutionResult::Revert { output, gas_used } => Ok(SimulationResult {
                success: false,
                gas_used,
                revert_reason: Some(self.decode_revert(&output)),
            }),
            ExecutionResult::Halt { reason, gas_used } => Ok(SimulationResult {
                success: false,
                gas_used,
                revert_reason: Some(format!("EVM Halted: {:?}", reason)),
            }),
        }
    }

    fn decode_revert(&self, bytes: &Bytes) -> String {
        if bytes.len() < 4 {
            return format!("Execution reverted (0x{})", hex::encode(bytes));
        }

        let mut selector = [0u8; 4];
        selector.copy_from_slice(&bytes[..4]);
        let data = &bytes[4..];

        match selector {
            // Standard Error(string) selector: 0x08c379a0
            [0x08, 0xc3, 0x79, 0xa0] => {
                if let Ok(decoded) = ethers::abi::decode(&[ethers::abi::ParamType::String], data) {
                    if let Some(msg) = decoded.first() {
                        return format!("Revert: {}", msg);
                    }
                }
            }
            // Standard Panic(uint256) selector: 0x4e487b71
            [0x4e, 0x48, 0x7b, 0x71] => {
                if let Ok(decoded) = ethers::abi::decode(&[ethers::abi::ParamType::Uint(256)], data)
                {
                    if let Some(code) = decoded.first() {
                        return format!("Panic Code: {}", code);
                    }
                }
            }
            _ => {
                if let Some(known_error) = self.error_registry.get(&selector) {
                    return format!("Custom Error: {}", known_error);
                }
            }
        }

        format!("CustomError(0x{})", hex::encode(selector))
    }
}