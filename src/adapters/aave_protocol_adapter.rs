use ethers::{providers::Middleware, types::U256};
use std::collections::HashSet;
use std::sync::Arc;

use crate::common::{
    abi_bindings::{AaveOracle, IAaveV3Pool, UiPoolDataProvider},
    get_token_decimals, has_outstanding_debt,
};
use crate::config::AaveConfig;
use crate::core::ports::{LendingProtocolReader, ProtocolWatchList};
use crate::core::types::{BorrowerProfile, Protocol, TrackerIdentity};

use super::helpers::aave_math_helpers::*;
use futures_util::{stream, StreamExt};

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

    /// Evaluates a single unique borrower.
    /// Exits after 1 RPC call if HF >= 1.0. Dynamically discovers actual debt reserves if liquidatable.
    async fn evaluate_borrower(
        borrower: ethers::types::Address,
        target_reserve: Option<ethers::types::Address>,
        pool: IAaveV3Pool<M>,
        oracle: AaveOracle<M>,
        ui_pool_data_provider: UiPoolDataProvider<M>,
        client: Arc<M>,
        config: AaveConfig,
    ) -> Option<BorrowerProfile> {
        tracing::debug!("Analyzing portfolio for borrower {:?}", borrower);

        // 1. Account-level Health Factor Check (Fast Path Exit)
        let (_, _, _, _, _, hf) = pool.get_user_account_data(borrower).call().await.ok()?;

        if hf >= U256::exp10(18) {
            tracing::debug!("Borrower {:?} is healthy with HF: {}", borrower, hf);
            return None;
        }

        tracing::warn!("🚨 Unhealthy borrower detected: {:?} (HF: {})", borrower, hf);

        // 2. Fetch User Reserves on-chain to discover actual Debt and Collateral positions
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
            tracing::warn!("Borrower {:?} has no enabled collateral positions", borrower);
            return None;
        }

        // 3. Determine the Debt Reserve to Repay
        let debt_reserve = match target_reserve {
            Some(res) => res,
            None => {
                let mut found_debt = None;
                for user_res in &user_reserves {
                    if !user_res.scaled_variable_debt.is_zero()
                    {
                        let candidate_reserve = user_res.underlying_asset;
                        if has_outstanding_debt(borrower, candidate_reserve, &pool, &config)
                            .await
                            .unwrap_or(false)
                        {
                            found_debt = Some(candidate_reserve);
                            break;
                        }
                    }
                }
                found_debt?
            }
        };

        let v_debt = *config
            .vdebt_tokens
            .get(&debt_reserve)
            .ok_or_else(|| anyhow::anyhow!("Missing vDebt mapping for reserve {:?}", debt_reserve))
            .ok()?;

        let debt_to_cover = compute_debt_to_cover(borrower, v_debt, hf, client.clone())
            .await
            .ok()?;

        if debt_to_cover.is_zero() {
            return None;
        }

        // 4. Select Best Collateral Candidate to Seize
        let collateral_candidate = select_collateral_candidate(
            borrower,
            &collateral_positions,
            debt_reserve,
            debt_to_cover,
            &pool,
            &oracle,
            client.clone(),
        )
        .await
        .ok()?;

         let (src_decimals, dest_decimals) = tokio::try_join!(
            get_token_decimals(collateral_candidate.asset, client.clone()),
            get_token_decimals(debt_reserve, client.clone())
        ).ok()?;

        let identity = TrackerIdentity::AaveV3 {
            borrower,
            reserve: debt_reserve,
        }
        .to_string_id();

        Some(BorrowerProfile {
            address: borrower,
            repaid_shares: None,
            seized_assets: None,
            market_id: None,
            debt_asset: debt_reserve,
            debt_to_cover,
            collateral_asset: collateral_candidate.asset,
            seize_amount: collateral_candidate.seize_amount,
            src_decimals,
            dest_decimals,
            protocol: Protocol::Aave,
            identity,
        })
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

        // Deduplicate borrower addresses strictly before issuing RPC calls
        let unique_borrowers: HashSet<ethers::types::Address> = tracked_identities
            .into_iter()
            .filter_map(|id| match id.parse::<TrackerIdentity>() {
                Ok(TrackerIdentity::AaveV3 { borrower, .. }) => Some(borrower),
                _ => None,
            })
            .collect();

        let pool = self.pool.clone();
        let oracle = self.oracle.clone();
        let ui_pool_provider = self.ui_pool_data_provider.clone();
        let client = self.client.clone();
        let config = self.config.clone();

        let candidates = stream::iter(unique_borrowers)
            .map(move |borrower| {
                Self::evaluate_borrower(
                    borrower,
                    None, // Reserve resolved dynamically if liquidatable
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

    fn name(&self) -> &'static str {
        "aave"
    }

    async fn refresh_borrower(&self, identity: &str) -> anyhow::Result<BorrowerProfile> {
        let (borrower, target_reserve) = match identity.parse::<TrackerIdentity>() {
            Ok(TrackerIdentity::AaveV3 { borrower, reserve }) => (borrower, Some(reserve)),
            _ => anyhow::bail!("Invalid tracker identity format: {}", identity),
        };

        Self::evaluate_borrower(
            borrower,
            target_reserve,
            self.pool.clone(),
            self.oracle.clone(),
            self.ui_pool_data_provider.clone(),
            self.client.clone(),
            self.config.clone(),
        )
        .await
        .ok_or_else(|| {
            anyhow::anyhow!(
                "Position no longer liquidatable for identity: {}",
                identity
            )
        })
    }
}