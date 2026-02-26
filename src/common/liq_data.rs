use ethers::types::{H256, U256, Address};

use crate::{
    aave::types::LiquidationCandidate,
    morpho::types::LiqCandidate
};

use crate::common::LiquidationParams;

impl From<LiquidationCandidate> for LiquidationParams {
    fn from(value: LiquidationCandidate) -> Self {
        Self {
             mode: 0,
             borrower: value.borrower, 
             aave_debt_asset: value.debt_asset,
             aave_collateral: value.collateral_asset, 
             aave_debt_to_cover: value.debt_to_cover, 
             morpho_market_id: H256::zero().into(), 
             morpho_repaid_shares: U256::zero(), 
             morpho_seized_assets: U256::zero(), 
             swap_target: value.swap_target, 
             swap_allowance_target: value.swap_proxy, 
             swap_data: value.swap_data,
             flash_asset: value.debt_asset,
             min_amt_out: value.min_amt_out

            }
    }
}

impl From<LiqCandidate> for LiquidationParams {
    fn from(value: LiqCandidate) -> Self {
        Self { 
            mode: 1, 
            borrower: value.borrower, 
            aave_debt_asset: Address::zero(), 
            aave_collateral: Address::zero(), 
            aave_debt_to_cover: U256::zero(), 
            morpho_market_id: value.market_id.to_fixed_bytes(), 
            morpho_repaid_shares: value.repaid_shares, 
            morpho_seized_assets: value.seized_assets, 
            swap_target: value.swap_target, 
            swap_allowance_target: value.swap_proxy, 
            swap_data: value.swap_data,
            flash_asset: value.debt_token,
            min_amt_out: value.min_amt_out 
        }
    }
    
}