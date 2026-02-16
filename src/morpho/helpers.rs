
use ethers::{
    abi::{Token, encode}, 
    providers::Middleware,
    types::{Bytes, H256, U256, Address}
};

use std::sync::Arc;

use super::{
    morpho_config::MorphoConfig,
    abi_bindings::IMorphoBlue
};

use crate::common::{abi_bindings::IFlashLiquidator, liq_data::LiqData};


pub fn encode_liq_data(liq: &LiqData) -> Bytes {
     let tokens = vec![
        // enum → uint8
        Token::Uint(U256::from(liq.mode.clone() as u8)),

        // common
        Token::Address(liq.borrower),

        // aave
        Token::Address(liq.aave_debt_asset),
        Token::Address(liq.aave_collateral),
        Token::Uint(liq.aave_debt_to_cover),
        //Token::Uint(liq.aave_min_amount_out),

        // morpho
        Token::FixedBytes(liq.morpho_market_id.as_fixed_bytes().to_vec()),
        Token::Uint(liq.morpho_repaid_shares),
        Token::Uint(liq.morpho_seized_assets),

        // swap
        Token::Address(liq.swap_target),
        Token::Address(liq.swap_allowance_target),
        Token::Bytes(liq.swap_data.to_vec()),
    ];

    Bytes::from(encode(&tokens))
}


pub fn fetch_contracts<M:Middleware>(
    client: Arc<M>, 
    config: Arc<MorphoConfig>
) -> (IMorphoBlue<M>, IFlashLiquidator<M>) {
    let morpho = IMorphoBlue::new(config.morpho_blue, client.clone());
    let flash_liq = IFlashLiquidator::new(config.flash_liquidator, client.clone());

    (morpho, flash_liq)
}

pub async fn has_outstanding_debt<M: Middleware + 'static>(
    morpho: Arc<IMorphoBlue<M>>,
    borrower: Address,
    market: H256
) -> anyhow::Result<bool>{
    let market = market.to_fixed_bytes();
    let (_supply_shares, borrow_shares, _collateral) =
        morpho.position(market, borrower).call().await?;

    Ok(borrow_shares != 0)
}