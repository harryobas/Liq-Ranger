use ethers::types::{Address, U256, H256, Bytes};
use crate::{constants::{ORACLE_PRICE_SCALE, WAD}};
use super::morpho_math::to_assets_down;

pub trait HealthCheck {
    fn is_healthy(&self, market: &Market, lltv: &U256, price: &U256) -> bool;
}


#[derive(Debug)]
pub enum LiquidationMode {
    /// Healthy collateral → repay shares
    RepayShares {
        repaid_shares: U256,
        expected_seized_assets: U256, // for swap only
    },

    /// Insufficient collateral → seize all collateral
    SeizeCollateral {
        seized_assets: U256,
    },
}

pub struct LiqCandidate{
    pub debt_to_cover: U256,
    pub borrower: Address,
    pub seized_assets: U256,
    pub repaid_shares: U256,
    pub market_id: H256,
    pub debt_token: Address,
    pub collateral_token: Address,
    pub swap_target: Address,
    pub swap_data: Bytes,
    pub swap_proxy: Address,
    pub min_amt_out: U256
}

pub struct Market {
    pub total_borrow_assets: u128,
    pub total_borrow_shares: u128,
   
}

pub struct Position {
    pub borrow_shares: u128,
    pub collateral: u128
}

impl HealthCheck for Position {
    fn is_healthy(&self, market: &Market, lltv: &U256, price: &U256) -> bool {
        // collateral value in loan asset units
        // collateral * price / 1e36
        let collateral_value = U256::from(self.collateral)
            * *price
            / *ORACLE_PRICE_SCALE;

        // max borrow = collateral_value * lltv / 1e18
        let max_borrow = collateral_value
            * *lltv
            / *WAD;

        // borrowed assets = borrowShares * (totalBorrowAssets + virtual) / (totalBorrowShares + virtual)
        let borrowed_assets = to_assets_down(
            U256::from(self.borrow_shares),
            U256::from(market.total_borrow_assets),
            U256::from(market.total_borrow_shares)
        );

        max_borrow >= borrowed_assets
    }
    
}



