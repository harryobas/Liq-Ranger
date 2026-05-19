use ethers::types::{Address, H256, U256};

use crate::{
    aave::types::LiquidationCandidate, compound::types::BuyCollateralParams,
    morpho::types::LiqCandidate,
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
            compound_collateral: Address::zero(),
            compound_debt_asset: Address::zero(),
            compound_debt_to_cover: U256::zero(),
            compound_min_collateral: U256::zero(),
            swap_target: value.swap_target,
            swap_allowance_target: value.swap_proxy,
            swap_data: value.swap_data,
            flash_asset: value.debt_asset,
            min_amt_out: value.min_amt_out,
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
            compound_collateral: Address::zero(),
            compound_debt_asset: Address::zero(),
            compound_debt_to_cover: U256::zero(),
            compound_min_collateral: U256::zero(),
            swap_target: value.swap_target,
            swap_allowance_target: value.swap_proxy,
            swap_data: value.swap_data,
            flash_asset: value.debt_token,
            min_amt_out: value.min_amt_out,
        }
    }
}

impl From<BuyCollateralParams> for LiquidationParams {
    fn from(value: BuyCollateralParams) -> Self {
        Self {
            mode: 2,
            borrower: Address::zero(),
            aave_debt_asset: Address::zero(),
            aave_collateral: Address::zero(),
            aave_debt_to_cover: U256::zero(),
            morpho_market_id: [0u8; 32],
            morpho_repaid_shares: U256::zero(),
            morpho_seized_assets: U256::zero(),
            compound_collateral: value.collateral_asset,
            compound_debt_asset: value.base_asset,
            compound_debt_to_cover: value.base_amount,
            compound_min_collateral: value.min_collateral,
            swap_target: value.swap_target,
            swap_allowance_target: value.swap_proxy,
            swap_data: value.swap_data,
            flash_asset: value.base_asset,
            min_amt_out: value.min_base_out,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ethers::types::Bytes;

    fn addr(n: u64) -> Address {
        Address::from_low_u64_be(n)
    }

    #[test]
    fn aave_candidate_maps_to_liquidation_params() {
        let candidate = LiquidationCandidate {
            debt_to_cover: U256::from(1_000u64),
            debt_asset: addr(1),
            collateral_asset: addr(2),
            borrower: addr(3),
            swap_target: addr(4),
            swap_proxy: addr(5),
            swap_data: Bytes::from_static(&[0xaa, 0xbb]),
            min_amt_out: U256::from(950u64),
        };

        let params = LiquidationParams::from(candidate);

        assert_eq!(params.mode, 0);
        assert_eq!(params.borrower, addr(3));
        assert_eq!(params.aave_debt_asset, addr(1));
        assert_eq!(params.aave_collateral, addr(2));
        assert_eq!(params.aave_debt_to_cover, U256::from(1_000u64));
        assert_eq!(params.flash_asset, addr(1));
        assert_eq!(params.swap_allowance_target, addr(5));
        assert_eq!(params.min_amt_out, U256::from(950u64));
        assert_eq!(params.morpho_repaid_shares, U256::zero());
        assert_eq!(params.compound_debt_to_cover, U256::zero());
    }

    #[test]
    fn morpho_candidate_maps_mode_specific_fields() {
        let candidate = LiqCandidate {
            debt_to_cover: U256::from(2_000u64),
            borrower: addr(10),
            seized_assets: U256::from(300u64),
            repaid_shares: U256::from(400u64),
            market_id: H256::from_low_u64_be(11),
            debt_token: addr(12),
            collateral_token: addr(13),
            swap_target: addr(14),
            swap_data: Bytes::from_static(&[0xcc]),
            swap_proxy: addr(15),
            min_amt_out: U256::from(1_900u64),
        };

        let params = LiquidationParams::from(candidate);

        assert_eq!(params.mode, 1);
        assert_eq!(params.borrower, addr(10));
        assert_eq!(
            params.morpho_market_id,
            H256::from_low_u64_be(11).to_fixed_bytes()
        );
        assert_eq!(params.morpho_repaid_shares, U256::from(400u64));
        assert_eq!(params.morpho_seized_assets, U256::from(300u64));
        assert_eq!(params.flash_asset, addr(12));
        assert_eq!(params.aave_debt_asset, Address::zero());
        assert_eq!(params.compound_collateral, Address::zero());
    }

    #[test]
    fn compound_buy_collateral_maps_flash_asset_and_limits() {
        let params = BuyCollateralParams {
            collateral_asset: addr(21),
            base_asset: addr(22),
            base_amount: U256::from(5_000u64),
            min_collateral: U256::from(6_000u64),
            swap_target: addr(23),
            swap_proxy: addr(24),
            swap_data: Bytes::from_static(&[0xdd]),
            min_base_out: U256::from(4_900u64),
        };

        let liq = LiquidationParams::from(params);

        assert_eq!(liq.mode, 2);
        assert_eq!(liq.compound_collateral, addr(21));
        assert_eq!(liq.compound_debt_asset, addr(22));
        assert_eq!(liq.compound_debt_to_cover, U256::from(5_000u64));
        assert_eq!(liq.compound_min_collateral, U256::from(6_000u64));
        assert_eq!(liq.flash_asset, addr(22));
        assert_eq!(liq.min_amt_out, U256::from(4_900u64));
        assert_eq!(liq.borrower, Address::zero());
        assert_eq!(liq.morpho_repaid_shares, U256::zero());
    }
}
