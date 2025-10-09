
use ethers::{
    abi::Token,
    providers::Middleware,
    types::{Address, Bytes, U256}
};
use anyhow::{Result, anyhow};

use crate::{
    abi_bindings::{
    ui_pool_data_provider::UserReserveData, 
    AaveOracle, 
    AaveV3Pool, 
    Dex, 
    UiPoolDataProvider, 
    IERC20
},  config::aave_config::AaveConfig, 
    constants, 
    models::{borrow::BorrowsData, liquidation::LiquidationCandidate}, 
    watch_list::{
        aave_watch_list::AaveWatchList,
        WatchList
    }
};

use std::sync::Arc;

pub async fn get_liquidation_bonus<M: Middleware + 'static>(
    asset: Address, 
    pool: &AaveV3Pool<M>) -> Result<u16> {
        let lb_mask: U256 = U256::from(0xFFFF);
        let config_data: U256 = pool.get_configuration(asset).call().await?;
        let shifted = config_data >> 32;
        let lb = (shifted & lb_mask).as_u32() as u16;

        Ok(lb)
}

pub async fn get_estimated_collateral_amt<M: Middleware + 'static>(
    debt_to_cover: U256,           // in debt token units
    collateral_asset: Address,
    debt_asset: Address,
    liquidation_bonus_bps: U256,   // e.g. 11000 = +10%
    oracle: &AaveOracle<M>,
    client: Arc<M>
) -> Result<U256> {
    // --- fetch prices from Aave oracle (1e18 scaled) ---
    let collateral_price: U256 = oracle
        .get_asset_price(collateral_asset)
        .call()
        .await?;
    let debt_price: U256 = oracle
        .get_asset_price(debt_asset)
        .call()
        .await?;

    // --- fetch token decimals ---
    let debt_decimals: u8 = IERC20::new(debt_asset, client.clone()).decimals().call().await?;
    let collateral_decimals: u8 = IERC20::new(collateral_asset, client).decimals().call().await?;

    // --- numerator = debt_to_cover * debt_price * liquidation_bonus_bps ---
    let mut numerator = debt_to_cover
        .checked_mul(debt_price)
        .ok_or_else(|| anyhow!("Overflow in debt * price"))?;

    numerator = numerator
        .checked_mul(liquidation_bonus_bps)
        .ok_or_else(|| anyhow!("Overflow in debt * price * LB"))?;

    // --- adjust for token decimals ---
    numerator = numerator
        .checked_mul(U256::exp10(collateral_decimals as usize))
        .ok_or_else(|| anyhow!("Overflow multiplying collateral decimals"))?;

    let denominator = collateral_price
        .checked_mul(U256::from(10_000)).ok_or_else(|| anyhow!("Multiplication error"))? // bps scaling
        .checked_mul(U256::exp10(debt_decimals as usize))
        .ok_or_else(|| anyhow!("Overflow in denominator"))?;

    let estimated_collateral = numerator
        .checked_div(denominator)
        .ok_or_else(|| anyhow!("Division error in collateral calculation"))?;

    Ok(estimated_collateral)
}



pub async fn simulate_swap_on_dex<M: Middleware + 'static>(
    collateral_amt: U256,
    debt_asset: Address,
    collateral_asset: Address,
    dex: &Dex<M>,
    slippage_bps: U256
) -> Result<U256> {
    if slippage_bps > U256::from(1000) {
        return Err(anyhow!("Slippage too high (>10%)"));
    }

    let path = vec![collateral_asset, debt_asset];
    let amounts_out: Vec<U256> = dex.get_amounts_out(collateral_amt, path)
        .call()
        .await?;

     let min_amount_out = amounts_out[1]
        .checked_mul(U256::from(10_000).checked_sub(slippage_bps).unwrap())
        .and_then(|v| v.checked_div(U256::from(10_000)))
        .ok_or_else(|| anyhow!("Slippage calculation error"))?;

    Ok(min_amount_out)

}

pub async fn get_debt_to_cover<M: Middleware + 'static>(
    vdebt_token: Address, 
    borrower: Address,
    client: Arc<M>,
    health_factor: U256
) -> Result<U256> {
    let vdebt = IERC20::new(vdebt_token, client);
    let balance: U256 = vdebt.balance_of(borrower).call().await?;

    let hf_095 = U256::exp10(18) * 95 / 100; // 0.95 * 1e18
    let close_factor_bps = if health_factor < hf_095 { 10_000 } else { 5_000 };

    let debt_to_cover = balance
        .checked_mul(U256::from(close_factor_bps))
        .and_then(|v| v.checked_div(U256::from(10_000)))
        .ok_or_else(|| anyhow!("Error computing debt_to_cover"))?;

    Ok(debt_to_cover)
}

