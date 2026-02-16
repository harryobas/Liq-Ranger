use anyhow::ensure;
use ethers::{
    providers::Middleware,
    types::{Address, H256, U256},
};

use std::sync::Arc;
use futures_util::stream::{self, StreamExt};

use super::{
    abi_bindings::{IMorphoBlue, IOracle, MarketParams},
    helpers,
    morpho_math::*,
    morpho_config::MorphoConfig,
    morpho_watchlist::MorphoWatchList,
    types::{LiqCandidate,Market, Position, HealthCheck, LiquidationMode},
};

use crate::common::{
    Liquidator, 
    SwapQueryParams, 
    execute_liq_tx, 
    get_token_decimals, 
    paraswap::ParaSwapClient, 
    simulate_liq_tx,
    liq_data::LiqData,
    abi_bindings::IFlashLiquidator
    
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
        return Ok(vec![]);
    }

    let results: Vec<_> = stream::iter(snapshot)
        .map(|(borrower, market_id)| async move {
            self.analyze_borrower(borrower, market_id.to_fixed_bytes()).await
        })
        .buffer_unordered(4)
        .filter_map(|res| async {
            match res {
                Ok(Some(candidate)) => Some(candidate),
                Ok(None) => None,
                Err(e) => {
                    tracing::warn!("analyze_borrower failed: {:?}", e);
                    None
                }
            }
        })
        .collect()
        .await;

    Ok(results)
}

async fn analyze_borrower(
     &self,
     borrower: Address,
     market_id: [u8; 32],
    ) -> anyhow::Result<Option<LiqCandidate>> {

        let (_, borrow_shares, collateral) =
            self.morpho_blue.position(market_id, borrower).call().await?;

        if borrow_shares == 0 {
            return Ok(None);
        }

        
        let (_, _, total_borrow_assets, total_borrow_shares, _, _) =
            self.morpho_blue.market(market_id).call().await?;

        let (loan_token, collateral_token, oracle_addr, _, lltv) =
            self.morpho_blue.id_to_market_params(market_id).call().await?;

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
            return Ok(None);
        }

        let debt_assets = to_assets_down(
            U256::from(borrow_shares),
            U256::from(total_borrow_assets),
            U256::from(total_borrow_shares),
        );

        if debt_assets.is_zero() {
            return Ok(None);
        }

        // ─────────────────────────────────────────────
        // 6. Partial liquidation sizing (50%)
        // ─────────────────────────────────────────────
        let close_factor = U256::from_dec_str("850000000000000000")?;
        let repay_assets = wmul_down(debt_assets, close_factor);

        // ─────────────────────────────────────────────
        // 7. Incentive & theoretical seize
        // ─────────────────────────────────────────────
        let lif = incentive_factor(lltv);
        let incentivized_repay = wmul_down(repay_assets, lif);

        let required_collateral = mul_div_down(
            incentivized_repay,
            self.config.oracle_price_scale,
            price,
        );

        let available_collateral = U256::from(collateral);

        // ─────────────────────────────────────────────
        // 8. Decide liquidation mode (CRITICAL)
        // ─────────────────────────────────────────────
        let mode = if available_collateral >= required_collateral {
            let repaid_shares = to_shares_down(
                repay_assets,
                U256::from(total_borrow_assets),
                U256::from(total_borrow_shares),
            );

            if repaid_shares.is_zero() {
                return Ok(None);
            }

            LiquidationMode::RepayShares {
                repaid_shares,
                expected_seized_assets: required_collateral,
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
            receiver: self.flash_liquidator.address().to_string()

        };

        let paraswap_client = ParaSwapClient::new();
        let route = paraswap_client.compose_swap_data(swap_params).await?;

        // ─────────────────────────────────────────────
        //  Enforce Morpho invariant
        // ─────────────────────────────────────────────
        let (repaid_shares, seized_assets) = match mode {
            LiquidationMode::RepayShares { repaid_shares, .. } => {
                (repaid_shares, U256::zero())
            }
            LiquidationMode::SeizeCollateral { seized_assets } => {
                (U256::zero(), seized_assets)
            }
        };

        ensure!(
            route.dest_amount >= repay_assets, 
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
            debt_to_cover: repay_assets,
            repaid_shares,
            seized_assets,
            debt_token: loan_token,
            collateral_token,
            swap_target: route.swap_target,
            swap_data: route.swap_data,
            swap_proxy: route.token_transfer_proxy
        }))
    }
}

#[async_trait::async_trait]
impl<M> Liquidator for MorphoLiquidator<M>
where
    M: Middleware + 'static,
{
    async fn run(&self) -> anyhow::Result<()> {
        let candidates = self.generate_liquidations().await?;
        if candidates.is_empty() {
            return Ok(());
        }

        let jobs = candidates
            .into_iter()
            .map(|c| {
                let debt = c.debt_to_cover;
                let debt_asset = c.debt_token;
                let data = LiqData::from(c);
                (debt, debt_asset, data)
            })
            .collect::<Vec<_>>()
            .into_iter()
            .map(|(debt, debt_asset, data)| {
                (debt, debt_asset, helpers::encode_liq_data(&data))
            })
            .collect::<Vec<_>>();

        stream::iter(jobs)
            .for_each_concurrent(2, |(loan_amt, debt_asset, data)| async move {
                if simulate_liq_tx(
                    &self.flash_liquidator,
                    self.config.clone(),
                    self.client.clone(),
                    loan_amt,
                    debt_asset,
                    data.clone(),
                )
                .await
                .is_ok()
                {
                    if let Err(e) = execute_liq_tx(
                        loan_amt,
                        debt_asset,
                        data,
                        &self.flash_liquidator,
                    )
                    .await
                    {
                        tracing::error!("liquidation failed: {:?}", e);
                    }
                }
            })
            .await;

        Ok(())
    }
}
