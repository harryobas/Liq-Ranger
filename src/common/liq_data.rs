use ethers::types::{H256, U256, Address, Bytes};

use crate::{
    aave::types::LiquidationCandidate,
    morpho::types::LiqCandidate
};

#[repr(u8)]
#[derive(Debug, Clone)]
pub enum LiquidationMode {
    Aave = 0,
    Morpho = 1
}

pub struct LiqData {
    pub mode: LiquidationMode,
    pub borrower: Address,

    // Aave
    pub aave_debt_asset: Address,
    pub aave_collateral: Address,
    pub aave_debt_to_cover: U256,

    // Morpho
    pub morpho_market_id: H256,
    pub morpho_repaid_shares: U256,
    pub morpho_seized_assets: U256,

    // Swap
    pub swap_target: Address,
    pub swap_allowance_target: Address,
    pub swap_data: Bytes,
}

impl From<LiquidationCandidate> for LiqData {
    fn from(value: LiquidationCandidate) -> Self {
        Self {
             mode: LiquidationMode::Aave,
             borrower: value.borrower, 
             aave_debt_asset: value.debt_asset,
             aave_collateral: value.collateral_asset, 
             aave_debt_to_cover: value.debt_to_cover, 
             morpho_market_id: H256::zero(), 
             morpho_repaid_shares: U256::zero(), 
             morpho_seized_assets: U256::zero(), 
             swap_target: value.swap_target, 
             swap_allowance_target: value.swap_proxy, 
             swap_data: value.swap_data 
            }
    }
}

impl From<LiqCandidate> for LiqData {
    fn from(value: LiqCandidate) -> Self {
        Self { 
            mode: LiquidationMode::Morpho, 
            borrower: value.borrower, 
            aave_debt_asset: Address::zero(), 
            aave_collateral: Address::zero(), 
            aave_debt_to_cover: U256::zero(), 
            morpho_market_id: value.market_id, 
            morpho_repaid_shares: value.repaid_shares, 
            morpho_seized_assets: value.seized_assets, 
            swap_target: value.swap_target, 
            swap_allowance_target: value.swap_proxy, 
            swap_data: value.swap_data 
        }
    }
    
}