pub async fn select_collateral<M: Middleware + 'static>(
    account: Address,
    pool: &AaveV3Pool<M>,
    usr_pool_data: &UiPoolDataProvider<M>,
    oracle: &AaveOracle<M>,       // to get prices
    debt_asset: Address,
    debt_to_cover: U256,
    client: Arc<M>,
    config: &AaveConfig
) -> Result<(Address, u16, U256)> {
    let provider = config.pool_address_provider;
    let (user_reserves, _): (Vec<UserReserveData>, u8) =
        usr_pool_data
            .get_user_reserves_data(provider, account)
            .call()
            .await?;

    let mut best_collateral: Option<(Address, u16, U256)> = None;

    for reserve in user_reserves {
        // only consider if user enabled as collateral + has nonzero balance
        if reserve.usage_as_collateral_enabled_on_user && reserve.scaled_a_token_balance > U256::zero() {
            let lb = get_liquidation_bonus(reserve.underlying_asset, pool).await?;

            // estimate seizeable collateral amount
            let estimated_amt = get_estimated_collateral_amt(
                debt_to_cover,
                reserve.underlying_asset,
                debt_asset,
                U256::from(lb),
                oracle,
                client.clone()
            ).await?;

            // compute USD value: collateral_amount * collateral_price
            let collateral_price = oracle
                .get_asset_price(reserve.underlying_asset)
                .call()
                .await?;

            let usd_value = estimated_amt
                .checked_mul(collateral_price)
                .ok_or_else(|| anyhow!("Overflow calculating collateral USD value"))?;

            match best_collateral {
                Some((_, _, best_value)) if usd_value > best_value => {
                    // pick collateral giving higher USD value
                    best_collateral = Some((reserve.underlying_asset, lb, usd_value));
                }
                None => {
                    best_collateral = Some((reserve.underlying_asset, lb, usd_value));
                }
                _ => {} // keep current best
            }
        }
    }

    match best_collateral {
        Some(best) => Ok(best),
        None => Err(anyhow!("No collateral found for user")),
    }
}


pub fn is_liquidation_profitable(
    debt_repaid: U256,       // amount of debt covered with flash loan
    min_amount_out: U256     // amount of debt asset recovered after swapping collateral (post-slippage)
) -> bool {
    // liquidation is profitable if recovered > repaid
    min_amount_out > debt_repaid
}

pub async fn aave_bootstrap_from_subgraph(watchlist: &AaveWatchList, config: &AaveConfig) -> Result<()> {
    let borrows_query = serde_json::json!({
            "query": constants::BORROWERS_QUERY_AAVE, 
            "variables": serde_json::json!({})
        });

      let resp = reqwest::Client::new()
            .post(config.subgraph_url.as_str())
            .header("Authorization", &format!("Bearer {}", config.subgraph_api_key.as_str()))
            .json(&borrows_query)
            .send()
            .await?
            .error_for_status()?
            .json::<serde_json::Value>()
            .await?;

    let raw_borrows = resp.get("data")
        .ok_or(anyhow!("failed to get data"))?;

    let raw_borrows: BorrowsData = serde_json::from_value(raw_borrows.clone())?;

    for (account, asset) in raw_borrows.borrows.into_iter().filter_map(|b| {
    let asset = b.asset.id.parse::<Address>().ok()?;
    let account = b.account.id.parse::<Address>().ok()?;
    Some((account, asset))

}) {
    watchlist.add((account, asset)).await?;
}

Ok(())

}

pub fn create_aave_liquidation_calldata(liq: &LiquidationCandidate) -> Result<Bytes> {
    let tokens = vec![
        Token::Address(liq.collateral_asset),
        Token::Address(liq.debt_asset),
        Token::Address(liq.borrower),
        Token::Uint(liq.debt_to_cover),
        Token::Uint(liq.min_amount_out)
    ];
    let encoded = ethers::abi::encode(&tokens);
    Ok(Bytes::from(encoded))
    
}

