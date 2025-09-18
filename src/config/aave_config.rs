use std::collections::{HashMap, HashSet};

use ethers::{providers::Middleware, signers::{LocalWallet, Signer}, types::Address};

use crate::{abi_bindings::{aave_v3_pool, AaveV3Pool}, constants};
use std::sync::Arc;

#[derive(Debug, Clone)]
pub struct AaveConfig {
    pub wallet: LocalWallet,
    pub lending_pool: Address,
    pub aave_oracle: Address,
    pub flash_liquidator: Address,
    pub rpc_url: String,
    pub dex_router: Address,
    pub ui_pool_data: Address,
    pub pool_address_provider: Address,
    pub subgraph_url: String,
    pub subgraph_api_key: String,
    pub reserves: HashSet<Address>,
    pub vdebt_tokens: HashMap<Address, Address>

}

impl AaveConfig  {
    pub fn load() -> anyhow::Result<Self> {
        let reserves: HashSet<Address> = constants::AAVE_RESERVES
            .into_iter()
            .map(|r|r.to_string().parse::<Address>().unwrap())
            .collect();
    

        Ok(AaveConfig { 
             wallet: super::PRIVATE_KEY.parse::<LocalWallet>()?, 
             lending_pool: super::LENDING_POOL.parse::<Address>()?, 
             aave_oracle: super::AAVE_ORACLE.parse::<Address>()?, 
             flash_liquidator: super::FLASH_LIQUIDATOR.parse::<Address>()?, 
             rpc_url: super::RPC_URL.to_string(), 
             dex_router: super::DEX_ROUTER.parse::<Address>()?, 
             ui_pool_data: super::UIPOOL_DATA.parse::<Address>()?, 
             pool_address_provider: super::POOL_ADDRESS_PROVIDER.parse::<Address>()?,
             subgraph_api_key: super::SUBGRAPH_API_KEY.to_string(),
             subgraph_url: super::SUBGRAPH_URL.to_string(), 
             reserves,
             vdebt_tokens: HashMap::new()
        })
    }

    pub async fn populate_vdebt_tokens<M: Middleware + 'static>(
        &mut self, 
        pool: &AaveV3Pool<M>
    ) -> anyhow::Result<()> {
        let mut mapping = HashMap::new();

        for reserve in &self.reserves {
            let data: aave_v3_pool::ReserveData = pool.get_reserve_data(*reserve).call().await?;
            let vdebt_token = data.variable_debt_token_address;

            mapping.insert(*reserve, vdebt_token);
        }
        self.vdebt_tokens = mapping;

        Ok(())


    }
}