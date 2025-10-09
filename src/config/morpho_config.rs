use std::{collections::HashSet, str::FromStr};

use ethers::{signers::{LocalWallet, Signer}, types::{Address, H256}};

use crate::constants;

pub struct MorphoConfig {
    pub wallet: LocalWallet,
    pub morpho_blue: Address,
    pub flash_liquidator: Address,
    pub rpc_url: String,
    pub dex_router: Address,
    pub markets: HashSet<H256>

}

impl MorphoConfig {

    pub fn load() -> anyhow::Result<Self> {

        let markets: HashSet<H256> = constants::MORPHO_MARKETS
            .into_iter()
            .map(|m| H256::from_str(m).unwrap())
            .collect();

        Ok(MorphoConfig{
            wallet: super::PRIVATE_KEY.parse::<LocalWallet>()?.with_chain_id(137u64),
            morpho_blue: super::MORPHO_BLUE.parse::<Address>()?,
            flash_liquidator: super::FLASH_LIQUIDATOR.parse::<Address>()?,
            rpc_url: super::RPC_URL.to_string(),
            dex_router: super::DEX_ROUTER.parse::<Address>()?,
            markets

        })
    }
}