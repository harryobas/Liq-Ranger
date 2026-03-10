
pub mod paraswap;
pub mod task_manager;
pub mod liq_data;
pub mod abi_bindings;

use ethers::signers::Signer;
use ethers::types::Address;

use std::{sync::Arc, str::FromStr};
use ethers::providers::Middleware;

use ethers::{ 
    utils::hex,
    types::{Bytes, U256, H256 as TxHash}
};

use tenderly_rs::{
    TraceResponse,
    Network, 
    Tenderly,
    TenderlyConfiguration,
    executors::types::{TransactionParameters, SimulationParameters}
};

use crate::aave::abi_bindings::IAaveV3Pool;
use crate::common::abi_bindings::{IFlashLiquidator, LiquidationParams};
use crate::compound::abi_bindings::IComet;
use crate::constants;
use crate::morpho::abi_bindings::IMorphoBlue;
use crate::{
    constants::{
        TOKEN_DECIMAL_CACHE, 
        TENDERLY_ACCESS_KEY, 
        TENDERLY_PROJECT, 
        TENDERLY_ACCOUNT,
        TOKEN_SYMBOL_CACHE
    }, 
    common::abi_bindings::IERC20
};


#[async_trait::async_trait]
pub trait Liquidator: Send + Sync {
    async fn run(&self) -> anyhow::Result<()>;

}

#[async_trait::async_trait]
pub trait WatchList<T>: Sync + Send {
     async fn remove(&self, item: T) -> anyhow::Result<()>;
     async fn add(&self, item: T) -> anyhow::Result<()>;     
}

pub trait Config: Send + Sync {
    fn load() -> anyhow::Result<Self>
    where
        Self: Sized;
    
    fn keeper_address(&self) -> Address;
    fn chain_id(&self) -> u64;
}

#[async_trait::async_trait]
pub trait LiquidationContract<M: Middleware + 'static>: Send + Sync{
    fn address(&self) -> Address;
    async fn execute_tx(&self, flash_amt: U256,  liq_params: LiquidationParams,) -> anyhow::Result<TxHash>;
    fn extract_calldata(&self, flash_amt: U256,  liq_params: LiquidationParams) -> anyhow::Result<Bytes>;
}

/// Swap query parameters
#[derive(Debug, Clone)]
pub struct SwapQueryParams {
    pub src_token: String,
    pub dest_token: String,
    pub src_decimals: u8,
    pub dest_decimals: u8,
    pub amount: String, // in wei
    pub side: String,   // "SELL" or "BUY"
    pub chain_id: u64,
    pub user_address: String, // flash_liquidator contract
    pub receiver: String,     // typically same flash_liquidator
    pub slippage_bps: u32,
}

pub enum AdminCmd {
    Prune,
    StatusCheck,
    
}

pub struct CoreContracts<M>{
    pub aave: IAaveV3Pool<M>,
    pub morpho: IMorphoBlue<M>,
    pub comet: IComet<M>,
    pub flash_liq: IFlashLiquidator<M>
}

pub async fn execute_liq_tx<M: Middleware + 'static>(
    loan_amt: U256,
    liq_params: LiquidationParams,
    flash_liq: &dyn LiquidationContract<M>
) -> anyhow::Result<TxHash> {
    flash_liq.execute_tx(loan_amt,  liq_params).await
}

