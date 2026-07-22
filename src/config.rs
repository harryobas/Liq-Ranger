use ethers::{
    providers::Middleware,
    signers::{LocalWallet, Signer},
    types::{Address, H256, U256},
};
use std::{
    collections::{HashMap, HashSet},
    sync::Arc,
};

use crate::{
    common::{self, Config},
    constants,
};

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

impl Config for AaveConfig {
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
    pub async fn populate_vdebt_tokens<M: Middleware + 'static>(
        &mut self,
        client: Arc<M>,
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

#[derive(Debug, Clone)]
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
    pub oracle_price_scale: U256,
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
            oracle_price_scale,
        })
    }

    fn keeper_address(&self) -> Address {
        self.keeper_address
    }
    fn chain_id(&self) -> u64 {
        self.chain_id
    }
}
