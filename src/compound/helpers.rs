use super::abi_bindings::IComet;
use ethers::{types::{Address, U256, U512}, providers::Middleware};

use anyhow::{anyhow,ensure};


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

    let factor_scale = U256::exp10(18);

    ensure!(!price_base.is_zero(), "Base price zero");

    // ---------- discountFactor ----------
    // discountFactor = SFPF * (1e18 - LF) / 1e18
    let one_minus_liq = factor_scale
        .checked_sub(U256::from(liq_factor))
        .ok_or_else(|| anyhow!("liq_factor > 1e18"))?;
    

    let discount_factor = sfpf
        .checked_mul(one_minus_liq)
        .ok_or_else(|| anyhow!("multiplication overflow"))?
        / factor_scale;

    // effective multiplier applied to asset price
    let effective_multiplier = factor_scale
        .checked_sub(discount_factor)
        .ok_or_else(|| anyhow!("subtraction overflow"))?;

    // ---------- Inverted formula ----------
    //
    // base =
    // collateral
    // × assetPrice
    // × effective_multiplier
    // × baseScale
    // /
    // (basePrice × assetScale × 1e18)

    let numerator = desired_collateral
    .full_mul(price_asset)
    * U512::from(effective_multiplier)
    * U512::from(base_scale);

    let denominator = price_base
    .full_mul(asset_scale)
    * U512::from(factor_scale);

    let quotient = numerator / denominator;

    // Overflow check (must fit in 256 bits)
    ensure!(
        quotient >> 256 == U512::zero(),
       "Overflow in base calculation"
    );

    let mut buf = [0u8; 64];
    quotient.to_big_endian(&mut buf);

    // Lower 32 bytes = U256
    let mut base_required = U256::from_big_endian(&buf[32..]);

    // ---------- Rounding correction ----------
    let actual = comet
        .quote_collateral(asset, base_required)
        .call()
        .await?;

    if actual < desired_collateral {
        base_required += U256::one();
        let actual2 = comet
            .quote_collateral(asset, base_required)
            .call()
            .await?;
        ensure!(actual2 >= desired_collateral, "Rounding adjustment failed");
    }

    Ok(base_required.min(max_base_cap))
}

