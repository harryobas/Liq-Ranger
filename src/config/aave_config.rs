use ethers::{signers::LocalWallet, types::Address};

#[derive(Debug, Clone)]
pub struct AaveConfig {
    pub private_key: LocalWallet,
    pub lending_pool: Address,
    pub aave_oracle: Address,
    pub flash_liquidator: Address,
    pub rpc_url: String,
    pub dex_router: Address,
    pub ui_pool_data: Address,
    pub pool_address_provider: Address,
    pub subgraph_url: String,
    pub subgraph_api_key: String

}

impl AaveConfig  {
    pub fn load() -> anyhow::Result<Self> {
        Ok(AaveConfig { 
             private_key: super::PRIVATE_KEY.parse::<LocalWallet>()?, 
             lending_pool: super::LENDING_POOL.parse::<Address>()?, 
             aave_oracle: super::AAVE_ORACLE.parse::<Address>()?, 
             flash_liquidator: super::FLASH_LIQUIDATOR.parse::<Address>()?, 
             rpc_url: super::RPC_URL.to_string(), 
             dex_router: super::DEX_ROUTER.parse::<Address>()?, 
             ui_pool_data: super::UIPOOL_DATA.parse::<Address>()?, 
             pool_address_provider: super::POOL_ADDRESS_PROVIDER.parse::<Address>()?,
             subgraph_api_key: super::SUBGRAPH_API_KEY.to_string(),
             subgraph_url: super::SUBGRAPH_URL.to_string() 
        })
    }
}