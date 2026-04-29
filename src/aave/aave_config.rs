use std::{collections::{HashMap, HashSet}, sync::Arc};
use ethers::{providers::Middleware, signers::{LocalWallet, Signer}, types::Address};

use crate::{common::{self, Config}, constants};


#[derive(Debug, Clone)]
pub struct AaveConfig {
    pub wallet: LocalWallet,
    pub lending_pool: Address,
    pub aave_oracle: Address,
    pub flash_liquidator: Address,
    pub rpc_url: String,
    pub ui_pool_data: Address,
    pub pool_address_provider: Address,
    pub reserves: HashSet<Address>,
    pub vdebt_tokens: HashMap<Address, Address>,
    pub chain_id: u64,
    
    

}

impl Config for  AaveConfig  {
     fn load() -> anyhow::Result<Self> {
       
        Ok(AaveConfig { 
             wallet: constants::WALLET.clone(),
             lending_pool: *constants::AAVE_V3_POOL, 
             aave_oracle: *constants::AAVE_ORACLE, 
             flash_liquidator: *constants::FLASH_LIQUIDATOR, 
             rpc_url: constants::RPC_URL.to_string(), 
             ui_pool_data: *constants::UIPOOL_DATA_PROVIDER, 
             pool_address_provider: *constants::POOL_ADDRESS_PROVIDER,
             reserves: constants::AAVE_RESERVES.clone(),
             vdebt_tokens: HashMap::new(),
             chain_id: constants::CHAIN_ID,
            
        })
    }

    fn chain_id(&self) -> u64 {
        self.chain_id
    }

    fn keeper_address(&self) -> Address {
        self.wallet.address()
        
    }
 
}

impl AaveConfig {
       pub async fn populate_tokens<M: Middleware + 'static>(
        &mut self, 
        client: Arc<M>
    ) -> anyhow::Result<()> {
        let mut vdebt_mapping = HashMap::new();
        

        for reserve in &self.reserves {
            let pool = common::fetch_contracts(client.clone())?.aave;
            let data = pool.get_reserve_data(*reserve).call().await?;
            let vdebt_token = data.variable_debt_token_address;
        
            vdebt_mapping.insert(*reserve, vdebt_token);
        }
        self.vdebt_tokens = vdebt_mapping;
    

        Ok(())
    }
}


