use std::collections::HashSet;
use ethers::{signers::{LocalWallet, Signer}, types::{Address, H256, U256}};

use crate::{common::Config, constants};

pub struct MorphoConfig {
    pub morpho_blue: Address,
    pub flash_liquidator: Address,
    pub morpho_markets: HashSet<H256>,
    pub rpc_url: String,
    pub wallet: LocalWallet,
    pub db_path: String,
    pub block_interval: u64,
    pub keeper_address: Address,
    pub chain_id: u64,
    pub oracle_price_scale: U256

}

impl Config for MorphoConfig {
     fn load() -> anyhow::Result<Self> {
        let morpho_blue: Address = *constants::MORPHO_BLUE;
        let flash_liquidator: Address = *constants::FLASH_LIQUIDATOR;
        let morpho_markets = constants::MORPHO_MARKETS.clone();
        let rpc_url = constants::RPC_URL.clone();
        let wallet = constants::WALLET.clone();
        let db_path = constants::SLED_PATH.into();
        let block_interval = constants::LIQ_EXECUTOR_INTERVAL;
        let keeper_address = wallet.address();
        let chain_id = constants::CHAIN_ID;
        let oracle_price_scale = constants::ORACLE_PRICE_SCALE.clone();
        Ok(Self { 
            morpho_blue, 
            flash_liquidator, 
            morpho_markets, 
            rpc_url, 
            wallet,
            db_path,
            block_interval,
            keeper_address,
            chain_id,
            oracle_price_scale
        })
        
    }

    fn keeper_address(&self) -> Address {
        self.keeper_address
    }
    fn chain_id(&self) -> u64 {
        self.chain_id
    }
      
}