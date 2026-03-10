use std::sync::Arc;
use anyhow::{Result, ensure};
use ethers::{
    providers::Middleware,
    types::{U256, Address},
};
use futures_util::{stream, StreamExt};

use super::{
    types::BuyCollateralParams,
    abi_bindings::IComet,
    compound_watchlist::CompoundWatchList,
    helpers,
    //compound_config::CompoundConfig,
};

use crate::{common::{
    self,
    Liquidator, 
    SwapQueryParams, 
    abi_bindings::{IFlashLiquidator, LiquidationParams}, 
    execute_liq_tx, 
    get_token_decimals, 
    paraswap::ParaSwapClient, 
    simulate_liq_tx
}, constants};

pub struct CompoundLiquidator<M: Middleware + 'static> {
    pub comet: IComet<M>,
    pub flash_liquidator: IFlashLiquidator<M>,
    pub client: Arc<M>,
    pub watch_list: Arc<CompoundWatchList>,
    //pub config: Arc<CompoundConfig>,
}

impl<M: Middleware + 'static> CompoundLiquidator<M> {
    pub fn new(
        //config: Arc<CompoundConfig>,
        client: Arc<M>,
        watch_list: Arc<CompoundWatchList>,
    ) -> Self {
        let contracts = common::fetch_contracts(client.clone()).expect("failed to fetch contracts");

        let comet = contracts.comet;
        let flash_liquidator = contracts.flash_liq;

        Self {
            comet,
            flash_liquidator,
            client,
            watch_list,
        }
    }

    /// Analyzes a single collateral opportunity
    async fn analyze_opportunity(
        &self,
        collateral_asset: Address,
        seized_amount: U256,
        deficit: U256,
        base_asset: Address,
    ) -> Result<Option<BuyCollateralParams>> {
        if seized_amount.is_zero() {
            return Ok(None);
        }

        // Max collateral purchasable from deficit
        let max_collateral_from_deficit = self.comet
            .quote_collateral(collateral_asset, deficit)
            .call()
            .await?;

        if max_collateral_from_deficit.is_zero() {
            return Ok(None);
        }

        // Desired collateral = min(seized, max purchasable)
        let desired_collateral = seized_amount.min(max_collateral_from_deficit);

        // Compute base required
        let base_required = helpers::base_amount_for_collateral(
            &self.comet,
            collateral_asset,
            desired_collateral,
            deficit,
        )
        .await?;

        if base_required.is_zero() {
            return Ok(None);
        }

        // Confirm exact collateral received
        let expected_collateral = self.comet
            .quote_collateral(collateral_asset, base_required)
            .call()
            .await?;

        if expected_collateral.is_zero() {
            return Ok(None);
        }

        // Slippage protection for buyCollateral (0.3%)
        let min_collateral = expected_collateral * U256::from(997u64) / U256::from(1000u64);

        // Get token decimals for swap
        let (src_decimals, dest_decimals) = tokio::try_join!(
            get_token_decimals(collateral_asset, self.client.clone()),
            get_token_decimals(base_asset, self.client.clone())
        )?;

        // Build ParaSwap query
        let swap_params = SwapQueryParams {
            src_token: collateral_asset.to_string(),
            dest_token: base_asset.to_string(),
            src_decimals,
            dest_decimals,
            amount: min_collateral.to_string(), // use min collateral to guarantee swap works
            side: "SELL".into(),
            chain_id: constants::CHAIN_ID,
            slippage_bps: 30,
            user_address: self.flash_liquidator.address().to_string(),
            receiver: self.flash_liquidator.address().to_string(),
        };

        let paraswap_client = ParaSwapClient::new();
        let route = paraswap_client
            .compose_swap_data(swap_params)
            .await?;

        let min_base_out = route.min_amt_out;

        ensure!(
            min_base_out >= base_required,
            "Unprofitable after fee"
        );

        Ok(Some(BuyCollateralParams {
            collateral_asset,
            base_asset,
            base_amount: base_required,
            min_collateral,
            swap_target: route.swap_target,
            swap_proxy: route.token_transfer_proxy,
            swap_data: route.swap_data,
            min_base_out,
        }))
    }

    /// Generates all profitable arbitrage opportunities
    async fn generate_arbs(&self) -> Result<Vec<BuyCollateralParams>> {


    let snapshot = self.watch_list.snapshot();
    if snapshot.is_empty() {
        return Ok(vec![]);
    }

    let base_asset = self.comet.base_token().call().await?;

    let results: Vec<_> = stream::iter(snapshot)
        .map(|(collateral_asset, seized_amount)| async move {

            // fetch fresh reserves
            let reserves_i256 = self.comet.get_reserves().call().await?;
            let target_reserves = self.comet.target_reserves().call().await?;

            let base_reserves = if reserves_i256.is_negative() {
                U256::zero()
            } else {
                reserves_i256.into_raw()
            };

            if base_reserves >= target_reserves {
                return Ok(None);
            }

            let deficit = target_reserves - base_reserves;

            self.analyze_opportunity(
                collateral_asset,
                seized_amount,
                deficit,
                base_asset,
            ).await
        })
        .buffer_unordered(4)
        .filter_map(|res| async {
            match res {
                Ok(Some(p)) => Some(p),
                _ => None,
            }
        })
        .collect()
        .await;

    Ok(results)
}
 
}

#[async_trait::async_trait]
impl<M> Liquidator for CompoundLiquidator<M>
where
    M: Middleware + 'static,
{
    async fn run(&self) -> Result<()> {
        let opportunities = self.generate_arbs().await?;
        if opportunities.is_empty() {
            return Ok(());
        }

        let jobs = opportunities
            .into_iter()
            .map(|opp| {
                let debt = opp.base_amount;
                let data = LiquidationParams::from(opp);
                (debt, data)
            })
            .collect::<Vec<_>>();

        // Execute sequentially to avoid nonce conflicts
        stream::iter(jobs)
            .for_each_concurrent(2, |(debt, data)| async move {
                if simulate_liq_tx(
                    &self.flash_liquidator,
                    self.client.clone(),
                    debt,
                    data.clone(),
                )
                .await
                .is_ok()
                {
                    if let Err(e) = execute_liq_tx(
                        debt,
                        data.clone(),
                        &self.flash_liquidator,
                    )
                    .await
                    {
                        tracing::error!("buyCollateral arb failed: {:?}", e);
                    }
                }
            })
            .await;

        Ok(())
    }
}