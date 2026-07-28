// src/infrastructure/adapters/protocol_readers/morpho_adapter.rs

use super::helpers::morpho_math_helpers::*;
use crate::common::abi_bindings::{IMorphoBlue, IOracle, MarketParams};
use crate::common::get_token_decimals;
use crate::config::MorphoConfig;
use crate::core::ports::{LendingProtocolReader, ProtocolWatchList};
use crate::core::types::{
    BorrowerProfile,
    LiquidationMode,
    Market,
    Position,
    Protocol,
    TrackerIdentity,
    HealthCheck
};
use ethers::{
    providers::Middleware,
    types::{Address, H256, U256},
};
use futures_util::stream::{self, StreamExt};
use std::collections::HashSet;
use std::sync::Arc;

pub struct MorphoProtocolAdapter<M: Middleware + 'static> {
    pub morpho: IMorphoBlue<M>,
    pub watchlist: Arc<dyn ProtocolWatchList>,
    pub client: Arc<M>,
    pub config: MorphoConfig,
}

impl<M: Middleware + 'static> MorphoProtocolAdapter<M> {
    pub fn new(
        morpho: IMorphoBlue<M>,
        watchlist: Arc<dyn ProtocolWatchList>,
        client: Arc<M>,
        config: MorphoConfig,
    ) -> Self {
        Self {
            morpho,
            watchlist,
            client,
            config,
        }
    }

    /// Evaluates a single Morpho Blue position.
    /// Uses concurrent RPC fetches for position, market data, and oracle prices to minimize roundtrip latency.
    async fn evaluate_target(
        identity: String,
        morpho: IMorphoBlue<M>,
        client: Arc<M>,
        config: MorphoConfig,
    ) -> Option<BorrowerProfile> {
        let (market_id, borrower) = match identity.parse::<TrackerIdentity>().ok()? {
            TrackerIdentity::MorphoBlue {
                market_id,
                borrower,
            } => (market_id, borrower),
            _ => {
                tracing::warn!("Failed to parse Morpho TrackerIdentity: {}", identity);
                return None;
            }
        };

        let market_bytes = market_id.to_fixed_bytes();

        // 1. Fetch Position and Market Data concurrently
        let pos_fut = morpho.position(market_bytes, borrower);
        let market_fut = morpho.market(market_bytes);
        let params_fut = morpho.id_to_market_params(market_bytes);

        let (pos_res, market_res, params_res) =
            tokio::try_join!(pos_fut.call(), market_fut.call(), params_fut.call()).ok()?;

        let (_, borrow_shares, collateral) = pos_res;
        if borrow_shares == 0 {
            tracing::debug!("Borrower {:?} in market {:?} has zero borrow shares", borrower, H256::from(market_id));
            return None;
        }

        let (_, _, total_borrow_assets, total_borrow_shares, _, _) = market_res;
        let (loan_token, collateral_token, oracle_addr, _, lltv) = params_res;

        // 2. Fetch Oracle Price
        let oracle_contract = IOracle::new(oracle_addr, client.clone());
        let price = oracle_contract.price().call().await.ok()?;

        let market = Market {
            total_borrow_assets: total_borrow_assets.into(),
            total_borrow_shares: total_borrow_shares.into(),
        };

        let market_params = MarketParams {
            loan_token,
            collateral_token,
            oracle: oracle_addr,
            irm: Address::zero(),
            lltv,
        };

        let position = Position {
            borrow_shares: borrow_shares.into(),
            collateral: collateral.into(),
        };

        // 3. Fast-Path Exit: Check Health Factor
        if position.is_healthy(&market, &market_params.lltv, &price) {
            tracing::debug!(
                "Borrower {:?} is healthy in market {:?}",
                borrower,
                H256::from(market_id)
            );
            return None;
        }

        tracing::warn!(
            "🚨 Unhealthy Morpho position detected: borrower {:?} in market {:?}",
            borrower,
            H256::from(market_id)
        );

        let total_assets = U256::from(total_borrow_assets);
        let total_shares = U256::from(total_borrow_shares);
        let borrow_shares_u256 = U256::from(borrow_shares);

        let debt_assets = to_assets_down(borrow_shares_u256, total_assets, total_shares);
        if debt_assets.is_zero() {
            return None;
        }

        let lif = incentive_factor(lltv);
        let required_collateral = mul_div_down(
            wmul_down(debt_assets, lif),
            config.oracle_price_scale,
            price,
        );

        let available_collateral = U256::from(collateral);

        // 4. Determine Liquidation Mode (Full Repay vs Bad Debt Seize)
        let mode = if available_collateral >= required_collateral {
            let seized_for_swap = seized_assets_from_repaid_shares(
                borrow_shares_u256,
                total_assets,
                total_shares,
                lltv,
                config.oracle_price_scale,
                price,
            );

            if seized_for_swap.is_zero() {
                tracing::debug!("Zero expected seized assets for borrower {:?}", borrower);
                return None;
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

        let collateral_for_swap = match &mode {
            LiquidationMode::RepayShares {
                expected_seized_assets,
                ..
            } => *expected_seized_assets,
            LiquidationMode::SeizeCollateral { seized_assets } => *seized_assets,
        };

        if collateral_for_swap.is_zero() {
            return None;
        }

        let (repaid_shares, seized_assets) = match &mode {
            LiquidationMode::RepayShares { repaid_shares, .. } => (*repaid_shares, U256::zero()),
            LiquidationMode::SeizeCollateral { seized_assets } => (U256::zero(), *seized_assets),
        };

        // 5. Fetch Decimals Concurrently
        let (src_decimals, dest_decimals) = tokio::try_join!(
            get_token_decimals(collateral_token, client.clone()),
            get_token_decimals(loan_token, client.clone())
        )
        .ok()?;

        Some(BorrowerProfile {
            address: borrower,
            repaid_shares: Some(repaid_shares),
            seized_assets: Some(seized_assets),
            market_id: Some(H256::from(market_id)),
            debt_asset: market_params.loan_token,
            debt_to_cover: debt_assets,
            collateral_asset: market_params.collateral_token,
            seize_amount: collateral_for_swap,
            src_decimals,
            dest_decimals,
            protocol: Protocol::Morpho,
            identity,
        })
    }
}

#[async_trait::async_trait]
impl<M: Middleware + 'static> LendingProtocolReader for MorphoProtocolAdapter<M> {
    async fn fetch_liquidation_candidates(&self) -> anyhow::Result<Vec<BorrowerProfile>> {
        let tracked_identities = self.watchlist.snapshot();
        if tracked_identities.is_empty() {
            tracing::info!("Morpho Liquidator: Watchlist is empty");
            return Ok(Vec::new());
        }

        // Deduplicate identities before processing RPC queries
        let unique_identities: HashSet<String> = tracked_identities.into_iter().collect();

        let morpho = self.morpho.clone();
        let client = self.client.clone();
        let config = self.config.clone();

        let candidates = stream::iter(unique_identities)
            .map(move |identity| {
                Self::evaluate_target(identity, morpho.clone(), client.clone(), config.clone())
            })
            .buffer_unordered(10)
            .filter_map(|opt| async move { opt })
            .collect::<Vec<BorrowerProfile>>()
            .await;

        Ok(candidates)
    }

    fn name(&self) -> &'static str {
        "morpho"
    }

    async fn refresh_borrower(&self, identity: &str) -> anyhow::Result<BorrowerProfile> {
        // Rapidly verify position state
        if let Ok(TrackerIdentity::MorphoBlue {
            market_id,
            borrower,
        }) = identity.parse::<TrackerIdentity>()
        {
            let (_, borrow_shares, _) = self
                .morpho
                .position(market_id.to_fixed_bytes(), borrower)
                .call()
                .await
                .map_err(|e| anyhow::anyhow!("RPC error checking Morpho position: {:?}", e))?;

            if borrow_shares == 0 {
                anyhow::bail!("Borrower {:?} has no outstanding debt shares", borrower);
            }
        }

        Self::evaluate_target(
            identity.to_string(),
            self.morpho.clone(),
            self.client.clone(),
            self.config.clone(),
        )
        .await
        .ok_or_else(|| {
            anyhow::anyhow!("Position no longer liquidatable for identity: {}", identity)
        })
    }
}