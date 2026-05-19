use ethers::{
    providers::Middleware,
    types::{Address, H256},
};

use std::sync::Arc;

use super::{abi_bindings::IMorphoBlue, morpho_config::MorphoConfig};

use crate::common::abi_bindings::IFlashLiquidator;

pub fn fetch_contracts<M: Middleware>(
    client: Arc<M>,
    config: Arc<MorphoConfig>,
) -> (IMorphoBlue<M>, IFlashLiquidator<M>) {
    let morpho = IMorphoBlue::new(config.morpho_blue, client.clone());
    let flash_liq = IFlashLiquidator::new(config.flash_liquidator, client.clone());

    (morpho, flash_liq)
}

pub async fn has_outstanding_debt<M: Middleware + 'static>(
    morpho: Arc<IMorphoBlue<M>>,
    borrower: Address,
    market: H256,
) -> anyhow::Result<bool> {
    let market = market.to_fixed_bytes();
    let (_supply_shares, borrow_shares, _collateral) =
        morpho.position(market, borrower).call().await?;

    Ok(borrow_shares != 0)
}
