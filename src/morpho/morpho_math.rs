use crate::constants::{
    LIQUIDATION_CURSOR, MAX_LIQUIDATION_INCENTIVE_FACTOR, VIRTUAL_ASSETS, VIRTUAL_SHARES, WAD,
};
use ethers::types::U256;

#[inline]
pub fn min(a: U256, b: U256) -> U256 {
    if a < b {
        a
    } else {
        b
    }
}

#[inline]
pub fn max(a: U256, b: U256) -> U256 {
    if a > b {
        a
    } else {
        b
    }
}

#[inline]
pub fn mul_div_down(x: U256, y: U256, d: U256) -> U256 {
    x.checked_mul(y)
        .expect("mul overflow")
        .checked_div(d)
        .expect("div by zero")
}

#[inline]
pub fn mul_div_up(x: U256, y: U256, d: U256) -> U256 {
    let n = x.checked_mul(y).expect("mul overflow");
    (n + d - U256::one()).checked_div(d).expect("div by zero")
}

#[inline]
pub fn wmul_down(x: U256, y: U256) -> U256 {
    mul_div_down(x, y, *WAD)
}

#[inline]
pub fn wdiv_down(x: U256, y: U256) -> U256 {
    mul_div_down(x, *WAD, y)
}

#[inline]
pub fn wdiv_up(x: U256, y: U256) -> U256 {
    mul_div_up(x, *WAD, y)
}

#[inline]
pub fn to_assets_down(shares: U256, total_assets: U256, total_shares: U256) -> U256 {
    mul_div_down(
        shares,
        total_assets + U256::from(VIRTUAL_ASSETS),
        total_shares + U256::from(VIRTUAL_SHARES),
    )
}

#[inline]
pub fn to_assets_up(shares: U256, total_assets: U256, total_shares: U256) -> U256 {
    mul_div_up(
        shares,
        total_assets + U256::from(VIRTUAL_ASSETS),
        total_shares + U256::from(VIRTUAL_SHARES),
    )
}

#[inline]
pub fn to_shares_down(assets: U256, total_assets: U256, total_shares: U256) -> U256 {
    mul_div_down(
        assets,
        total_shares + U256::from(VIRTUAL_SHARES),
        total_assets + U256::from(VIRTUAL_ASSETS),
    )
}

#[inline]
pub fn to_shares_up(assets: U256, total_assets: U256, total_shares: U256) -> U256 {
    mul_div_up(
        assets,
        total_shares + U256::from(VIRTUAL_SHARES),
        total_assets + U256::from(VIRTUAL_ASSETS),
    )
}

/// Collateral seized when liquidating via `repaid_shares` (Morpho rounds down).
#[inline]
pub fn seized_assets_from_repaid_shares(
    repaid_shares: U256,
    total_borrow_assets: U256,
    total_borrow_shares: U256,
    lltv: U256,
    oracle_price_scale: U256,
    price: U256,
) -> U256 {
    let repaid_assets = to_assets_down(repaid_shares, total_borrow_assets, total_borrow_shares);
    let lif = incentive_factor(lltv);
    mul_div_down(
        wmul_down(repaid_assets, lif),
        oracle_price_scale,
        price,
    )
}

/// Loan assets Morpho pulls when liquidating via `repaid_shares` (rounds up).
#[inline]
pub fn repaid_assets_from_repaid_shares(
    repaid_shares: U256,
    total_borrow_assets: U256,
    total_borrow_shares: U256,
) -> U256 {
    to_assets_up(repaid_shares, total_borrow_assets, total_borrow_shares)
}

/// Upper bound on loan repayment when seizing `seized` collateral (Morpho rounds up).
#[inline]
pub fn repaid_assets_from_seized_collateral(
    seized: U256,
    total_borrow_assets: U256,
    total_borrow_shares: U256,
    lltv: U256,
    oracle_price_scale: U256,
    price: U256,
) -> U256 {
    let quoted = mul_div_up(seized, price, oracle_price_scale);
    let lif = incentive_factor(lltv);
    let repaid_shares = to_shares_up(wdiv_up(quoted, lif), total_borrow_assets, total_borrow_shares);
    to_assets_up(repaid_shares, total_borrow_assets, total_borrow_shares)
}

#[inline]
pub fn incentive_factor(lltv: U256) -> U256 {
    min(
        *MAX_LIQUIDATION_INCENTIVE_FACTOR,
        wdiv_down(*WAD, *WAD - wmul_down(*LIQUIDATION_CURSOR, *WAD - lltv)),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mul_div_rounding_matches_expected_direction() {
        let x = U256::from(10);
        let y = U256::from(10);
        let denominator = U256::from(6);

        assert_eq!(mul_div_down(x, y, denominator), U256::from(16));
        assert_eq!(mul_div_up(x, y, denominator), U256::from(17));
    }

    #[test]
    fn share_asset_conversions_include_virtual_offsets() {
        let shares = U256::from(2_000_000u64);
        let total_assets = U256::from(100u64);
        let total_shares = U256::from(1_000_000u64);

        assert_eq!(
            to_assets_down(shares, total_assets, total_shares),
            U256::from(101u64)
        );

        let assets = U256::from(101u64);
        assert_eq!(
            to_shares_down(assets, total_assets, total_shares),
            U256::from(2_000_000u64)
        );
    }

    #[test]
    fn mul_div_up_exact_division_does_not_round() {
        assert_eq!(
            mul_div_up(U256::from(10), U256::from(10), U256::from(5)),
            U256::from(20)
        );
    }

    #[test]
    #[should_panic(expected = "div by zero")]
    fn mul_div_down_panics_on_zero_denominator() {
        let _ = mul_div_down(U256::from(1), U256::from(1), U256::zero());
    }

    #[test]
    fn to_assets_up_rounds_up() {
        assert_eq!(
            to_assets_up(U256::from(1), U256::from(100), U256::from(1_000_000)),
            U256::from(1)
        );
    }

    #[test]
    fn incentive_factor_caps_at_max() {
        assert_eq!(
            incentive_factor(U256::zero()),
            *MAX_LIQUIDATION_INCENTIVE_FACTOR
        );
    }

    #[test]
    fn repaid_assets_from_shares_rounds_up() {
        let total_assets = U256::from(100u64);
        let total_shares = U256::from(1_000_000u64);
        let shares = U256::from(1_000_000u64);

        assert_eq!(
            repaid_assets_from_repaid_shares(shares, total_assets, total_shares),
            U256::from(51u64)
        );
        assert_eq!(
            to_assets_down(shares, total_assets, total_shares),
            U256::from(50u64)
        );
    }

    #[test]
    fn seized_from_repaid_shares_is_at_most_theoretical() {
        let total_assets = U256::from(100u64);
        let total_shares = U256::from(1_000_000u64);
        let shares = U256::from(2_000_000u64);
        let lltv = U256::from(860_000_000_000_000_000u64);
        let scale = U256::from(10).pow(U256::from(36));
        let price = scale;

        let theoretical = mul_div_down(
            wmul_down(to_assets_down(shares, total_assets, total_shares), incentive_factor(lltv)),
            scale,
            price,
        );
        let seized =
            seized_assets_from_repaid_shares(shares, total_assets, total_shares, lltv, scale, price);

        assert!(seized <= theoretical);
    }
}
