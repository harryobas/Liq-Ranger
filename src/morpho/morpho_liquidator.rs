use anyhow::ensure;
use ethers::{
    providers::Middleware,
    types::{Address, H256, U256},
};

use futures_util::stream::{self, StreamExt};
use std::sync::Arc;

use super::{
    abi_bindings::{IMorphoBlue, IOracle, MarketParams},
    morpho_config::MorphoConfig,
    morpho_math::*,
    morpho_watchlist::MorphoWatchList,
    types::{HealthCheck, LiqCandidate, LiquidationMode, Market, Position},
};

use crate::common::{
    abi_bindings::{IFlashLiquidator, LiquidationParams},
    create_simulation_sandbox, execute_liq_tx, get_token_decimals,
    paraswap::ParaSwapClient,
    simulate_liq_tx,
    simulation_sandbox::AnvilSandbox,
    Liquidator, SwapQueryParams,
};

/// ─────────────────────────────────────────────
/// Liquidation mode (Morpho invariant enforced)
/// ─────────────────────────────────────────────

pub struct MorphoLiquidator<M: Middleware> {
    pub watch_list: Arc<MorphoWatchList>,
    pub morpho_blue: IMorphoBlue<M>,
    pub flash_liquidator: IFlashLiquidator<M>,
    pub client: Arc<M>,
    pub config: Arc<MorphoConfig>,
}

impl<M: Middleware + 'static> MorphoLiquidator<M> {
    pub fn new(
        morpho_blue: IMorphoBlue<M>,
        flash_liquidator: IFlashLiquidator<M>,
        watch_list: Arc<MorphoWatchList>,
        client: Arc<M>,
        config: Arc<MorphoConfig>,
    ) -> Self {
        Self {
            watch_list,
            morpho_blue,
            flash_liquidator,
            client,
            config,
        }
    }

    /// ─────────────────────────────────────────────
    /// Scan watchlist → produce liquidation candidates
    /// ─────────────────────────────────────────────
    pub async fn generate_liquidations(&self) -> anyhow::Result<Vec<LiqCandidate>> {
        let snapshot = self.watch_list.snapshot();

        if snapshot.is_empty() {
            tracing::info!("Morpho Liquidator: No borrowers to check");
            return Ok(vec![]);
        }

        tracing::info!("Morpho Liquidator: Checking {} borrowers", snapshot.len());

        let results: Vec<LiqCandidate> =
            stream::iter(snapshot)
                .map(|(borrower, markets)| async move {
                    self.analyze_portfolio(borrower, &markets).await
                })
                .buffer_unordered(10)
                .map(|res| match res {
                    Ok(candidates) => candidates,
                    Err(e) => {
                        tracing::warn!("analyze_portfolio failed: {:?}", e);
                        vec![]
                    }
                })
                .flat_map(stream::iter) // flatten Vec<Vec<_>>
                .collect()
                .await;

        Ok(results)
    }

    async fn analyze_portfolio(
        &self,
        borrower: Address,
        markets: &[H256],
    ) -> anyhow::Result<Vec<LiqCandidate>> {
        let mut candidates = Vec::new();

        for market_id in markets {
            if let Some(c) = self
                .analyze_borrower(borrower, market_id.to_fixed_bytes())
                .await?
            {
                candidates.push(c);
            }
        }

        Ok(candidates)
    }

    async fn analyze_borrower(
        &self,
        borrower: Address,
        market_id: [u8; 32],
    ) -> anyhow::Result<Option<LiqCandidate>> {
        tracing::debug!(
            "Analyzing borrower: {:?} in market: {:?}",
            borrower,
            H256::from(market_id)
        );

        let (_, borrow_shares, collateral) = self
            .morpho_blue
            .position(market_id, borrower)
            .call()
            .await?;

        if borrow_shares == 0 {
            return Ok(None);
        }

        let (_, _, total_borrow_assets, total_borrow_shares, _, _) =
            self.morpho_blue.market(market_id).call().await?;

        let (loan_token, collateral_token, oracle_addr, _, lltv) = self
            .morpho_blue
            .id_to_market_params(market_id)
            .call()
            .await?;

        let market = Market {
            total_borrow_assets,
            total_borrow_shares,
        };

        let market_params = MarketParams {
            loan_token,
            collateral_token,
            oracle: oracle_addr,
            irm: Address::zero(),
            lltv,
        };

        let oracle = IOracle::new(oracle_addr, self.client.clone());
        let price = oracle.price().call().await?;

        let position = Position {
            borrow_shares,
            collateral,
        };

        if position.is_healthy(&market, &market_params.lltv, &price) {
            tracing::debug!(
                "Borrower: {:?} is healthy in market: {:?}",
                borrower,
                H256::from(market_id)
            );
            return Ok(None);
        }

        let total_assets = U256::from(total_borrow_assets);
        let total_shares = U256::from(total_borrow_shares);
        let borrow_shares_u256 = U256::from(borrow_shares);

        let debt_assets = to_assets_down(borrow_shares_u256, total_assets, total_shares);

        if debt_assets.is_zero() {
            return Ok(None);
        }

        // Full-debt sizing for mode selection (theoretical max collateral needed).
        let lif = incentive_factor(lltv);
        let required_collateral = mul_div_down(
            wmul_down(debt_assets, lif),
            self.config.oracle_price_scale,
            price,
        );

        let available_collateral = U256::from(collateral);

        // ─────────────────────────────────────────────
        // 8. Decide liquidation mode (CRITICAL)
        // ─────────────────────────────────────────────
        let mode = if available_collateral >= required_collateral {
            let seized_for_swap = seized_assets_from_repaid_shares(
                borrow_shares_u256,
                total_assets,
                total_shares,
                lltv,
                self.config.oracle_price_scale,
                price,
            );

            if seized_for_swap.is_zero() {
                return Ok(None);
            }

            LiquidationMode::RepayShares {
                repaid_shares: borrow_shares_u256,
                expected_seized_assets: seized_for_swap,
            }
        } else {
            LiquidationMode::SeizeCollateral {
                seized_assets: available_collateral,
            }
        };

        // ─────────────────────────────────────────────
        // 9. Swap sizing (economic only)
        // ─────────────────────────────────────────────
        let collateral_for_swap = match &mode {
            LiquidationMode::RepayShares {
                expected_seized_assets,
                ..
            } => *expected_seized_assets,
            LiquidationMode::SeizeCollateral { seized_assets } => *seized_assets,
        };

        if collateral_for_swap.is_zero() {
            return Ok(None);
        }

        // ─────────────────────────────────────────────
        //  ParaSwap routing
        // ─────────────────────────────────────────────
        let (src_decimals, dest_decimals) = tokio::try_join!(
            get_token_decimals(collateral_token, self.client.clone()),
            get_token_decimals(loan_token, self.client.clone())
        )?;

        let swap_params = SwapQueryParams {
            src_token: collateral_token.to_string(),
            dest_token: loan_token.to_string(),
            src_decimals: src_decimals,
            dest_decimals: dest_decimals,
            amount: collateral_for_swap.to_string(),
            side: "SELL".to_string(),
            chain_id: self.config.chain_id,
            user_address: self.flash_liquidator.address().to_string(),
            slippage_bps: 30,
            receiver: self.flash_liquidator.address().to_string(),
        };

        let paraswap_client = ParaSwapClient::new();
        let route = paraswap_client.compose_swap_data(swap_params).await?;

        // ─────────────────────────────────────────────
        //  Enforce Morpho invariant
        // ─────────────────────────────────────────────
        let (repaid_shares, seized_assets) = match &mode {
            LiquidationMode::RepayShares { repaid_shares, .. } => (*repaid_shares, U256::zero()),
            LiquidationMode::SeizeCollateral { seized_assets } => (U256::zero(), *seized_assets),
        };

        let debt_to_cover = match &mode {
            LiquidationMode::RepayShares { repaid_shares, .. } => {
                repaid_assets_from_repaid_shares(*repaid_shares, total_assets, total_shares)
            }
            LiquidationMode::SeizeCollateral { seized_assets } => repaid_assets_from_seized_collateral(
                *seized_assets,
                total_assets,
                total_shares,
                lltv,
                self.config.oracle_price_scale,
                price,
            ),
        };

        ensure!(
            route.min_amt_out >= debt_to_cover,
            "swap output insufficient to repay debt"
        );

        ensure!(
            collateral_for_swap >= route.src_amount,
            "swap src exceeds seized collateral"
        );

        // ─────────────────────────────────────────────
        //  Build candidate
        // ─────────────────────────────────────────────
        Ok(Some(LiqCandidate {
            borrower,
            market_id: H256::from(market_id),
            debt_to_cover,
            repaid_shares,
            seized_assets,
            debt_token: loan_token,
            collateral_token,
            swap_target: route.swap_target,
            swap_data: route.swap_data,
            swap_proxy: route.token_transfer_proxy,
            min_amt_out: route.min_amt_out,
        }))
    }
}

