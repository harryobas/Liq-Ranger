use super::{
    aave_config::AaveConfig,
    aave_watchlist::AaveWatchList,
    abi_bindings::{AaveOracle, IAaveV3Pool, UiPoolDataProvider},
    helpers::{compute_debt_to_cover, select_collateral_candidates},
    types::LiquidationCandidate,
};

use ethers::{
    providers::Middleware,
    types::{Address, U256},
};

use super::helpers::has_outstanding_debt;
use std::{collections::HashSet, sync::Arc};

use crate::common::{
    abi_bindings::{IFlashLiquidator, LiquidationParams},
    create_simulation_sandbox, execute_liq_tx, get_token_decimals,
    paraswap::ParaSwapClient,
    simulate_liq_tx, Liquidator, SwapQueryParams,
};
use futures_util::{self, stream, StreamExt};

pub struct AaveLiquidator<M: Middleware + 'static> {
    pub lending_pool: IAaveV3Pool<M>,
    pub flash_liq: IFlashLiquidator<M>,
    pub aave_oracle: AaveOracle<M>,
    pub ui_pool_data: UiPoolDataProvider<M>,
    pub client: Arc<M>,
    pub watch_list: Arc<AaveWatchList>,
    pub config: Arc<AaveConfig>,
}

impl<M: Middleware> AaveLiquidator<M> {
    pub fn new(
        config: Arc<AaveConfig>,
        client: Arc<M>,
        watch_list: Arc<AaveWatchList>,
        lending_pool: IAaveV3Pool<M>,
        aave_oracle: AaveOracle<M>,
        ui_pool_data: UiPoolDataProvider<M>,
        flash_liq: IFlashLiquidator<M>,
    ) -> Self {
        Self {
            lending_pool,
            flash_liq,
            aave_oracle,
            ui_pool_data,
            client,
            watch_list,
            config,
        }
    }

    async fn generate_liquidations(&self) -> anyhow::Result<Vec<LiquidationCandidate>> {
        let snapshot = self.watch_list.snapshot();

        if snapshot.is_empty() {
            tracing::info!("Aave Liquidator: No borrowers to check");
            return Ok(vec![]);
        }

        tracing::info!("Aave Liquidator: Checking {} borrowers", snapshot.len());

        let results: Vec<LiquidationCandidate> = stream::iter(snapshot)
            .map(|(borrower, reserves)| {
                let this = self;
                async move { this.analyze_portfolio(borrower, reserves).await }
            })
            .buffer_unordered(10)
            .filter_map(|res| async {
                match res {
                    Ok(candidates) => Some(candidates),
                    Err(e) => {
                        tracing::warn!("Aave borrower analysis failed: {:?}", e);
                        None
                    }
                }
            })
            .flat_map(stream::iter)
            .collect()
            .await;

        Ok(results)
    }

