use ethers::providers::Middleware;
use ethers::types::U256;
use std::sync::Arc;

use crate::common::{
    abi_bindings::{AaveOracle, IAaveV3Pool, UiPoolDataProvider},
    has_outstanding_debt,
};
use crate::config::AaveConfig;
use crate::core::ports::{LendingProtocolReader, ProtocolWatchList};
use crate::core::types::{BorrowerProfile, TrackerIdentity};

use super::helpers::aave_math_helpers::*;

use crate::common::get_token_decimals;
use crate::core::types::Protocol;
use futures_util::{self, stream, StreamExt};

pub struct AaveProtocolAdapter<M: Middleware + 'static> {
    pub pool: IAaveV3Pool<M>,
    pub oracle: AaveOracle<M>,
    pub watchlist: Arc<dyn ProtocolWatchList>,
    pub ui_pool_data_provider: UiPoolDataProvider<M>,
    pub client: Arc<M>,
    pub config: AaveConfig,
}

impl<M: Middleware + 'static> AaveProtocolAdapter<M> {
    pub fn new(
        pool: IAaveV3Pool<M>,
        oracle: AaveOracle<M>,
        watchlist: Arc<dyn ProtocolWatchList>,
        ui_pool_data_provider: UiPoolDataProvider<M>,
        client: Arc<M>,
        config: AaveConfig,
    ) -> Self {
        Self {
            pool,
            oracle,
            watchlist,
            ui_pool_data_provider,
            client,
            config,
        }
    }

    async fn evaluate_target(
        identity: String,
        pool: IAaveV3Pool<M>,
        oracle: AaveOracle<M>,
        ui_pool_data_provider: UiPoolDataProvider<M>,
        client: Arc<M>,
        config: AaveConfig,
    ) -> Option<BorrowerProfile> {
        if let Ok(TrackerIdentity::AaveV3 { borrower, reserve }) =
            identity.parse::<TrackerIdentity>()
        {
            tracing::debug!("Analyzing portfolio for borrower {}", borrower);

            let (_, _, _, _, _, hf) = pool.get_user_account_data(borrower).call().await.ok()?;

            if hf >= U256::exp10(18) {
                tracing::debug!("Borrower: {:?} is healthy with HF: {}", borrower, hf);
                return None;
            }

            let (user_reserves, _) = ui_pool_data_provider
                .get_user_reserves_data(config.pool_address_provider, borrower)
                .call()
                .await
                .map_err(|e| tracing::error!(?borrower, "UiPoolDataProvider call failed: {:?}", e))
                .ok()?;

            let collateral_positions: Vec<_> = user_reserves
                .iter()
                .filter(|r| {
                    r.usage_as_collateral_enabled_on_user && !r.scaled_a_token_balance.is_zero()
                })
                .collect();

            if collateral_positions.is_empty() {
                tracing::warn!("Borrower: {:?} has no collateral positions", borrower);
                return None;
            }

            if !has_outstanding_debt(borrower, reserve, &pool, &config)
                .await
                .ok()?
            {
                return None;
            }

            let v_debt = *config
                .vdebt_tokens
                .get(&reserve)
                .ok_or_else(|| anyhow::anyhow!("Missing vDebt"))
                .ok()?;

            let debt_to_cover = compute_debt_to_cover(borrower, v_debt, hf, client.clone())
                .await
                .ok()?;

            if debt_to_cover.is_zero() {
                return None;
            }

            let collateral_candidate = select_collateral_candidate(
                borrower,
                &collateral_positions,
                reserve,
                debt_to_cover,
                &pool,
                &oracle,
                client.clone(),
            )
            .await
            .ok()?;

            let (src_decimals, dest_decimals) = match tokio::try_join!(
                get_token_decimals(collateral_candidate.asset, client.clone()),
                get_token_decimals(reserve, client.clone())
            ) {
                Ok((src_decimals, dest_decimals)) => (src_decimals, dest_decimals),
                Err(e) => {
                    tracing::error!(
                        "Failed to fetch token decimals for borrower {:?}: {:?}",
                        borrower,
                        e
                    );
                    return None;
                }
            };

            return Some(BorrowerProfile {
                address: borrower,
                repaid_shares: None,
                seized_assets: None,
                market_id: None,
                debt_asset: reserve,
                debt_to_cover,
                collateral_asset: collateral_candidate.asset,
                seize_amount: collateral_candidate.seize_amount,
                src_decimals,
                dest_decimals,
                protocol: Protocol::Aave,
            });
        }

        None
    }
}

#[async_trait::async_trait]
impl<M: Middleware + 'static> LendingProtocolReader for AaveProtocolAdapter<M> {
    async fn fetch_liquidation_candidates(&self) -> anyhow::Result<Vec<BorrowerProfile>> {
        let tracked_identities = self.watchlist.snapshot();
        if tracked_identities.is_empty() {
            tracing::info!("Aave Liquidator: No borrowers to check");
            return Ok(Vec::new());
        }

        let pool = self.pool.clone();
        let oracle = self.oracle.clone();
        let ui_pool_provider = self.ui_pool_data_provider.clone();
        let client = self.client.clone();
        let config = self.config.clone();

        let candidates = stream::iter(tracked_identities)
            .map(move |identity| {
                Self::evaluate_target(
                    identity,
                    pool.clone(),
                    oracle.clone(),
                    ui_pool_provider.clone(),
                    client.clone(),
                    config.clone(),
                )
            })
            .buffer_unordered(10)
            .filter_map(|opt| async move { opt })
            .collect::<Vec<BorrowerProfile>>()
            .await;

        Ok(candidates)
    }
}
