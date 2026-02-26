use anyhow::{Result, anyhow};
use ethers::types::{Address, U256};
use ethers::providers::Middleware;
use std::sync::Arc;

use super::{
    abi_bindings::{
        AaveOracle,
        IAaveV3Pool,
        i_aave_v3_pool,
        UiPoolDataProvider,
        ui_pool_data_provider::UserReserveData,
    },
    aave_config::AaveConfig,
    types::CollateralCandidate,
};

use crate::{
    common::{abi_bindings::IERC20, get_token_decimals},
    constants::{WAD, ATOKENS_ADDR, HF_LIQUIDATION_THRESHOLD_BPS}
    
};

const BPS: u128 = 10_000;


//
// ─────────────────────────────────────────────────────────────
//  Liquidation Bonus
// ─────────────────────────────────────────────────────────────
//

/// Reads liquidation bonus (bps) from Aave V3 reserve config
pub async fn liquidation_bonus_bps<M: Middleware + 'static>(
    asset: Address,
    pool: &IAaveV3Pool<M>,
) -> Result<u16> {
    let config: U256 = pool.get_configuration(asset).call().await?;
    let bonus = ((config >> 32) & U256::from(0xFFFF)).as_u32() as u16;
    Ok(bonus)
}

//
// ─────────────────────────────────────────────────────────────
//  Debt To Cover (Close Factor)
// ─────────────────────────────────────────────────────────────
//

/// Computes debtToCover following Aave V3 close factor rules
pub async fn compute_debt<M: Middleware + 'static>(
    vdebt_token: Address,
    borrower: Address,
    health_factor: U256,
    client: Arc<M>,
) -> Result<U256> {
    let debt_balance = IERC20::new(vdebt_token, client)
        .balance_of(borrower)
        .call()
        .await?;

    if debt_balance.is_zero() {
        return Ok(U256::zero());
    }

    let hf_threshold =
        U256::from(HF_LIQUIDATION_THRESHOLD_BPS) * *WAD / U256::from(BPS);

    let close_factor_bps = if health_factor < hf_threshold {
        BPS // 100%
    } else {
        BPS / 2 // 50%
    };

    Ok(debt_balance * U256::from(close_factor_bps) / U256::from(BPS))
}

//
// ─────────────────────────────────────────────────────────────
//  Max Seizable Collateral (Upper Bound)
// ─────────────────────────────────────────────────────────────
//

/// Estimates max collateral Aave *may* seize (upper bound)
pub async fn estimate_seizable_collateral<M: Middleware +  'static>(
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

    let collateral_price = oracle.get_asset_price(collateral_asset).call().await?;
    let debt_price = oracle.get_asset_price(debt_asset).call().await?;

    let (collateral_decimals, debt_decimals) = tokio::try_join!(
        get_token_decimals(collateral_asset, client.clone()),
        get_token_decimals(debt_asset, client.clone())

    )?;

    let numerator = debt_to_cover
        .checked_mul(debt_price).ok_or(anyhow!("overflow: debt * price"))?
        .checked_mul(U256::from(liquidation_bonus_bps)).ok_or(anyhow!("overflow: bonus"))?
        .checked_mul(U256::exp10(collateral_decimals as usize))
        .ok_or(anyhow!("overflow: collateral decimals"))?;

    let denominator = collateral_price
        .checked_mul(U256::from(BPS)).ok_or(anyhow!("overflow: price * bps"))?
        .checked_mul(U256::exp10(debt_decimals as usize))
        .ok_or(anyhow!("overflow: debt decimals"))?;

    Ok(numerator / denominator)
}

//
// ─────────────────────────────────────────────────────────────
//  Collateral Selection
// ─────────────────────────────────────────────────────────────
//

/// Selects best collateral based on max USD seizeable value
pub async fn select_best_collateral<M: Middleware + 'static>(
    borrower: Address,
    pool: &IAaveV3Pool<M>,
    ui_provider: &UiPoolDataProvider<M>,
    oracle: &AaveOracle<M>,
    debt_asset: Address,
    debt_to_cover: U256,
    client: Arc<M>,
    config: &AaveConfig,
) -> Result<CollateralCandidate> {
    let (reserves, _): (Vec<UserReserveData>, _) =
        ui_provider
            .get_user_reserves_data(config.pool_address_provider, borrower)
            .call()
            .await?;

    let mut best: Option<CollateralCandidate> = None;

    for r in reserves {
        if !r.usage_as_collateral_enabled_on_user || r.scaled_a_token_balance.is_zero() {
            continue;
        }

        let lb = liquidation_bonus_bps(r.underlying_asset, pool).await?;
        let atoken = resolve_atoken(pool, r.underlying_asset).await?;

        let balance = IERC20::new(atoken, client.clone())
            .balance_of(borrower)
            .call()
            .await?;

        let mut seize_estimate = estimate_seizable_collateral(
            debt_to_cover,
            r.underlying_asset,
            debt_asset,
            lb,
            oracle,
            client.clone(),
        ).await?;

        seize_estimate = seize_estimate.min(balance);
        if seize_estimate.is_zero() {
            continue;
        }

        let price = oracle.get_asset_price(r.underlying_asset).call().await?;
        let usd_value = seize_estimate * price;

        let candidate = CollateralCandidate {
            asset: r.underlying_asset,
            liquidation_bonus_bps: lb,
            seize_amount: seize_estimate,
            usd_value,
        };

        if best.as_ref().map_or(true, |b| candidate.usd_value > b.usd_value) {
            best = Some(candidate);
        }
    }

    best.ok_or_else(|| anyhow!("no viable collateral"))
}

//
// ─────────────────────────────────────────────────────────────
//  aToken Resolution
// ─────────────────────────────────────────────────────────────
//

async fn resolve_atoken<M: Middleware + 'static>(
    pool: &IAaveV3Pool<M>,
    asset: Address,
) -> Result<Address> {
    if let Some(addr) = ATOKENS_ADDR.get(&asset) {
        return Ok(*addr);
    }

    let data: i_aave_v3_pool::ReserveData = pool.get_reserve_data(asset).call().await?;
    ATOKENS_ADDR.insert(asset, data.a_token_address);
    Ok(data.a_token_address)
}

 pub async fn has_outstanding_debt<M: Middleware + 'static>(
        borrower: Address,
        reserve: Address,
        pool: &IAaveV3Pool<M>,
        config: &AaveConfig
    ) -> anyhow::Result<bool> {
        if let Some(vdebt) = config.vdebt_tokens.get(&reserve) {
            // create token binding and call balance_of on the borrower
            let token = IERC20::new(*vdebt, pool.client());
            let debt: U256 = token.balance_of(borrower).call().await?;
            Ok(!debt.is_zero())
        } else {
            tracing::warn!("No vDebtToken configured for reserve {:?}", reserve);
            Ok(false)
        }
    }