#[async_trait::async_trait]
impl<M> Liquidator for MorphoLiquidator<M>
where
    M: Middleware + 'static,
{
    async fn run(&self, block_number: u64) -> anyhow::Result<()> {
        tracing::info!(
            "🚀 Running Morpho liquidation engine for block {}",
            block_number
        );
        let candidates = self.generate_liquidations().await?;
        if candidates.is_empty() {
            tracing::info!("Morpho Liquidator: No liquidation candidates found");
            return Ok(());
        }

        tracing::info!(
            "Morpho Liquidator: Found {} liquidation candidates",
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

        let sim_sandbox: AnvilSandbox =
            create_simulation_sandbox(block_number, &self.flash_liquidator).await?;
        let snapshot_id = sim_sandbox.snapshot().await?;

        for (loan_amt, liq_params) in &jobs {
            match simulate_liq_tx(
                &self.flash_liquidator,
                &sim_sandbox,
                *loan_amt,
                liq_params.clone(),
                snapshot_id,
            )
            .await
            {
                Ok(res) => {
                    if let Err(e) = execute_liq_tx(
                        *loan_amt,
                        liq_params.clone(),
                        &self.flash_liquidator,
                        res.gas_used,
                    )
                    .await
                    {
                        tracing::error!("liquidation failed: {:?}", e);
                    }
                }
                Err(e) => {
                    tracing::error!("Simulation failed: {:?}", e)
                }
            }
        }
        tracing::info!(
            "Morpho liquidation cycle completed for block {}",
            block_number
        );
        Ok(())
    }
}
