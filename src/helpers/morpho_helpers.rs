use ethers::{abi::Token, types::{Bytes, U256}};

use crate::{
    abi_bindings::{Market, Position},  
    constants::*, 
    liquidators::morpho_blue_liquidator::MorphoLiquidationCandidate, 
};

pub fn create_morpho_liquidation_calldata(liq: &MorphoLiquidationCandidate) -> anyhow::Result<Bytes> {
    let tokens = vec![
        Token::Uint(liq.debt_to_cover),
        Token::Address(liq.borrower),
        Token::Uint(liq.seized_assets),
        Token::Uint(liq.repaid_shares),
        Token::FixedBytes(liq.market_id.to_fixed_bytes().to_vec())
    ];
    let encoded = ethers::abi::encode(&tokens);
    Ok(Bytes::from(encoded))
    
}

pub fn position_is_healthy(
    position: &Position,
    market: &Market,
    lltv: U256,
    price: U256,
) -> anyhow::Result<(bool, U256, U256)> {
    
    // --- Calculate collateral value in loan token terms ---
    let collateral_value = U256::from(position.collateral)
        .checked_mul(price)
        .ok_or_else(|| anyhow::anyhow!("overflow in collateral_value multiplication"))?
        .checked_div(U256::exp10(PRICE_DECIMALS))
        .ok_or_else(|| anyhow::anyhow!("division error in collateral_value"))?;

    // --- Calculate maximum allowed borrow ---
    let max_borrow = collateral_value
        .checked_mul(lltv)
        .ok_or_else(|| anyhow::anyhow!("overflow in max_borrow multiplication"))?
        .checked_div(U256::exp10(RATIO_DECIMALS))
        .ok_or_else(|| anyhow::anyhow!("division error in max_borrow"))?;

    // --- Calculate actual borrowed assets using virtual values for share conversion ---
    let borrowed_assets = U256::from(position.borrow_shares)
        .checked_mul(U256::from(market.total_borrow_assets).checked_add(U256::from(VIRTUAL_ASSETS))
            .ok_or_else(|| anyhow::anyhow!("overflow in total_borrow_assets with virtual assets"))?)
        .ok_or_else(|| anyhow::anyhow!("overflow in borrowed_assets multiplication"))?
        .checked_div(U256::from(market.total_borrow_shares).checked_add(U256::from(VIRTUAL_SHARES))
            .ok_or_else(|| anyhow::anyhow!("overflow in total_borrow_shares with virtual shares"))?)
        .ok_or_else(|| anyhow::anyhow!("division error in borrowed_assets"))?;

    // --- Check position health ---
    let is_liquidatable = borrowed_assets >= max_borrow;
    if !is_liquidatable {
        return Ok((false, U256::zero(), U256::zero()));
    }

    // --- Liquidate 100% of the debt ---
    let debt_to_cover = borrowed_assets;

    // --- Convert debt amount back to shares using virtual values ---
    let repaid_shares = debt_to_cover
        .checked_mul(U256::from(market.total_borrow_shares).checked_add(U256::from(VIRTUAL_SHARES))
            .ok_or_else(|| anyhow::anyhow!("overflow in total_borrow_shares with virtual shares"))?)
        .ok_or_else(|| anyhow::anyhow!("overflow in repaid_shares multiplication"))?
        .checked_div(U256::from(market.total_borrow_assets).checked_add(U256::from(VIRTUAL_ASSETS))
            .ok_or_else(|| anyhow::anyhow!("overflow in total_borrow_assets with virtual assets"))?)
        .ok_or_else(|| anyhow::anyhow!("division error in repaid_shares"))?;

      Ok((true, debt_to_cover, repaid_shares))
    }