pub async fn simulate_liq_tx<M: Middleware + 'static>(
    flash_liq: &dyn LiquidationContract<M>, 
    provider: Arc<M>,
    loan_amt: U256,
    liq_params: LiquidationParams
) -> anyhow::Result<()>{
    // 1. Initialize Tenderly SDK client
    let tenderly = Tenderly::new(TenderlyConfiguration::new(
        TENDERLY_ACCOUNT.to_string(), 
        TENDERLY_PROJECT.to_string(),
        TENDERLY_ACCESS_KEY.clone(),
        Network::Polygon
    ))?;

    // 2. Build the simulation parameters from the contract call
    let target_address = flash_liq.address();
    let call_data = flash_liq.extract_calldata(loan_amt, liq_params)?;

    let gas_price = provider.get_gas_price().await?;

    let transaction = TransactionParameters {
        from: constants::WALLET.address().to_string(), // Your bot's address
        to: target_address.to_string(),
        gas: 0, // Tenderly estimates this
        gas_price: gas_price.to_string(),
        value: "0".to_string(),
        input: format!("0x{}", hex::encode(call_data)),
        max_fee_per_gas: None,
        max_priority_fee_per_gas: None,
        access_list: None,
    };

    let simulation = SimulationParameters {
        transaction,
        block_number: provider.get_block_number().await.map(|v| v.as_u64())?,//None, // Simulate on latest block
        overrides: None, // You can use state overrides here to test edge cases
    };

    // 3. Execute the simulation
    match tenderly.simulator.simulate_transaction(&simulation).await {
        Ok(sim_result) => {
            if sim_result.status == Some(true) {
                tracing::info!(
                    "Tenderly simulation successful. Gas used: {:?}, Block nimber: {:?} Logs: {:?}", 
                    sim_result.gas_used.unwrap_or(0), sim_result.block_number, sim_result.logs);
                // Optional: Calculate estimated profit from simulation traces here
            } else {
                if let Some(traces) = &sim_result.trace {
                    if let Some(error_msg) = extract_error_from_trace(traces)  {
                        tracing::warn!("Simulation failed: {}", error_msg);
                        return Err(anyhow::anyhow!("Simulation failed: {}", error_msg));
                        
                    }
                }
            }
        }
        Err(e) => {
            tracing::error!(error = ?e, "Tenderly simulation failed");
            return Err(e.into());
        }
    }

    Ok(())


}

fn extract_error_from_trace(traces: &[TraceResponse]) -> Option<String> {
    for trace in traces {
        // Check for any error fields in the trace
        if let Some(error) = &trace.error {
            return Some(format!("Trace error: {}", error));
        }
        if let Some(error_reason) = &trace.error_reason {
            return Some(format!("Error reason: {}", error_reason));
        }
        if let Some(error_messages) = &trace.error_messages {
            return Some(format!("Error messages: {}", error_messages));
        }
        
        // Also check if this is a CALL that reverted (common pattern)
        if trace.r#type.as_deref() == Some("CALL") && trace.output.as_deref() == Some("0x") {
            return Some("Call reverted with empty output".to_string());
        }
    }
    
    None
}

pub async fn get_token_decimals<M: Middleware + 'static>(
    token: Address,
    provider: Arc<M>
) -> anyhow::Result<u8> {
    if let Some(dec) = TOKEN_DECIMAL_CACHE.get(&token) {
        return Ok(dec.value().clone());
    }

    let contract = IERC20::new(token, provider.clone());
    let result = contract.decimals().call().await?;

    TOKEN_DECIMAL_CACHE.insert(token, result);
    Ok(result)

}

pub async fn get_token_symbol<M: Middleware + 'static>(
    token: Address, 
    provider: Arc<M>
) -> anyhow::Result<String> {
    if let Some(dec) = TOKEN_SYMBOL_CACHE.get(&token) {
        return Ok(dec.value().clone());
    }

    let contract = IERC20::new(token, provider.clone());
    let result = contract.symbol().call().await?;

    TOKEN_SYMBOL_CACHE.insert(token, result.clone());
    Ok(result)
    
}

pub fn fetch_contracts<M: Middleware + 'static>(client: Arc<M>) -> anyhow::Result<CoreContracts<M>> {
        let liq_addr = Address::from_str(constants::FLASH_LIQUIDATOR)?;
        let aave_addr = Address::from_str(constants::AAVE_V3_POOL)?;
        let comet_addr = Address::from_str(constants::COMET)?;
        let morpho_addr = Address::from_str(constants::MORPHO_BLUE)?;

        let flash_liq = IFlashLiquidator::new(liq_addr, client.clone());
        let aave = IAaveV3Pool::new(aave_addr, client.clone());
        let comet = IComet::new(comet_addr, client.clone());
        let morpho = IMorphoBlue::new(morpho_addr, client.clone());

        Ok(CoreContracts { aave, morpho, comet, flash_liq })
}






   


    
