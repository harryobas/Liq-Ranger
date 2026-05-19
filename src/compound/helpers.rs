use super::abi_bindings::IComet;
use ethers::{
    providers::Middleware,
    types::{Address, U256, U512},
};

use anyhow::{anyhow, ensure};

fn discounted_base_amount(
    desired_collateral: U256,
    price_asset: U256,
    price_base: U256,
    store_front_price_factor: U256,
    base_scale: U256,
    liquidation_factor: u128,
    asset_scale: U256,
) -> anyhow::Result<U256> {
    let factor_scale = U256::exp10(18);

    ensure!(!price_base.is_zero(), "Base price zero");

    let one_minus_liq = factor_scale
        .checked_sub(U256::from(liquidation_factor))
        .ok_or_else(|| anyhow!("liq_factor > 1e18"))?;

    let discount_factor = store_front_price_factor
        .checked_mul(one_minus_liq)
        .ok_or_else(|| anyhow!("multiplication overflow"))?
        / factor_scale;

    let effective_multiplier = factor_scale
        .checked_sub(discount_factor)
        .ok_or_else(|| anyhow!("subtraction overflow"))?;

    let numerator = desired_collateral.full_mul(price_asset)
        * U512::from(effective_multiplier)
        * U512::from(base_scale);

    let denominator = price_base.full_mul(asset_scale) * U512::from(factor_scale);

    let quotient = numerator / denominator;

    ensure!(
        quotient >> 256 == U512::zero(),
        "Overflow in base calculation"
    );

    let mut buf = [0u8; 64];
    quotient.to_big_endian(&mut buf);

    Ok(U256::from_big_endian(&buf[32..]))
}

pub async fn base_amount_for_collateral<M: Middleware + 'static>(
    comet: &IComet<M>,
    asset: Address,
    desired_collateral: U256,
    max_base_cap: U256,
) -> anyhow::Result<U256> {
    if desired_collateral.is_zero() || max_base_cap.is_zero() {
        return Ok(U256::zero());
    }

    // ---------- Fetch protocol data ----------
    let price_asset = comet.get_price(asset).call().await?;
    let base_token = comet.base_token().call().await?;
    let price_base = comet.get_price(base_token).call().await?;
    let sfpf = comet.store_front_price_factor().call().await?;
    let base_scale = U256::from(comet.base_scale().call().await?);

    let asset_info = comet.get_asset_info_by_address(asset).call().await?;
    let liq_factor = asset_info.liquidation_factor;
    let asset_scale = U256::from(asset_info.scale);

    // ---------- Inverted formula ----------
    //
    // base =
    // collateral
    // × assetPrice
    // × effective_multiplier
    // × baseScale
    // /
    // (basePrice × assetScale × 1e18)

    let mut base_required = discounted_base_amount(
        desired_collateral,
        price_asset,
        price_base,
        sfpf,
        base_scale,
        liq_factor.into(),
        asset_scale,
    )?;

    // ---------- Rounding correction ----------
    let actual = comet.quote_collateral(asset, base_required).call().await?;

    if actual < desired_collateral {
        base_required += U256::one();
        let actual2 = comet.quote_collateral(asset, base_required).call().await?;
        ensure!(actual2 >= desired_collateral, "Rounding adjustment failed");
    }

    Ok(base_required.min(max_base_cap))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn discounted_base_amount_rejects_zero_base_price() {
        let err = discounted_base_amount(
            U256::one(),
            U256::one(),
            U256::zero(),
            U256::one(),
            U256::one(),
            0,
            U256::one(),
        )
        .expect_err("zero base price");

        assert!(err.to_string().contains("Base price zero"));
    }

    #[test]
    fn discounted_base_amount_rejects_liquidation_factor_above_one() {
        let err = discounted_base_amount(
            U256::one(),
            U256::one(),
            U256::one(),
            U256::one(),
            U256::one(),
            1_000_000_000_000_000_001,
            U256::one(),
        )
        .expect_err("invalid liquidation factor");

        assert!(err.to_string().contains("liq_factor > 1e18"));
    }

    #[test]
    fn discounted_base_amount_applies_discount_formula() {
        let amount = discounted_base_amount(
            U256::from(100u64),
            U256::from(2u64),
            U256::from(1u64),
            U256::from(100_000_000_000_000_000u64),
            U256::one(),
            800_000_000_000_000_000,
            U256::one(),
        )
        .expect("formula");

        assert_eq!(amount, U256::from(196u64));
    }
}
