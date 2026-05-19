use super::morpho_math::to_assets_up;
use crate::constants::{ORACLE_PRICE_SCALE, WAD};
use ethers::types::{Address, Bytes, H256, U256};

pub trait HealthCheck {
    fn is_healthy(&self, market: &Market, lltv: &U256, price: &U256) -> bool;
}

#[derive(Debug)]
pub enum LiquidationMode {
    /// Healthy collateral → repay shares
    RepayShares {
        repaid_shares: U256,
        /// Conservative on-chain seized collateral (rounded down), used for swap sizing.
        expected_seized_assets: U256,
    },

    /// Insufficient collateral → seize all collateral
    SeizeCollateral { seized_assets: U256 },
}

pub struct LiqCandidate {
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
    pub min_amt_out: U256,
}

pub struct Market {
    pub total_borrow_assets: u128,
    pub total_borrow_shares: u128,
}

pub struct Position {
    pub borrow_shares: u128,
    pub collateral: u128,
}

impl HealthCheck for Position {
    fn is_healthy(&self, market: &Market, lltv: &U256, price: &U256) -> bool {
        // collateral value in loan asset units
        // collateral * price / 1e36
        let collateral_value = U256::from(self.collateral) * *price / *ORACLE_PRICE_SCALE;

        // max borrow = collateral_value * lltv / 1e18
        let max_borrow = collateral_value * *lltv / *WAD;

        // Morpho _isHealthy uses toAssetsUp for borrowed (rounds in favor of the protocol).
        let borrowed_assets = to_assets_up(
            U256::from(self.borrow_shares),
            U256::from(market.total_borrow_assets),
            U256::from(market.total_borrow_shares),
        );

        max_borrow >= borrowed_assets
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn half_wad() -> U256 {
        *WAD / U256::from(2u64)
    }

    #[test]
    fn position_is_healthy_when_max_borrow_covers_debt_up() {
        let market = Market {
            total_borrow_assets: 100,
            total_borrow_shares: 1_000_000,
        };
        // to_assets_up(990_099) = 50; max_borrow at 50% LLTV with 100 collateral = 50
        let position = Position {
            borrow_shares: 990_099,
            collateral: 100,
        };

        assert!(position.is_healthy(&market, &half_wad(), &ORACLE_PRICE_SCALE));
    }

    #[test]
    fn position_is_unhealthy_at_boundary_with_up_rounding() {
        let market = Market {
            total_borrow_assets: 100,
            total_borrow_shares: 1_000_000,
        };
        // to_assets_up(1_000_000) = 51; max_borrow = 50 → unhealthy (matches Morpho)
        let position = Position {
            borrow_shares: 1_000_000,
            collateral: 100,
        };

        assert!(!position.is_healthy(&market, &half_wad(), &ORACLE_PRICE_SCALE));
    }

    #[test]
    fn position_is_unhealthy_when_borrow_exceeds_lltv() {
        let market = Market {
            total_borrow_assets: 100,
            total_borrow_shares: 1_000_000,
        };
        let position = Position {
            borrow_shares: 1_020_000,
            collateral: 100,
        };

        assert!(!position.is_healthy(&market, &half_wad(), &ORACLE_PRICE_SCALE));
    }

    #[test]
    fn health_check_respects_virtual_share_conversion() {
        let market = Market {
            total_borrow_assets: 100,
            total_borrow_shares: 1_000_000,
        };
        let position = Position {
            borrow_shares: 2_000_000,
            collateral: 100,
        };

        // Virtual offsets make the borrowed assets 101, so 100 collateral at
        // 100% LLTV is still unhealthy.
        assert!(!position.is_healthy(&market, &WAD, &ORACLE_PRICE_SCALE));
    }
}
