use ethers::types::U256;
use crate::constants::{
    WAD,
    VIRTUAL_ASSETS,
    VIRTUAL_SHARES,
    MAX_LIQUIDATION_INCENTIVE_FACTOR,
    LIQUIDATION_CURSOR,
};

#[inline]
pub fn min(a: U256, b: U256) -> U256 {
    if a < b { a } else { b }
}

#[inline]
pub fn max(a: U256, b: U256) -> U256 {
    if a > b { a } else { b }
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
    (n + d - U256::one())
        .checked_div(d)
        .expect("div by zero")
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
pub fn incentive_factor(lltv: U256) -> U256 {
    min(
        *MAX_LIQUIDATION_INCENTIVE_FACTOR,
        wdiv_down(
            *WAD,
            *WAD - wmul_down(*LIQUIDATION_CURSOR, *WAD - lltv),
        ),
    )
}