    async fn analyze_portfolio(
        &self,
        borrower: Address,
        reserves: HashSet<Address>,
    ) -> anyhow::Result<Vec<LiquidationCandidate>> {
        tracing::debug!("Analyzing portfolio for borrower {}", borrower);

        // 1. Health factor check
        let (_, _, _, _, _, hf) = self
            .lending_pool
            .get_user_account_data(borrower)
            .call()
            .await?;

        if hf >= U256::exp10(18) {
            tracing::debug!("Borrower: {:?} is healthy with HF: {}", borrower, hf);
            return Ok(vec![]);
        }

        let (user_reserves, _) = self
            .ui_pool_data
            .get_user_reserves_data(self.config.pool_address_provider, borrower)
            .call()
            .await?;

        let collateral_positions: Vec<_> = user_reserves
            .iter()
            .filter(|r| {
                r.usage_as_collateral_enabled_on_user && !r.scaled_a_token_balance.is_zero()
            })
            .collect();

        if collateral_positions.is_empty() {
            return Ok(vec![]);
        }

        let mut candidates = Vec::new();

        for debt_asset in reserves {
            if !has_outstanding_debt(borrower, debt_asset, &self.lending_pool, &self.config).await?
            {
                continue;
            }

            let v_debt = *self
                .config
                .vdebt_tokens
                .get(&debt_asset)
                .ok_or_else(|| anyhow::anyhow!("Missing vDebt"))?;

            let debt_to_cover =
                compute_debt_to_cover(borrower, v_debt, hf, self.client.clone()).await?;

            if debt_to_cover.is_zero() {
                continue;
            }

            // 3. Rank collateral and try routes until one is economically viable.
            let collateral_candidates = select_collateral_candidates(
                borrower,
                &collateral_positions,
                debt_asset,
                debt_to_cover,
                &self.lending_pool,
                &self.aave_oracle,
                self.client.clone(),
            )
            .await?;

            let paraswap_client = ParaSwapClient::new();
            let mut found_route = false;

            for collateral in collateral_candidates {
                // 4. ParaSwap routing
                let (src_decimals, dest_decimals) = match tokio::try_join!(
                    get_token_decimals(collateral.asset, self.client.clone()),
                    get_token_decimals(debt_asset, self.client.clone())
                ) {
                    Ok(decimals) => decimals,
                    Err(e) => {
                        tracing::warn!(
                        "Aave token decimal lookup failed for borrower {:?}, debt {:?}, collateral {:?}: {:?}",
                        borrower,
                        debt_asset,
                        collateral.asset,
                        e
                    );
                        continue;
                    }
                };

                let swap_params = SwapQueryParams {
                    src_token: collateral.asset.to_string(),
                    dest_token: debt_asset.to_string(),
                    src_decimals,
                    dest_decimals,
                    amount: collateral.seize_amount.to_string(),
                    side: String::from("SELL"),
                    chain_id: self.config.chain_id,
                    slippage_bps: 30, // 0.3%
                    user_address: self.flash_liq.address().to_string(),
                    receiver: self.flash_liq.address().to_string(),
                };

                let route = match paraswap_client.compose_swap_data(swap_params).await {
                    Ok(route) => route,
                    Err(e) => {
                        tracing::debug!(
                        "Aave ParaSwap route failed for borrower {:?}, debt {:?}, collateral {:?}: {:?}",
                        borrower,
                        debt_asset,
                        collateral.asset,
                        e
                    );
                        continue;
                    }
                };

                if collateral.seize_amount < route.src_amount {
                    tracing::debug!(
                    "Aave route skipped: source amount exceeds seized collateral for borrower {:?}, debt {:?}, collateral {:?}",
                    borrower,
                    debt_asset,
                    collateral.asset
                );
                    continue;
                }

                if route.min_amt_out < debt_to_cover {
                    tracing::debug!(
                    "Aave route skipped: swap output below debt for borrower {:?}, debt {:?}, collateral {:?}",
                    borrower,
                    debt_asset,
                    collateral.asset
                );
                    continue;
                }

                candidates.push(LiquidationCandidate {
                    debt_to_cover,
                    debt_asset: debt_asset,
                    collateral_asset: collateral.asset,
                    borrower,
                    swap_target: route.swap_target,
                    swap_proxy: route.token_transfer_proxy,
                    swap_data: route.swap_data,
                    min_amt_out: route.min_amt_out,
                });
                found_route = true;
                break;
            }

            if !found_route {
                tracing::debug!(
                    "Aave borrower {:?} has debt {:?} but no viable collateral swap route",
                    borrower,
                    debt_asset
                );
            }
        }
        Ok(candidates)
    }
}

#[async_trait::async_trait]
impl<M> Liquidator for AaveLiquidator<M>
where
    M: Middleware + 'static,
{
    async fn run(&self, block_number: u64) -> anyhow::Result<()> {
        tracing::info!(
            "🚀 Running Aave liquidation engine for block {}",
            block_number
        );
        let candidates = self.generate_liquidations().await?;
        if candidates.is_empty() {
            tracing::debug!("Aave Liquidator: No unhealthy borrowers to check");
            return Ok(());
        }

        tracing::info!(
            "Aave Liquidator: Found {} liquidation candidates",
            candidates.len()
        );

        let jobs = candidates
            .into_iter()
            .map(|c| {
                let debt = c.debt_to_cover;
                let data = LiquidationParams::from(c);
                (debt, data)
            })
            .collect::<Vec<_>>();

        let sim_sandbox = create_simulation_sandbox(block_number, &self.flash_liq).await?;
        let snapshot_id = sim_sandbox.snapshot().await?;

        for (loan_amt, liq_params) in &jobs {
            match simulate_liq_tx(
                &self.flash_liq,
                &sim_sandbox,
                *loan_amt,
                liq_params.clone(),
                snapshot_id,
            )
            .await
            {
                Ok(res) => {
                    if let Err(e) =
                        execute_liq_tx(*loan_amt, liq_params.clone(), &self.flash_liq, res.gas_used)
                            .await
                    {
                        tracing::error!("Liquidation execution failed: {:?}", e);
                    }
                }
                Err(e) => {
                    tracing::error!("Simulation failed for loan amount {}: {:?}", loan_amt, e);
                }
            }
        }

        tracing::info!(
            "Aave liquidation cycle completed for block {}",
            block_number
        );
        Ok(())
    }
}
