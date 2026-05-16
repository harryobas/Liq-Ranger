use anyhow::{anyhow, ensure, Result};
use ethers::{
    providers::Middleware,
    types::{Address, U256},
};
use std::sync::Arc;

use super::{
    aave_config::AaveConfig,
    abi_bindings::{AaveOracle, IAaveV3Pool, UserReserveData},
    types::CollateralCandidate,
};

use crate::{
    common::{abi_bindings::IERC20, get_token_decimals},
    constants::{ATOKENS_ADDR, HF_LIQUIDATION_THRESHOLD_BPS, WAD},
};

const BPS: u128 = 10_000;

//
// ─────────────────────────────────────────────────────────────
// Liquidation Bonus
// ─────────────────────────────────────────────────────────────
//

pub async fn liquidation_bonus_bps<M: Middleware + 'static>(
    asset: Address,
    pool: &IAaveV3Pool<M>,
) -> Result<u16> {
    let config = pool.get_configuration(asset).call().await?;
    let raw_bonus = ((config.data >> 32) & U256::from(0xFFFF)).as_u32() as u16;

    ensure!(
        raw_bonus >= 10_000 && raw_bonus <= 20_000,
        "invalid liquidation bonus"
    );

    Ok(raw_bonus)
}

//
// ─────────────────────────────────────────────────────────────
// Debt To Cover
// ─────────────────────────────────────────────────────────────
//

pub async fn compute_debt_to_cover<M: Middleware + 'static>(
    borrower: Address,
    vdebt_token: Address,
    health_factor: U256,
    client: Arc<M>,
) -> Result<U256> {
    let debt = IERC20::new(vdebt_token, client)
        .balance_of(borrower)
        .call()
        .await?;

    if debt.is_zero() {
        return Ok(U256::zero());
    }

    let threshold = U256::from(HF_LIQUIDATION_THRESHOLD_BPS) * *WAD / U256::from(BPS);

    let close_factor = if health_factor < threshold {
        BPS
    } else {
        BPS / 2
    };

    Ok(debt * U256::from(close_factor) / U256::from(BPS))
}

//
// ─────────────────────────────────────────────────────────────
// Seizable Collateral Estimate
// ─────────────────────────────────────────────────────────────
//

pub async fn estimate_seizable_collateral<M: Middleware + 'static>(
    debt_to_cover: U256,
    collateral_asset: Address,
    debt_asset: Address,
    liquidation_bonus_bps: u16,
    oracle: &AaveOracle<M>,
    client: Arc<M>,
) -> Result<U256> {
    if debt_to_cover.is_zero() {
        return Ok(U256::zero());
    }

    let collateral_price_call = oracle.get_asset_price(collateral_asset);
    let debt_price_call = oracle.get_asset_price(debt_asset);

    let (collateral_price, debt_price) =
        tokio::try_join!(collateral_price_call.call(), debt_price_call.call(),)?;

    let (coll_decimals, debt_decimals) = tokio::try_join!(
        get_token_decimals(collateral_asset, client.clone()),
        get_token_decimals(debt_asset, client.clone()),
    )?;

    let numerator = debt_to_cover
        .checked_mul(debt_price)
        .ok_or(anyhow!("overflow: debt * price"))?
        .checked_mul(U256::exp10(coll_decimals as usize))
        .ok_or(anyhow!("overflow: collateral decimals"))?
        .checked_mul(U256::from(liquidation_bonus_bps))
        .ok_or(anyhow!("overflow: bonus"))?;

    let denominator = collateral_price
        .checked_mul(U256::from(BPS))
        .ok_or(anyhow!("overflow: price * bps"))?
        .checked_mul(U256::exp10(debt_decimals as usize))
        .ok_or(anyhow!("overflow: debt decimals"))?;

    Ok(numerator / denominator)
}

//
// ─────────────────────────────────────────────────────────────
// Select Collateral Candidates
// ─────────────────────────────────────────────────────────────
//

pub async fn select_collateral_candidates<M: Middleware + 'static>(
    borrower: Address,
    collaterals: &[&UserReserveData],
    debt_asset: Address,
    debt_to_cover: U256,
    pool: &IAaveV3Pool<M>,
    oracle: &AaveOracle<M>,
    client: Arc<M>,
) -> Result<Vec<CollateralCandidate>> {
    let mut candidates = Vec::new();

    for reserve in collaterals {
        let asset = reserve.underlying_asset;

        let bonus = liquidation_bonus_bps(asset, pool).await?;

        let atoken = resolve_atoken(pool, asset).await?;

        let balance = IERC20::new(atoken, client.clone())
            .balance_of(borrower)
            .call()
            .await?;

        let seize = estimate_seizable_collateral(
            debt_to_cover,
            asset,
            debt_asset,
            bonus,
            oracle,
            client.clone(),
        )
        .await?
        .min(balance);

        if seize.is_zero() {
            continue;
        }

        let price = oracle.get_asset_price(asset).call().await?;
        let decimals = get_token_decimals(asset, client.clone()).await?;
        let usd_value = seize
            .checked_mul(price)
            .ok_or_else(|| anyhow!("overflow: collateral amount * price"))?
            / U256::exp10(decimals as usize);

        candidates.push(CollateralCandidate {
            asset,
            liquidation_bonus_bps: bonus,
            seize_amount: seize,
            usd_value,
        });
    }

    candidates.sort_by(|a, b| b.usd_value.cmp(&a.usd_value));

    if candidates.is_empty() {
        return Err(anyhow!("no viable collateral"));
    }

    Ok(candidates)
}

//
// ─────────────────────────────────────────────────────────────
// Resolve aToken
// ─────────────────────────────────────────────────────────────
//

async fn resolve_atoken<M: Middleware + 'static>(
    pool: &IAaveV3Pool<M>,
    asset: Address,
) -> Result<Address> {
    if let Some(addr) = ATOKENS_ADDR.get(&asset) {
        return Ok(*addr);
    }

    let data = pool.get_reserve_data(asset).call().await?;

    ATOKENS_ADDR.insert(asset, data.a_token_address);

    Ok(data.a_token_address)
}

//
// ─────────────────────────────────────────────────────────────
// Check Outstanding Debt
// ─────────────────────────────────────────────────────────────
//

pub async fn has_outstanding_debt<M: Middleware + 'static>(
    borrower: Address,
    reserve: Address,
    pool: &IAaveV3Pool<M>,
    config: &AaveConfig,
) -> Result<bool> {
    let vdebt = config
        .vdebt_tokens
        .get(&reserve)
        .ok_or_else(|| anyhow!("missing vDebt token"))?;

    let token = IERC20::new(*vdebt, pool.client());

    let debt = token.balance_of(borrower).call().await?;

    Ok(!debt.is_zero())
}
