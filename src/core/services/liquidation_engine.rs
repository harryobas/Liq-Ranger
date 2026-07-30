use crate::core::ports::{DexRouteFinder, EvmSimulator, LiquidationContract};
use crate::core::types::{BorrowerProfile, LiqPayload, LiquidationJob, MarketQuote, TxPayload};
use std::sync::Arc;
use std::time::Instant;
use tokio::sync::{mpsc, watch, Semaphore};
use tracing::{error, info, warn};

#[derive(Clone)]
pub struct PipelineEngine {
    pub dex_finder: Arc<dyn DexRouteFinder + Send + Sync>,
    pub simulator: Arc<dyn EvmSimulator + Send + Sync>,
    pub liquidator: Arc<dyn LiquidationContract + Send + Sync>,
}

impl PipelineEngine {
    pub fn new(
        dex_finder: Arc<dyn DexRouteFinder + Send + Sync>,
        simulator: Arc<dyn EvmSimulator + Send + Sync>,
        liquidator: Arc<dyn LiquidationContract + Send + Sync>,
    ) -> Self {
        Self {
            dex_finder,
            simulator,
            liquidator,
        }
    }

    /// Spawns the background execution engine task consuming LiqPayload items from the mpsc channel
    pub fn start_pipeline(
        self: Arc<Self>,
        mut payload_rx: mpsc::Receiver<LiqPayload>,
        mut shutdown_rx: watch::Receiver<bool>,
        concurrency_limit: usize,
    ) {
        tokio::spawn(async move {
            info!(
                concurrency_limit = concurrency_limit,
                "🚀 Pipeline Execution Engine initialized and listening for candidate payloads"
            );

            let semaphore = Arc::new(Semaphore::new(concurrency_limit));

            loop {
                tokio::select! {
                    // Graceful Shutdown Signal
                    _ = shutdown_rx.changed() => {
                        info!("🛑 Pipeline Execution Engine shutting down...");
                        break;
                    }

                    // Process Incoming Liquidation Payloads
                    Some(payload) = payload_rx.recv() => {
                        let permit = match semaphore.clone().acquire_owned().await {
                            Ok(p) => p,
                            Err(_) => break, // Semaphore closed
                        };

                        let engine = Arc::clone(&self);
                        tokio::spawn(async move {
                            let _permit = permit; // Holds permit until evaluation finishes
                            engine.process_payload(payload).await;
                        });
                    }
                }
            }

            info!("✅ Pipeline Execution Engine stopped cleanly");
        });
    }

    /// Main orchestration pipeline: Validates -> Quotes -> Checks Profit -> Simulates -> JIT Refreshes -> Executes
    async fn process_payload(&self, payload: LiqPayload) {
        let start_time = Instant::now();
        let profile = &payload.profile;

        info!(
            borrower = %profile.address,
            protocol = payload.reader.name(),
            block = payload.block_number,
            debt = %profile.debt_to_cover,
            collateral = ?profile.collateral_asset,
            seize = %profile.seize_amount,
            "📥 Liquidation payload received in execution pipeline"
        );

        // Step 0: Basic Parameter Guard
        if !Self::validate_profile(profile) {
            return;
        }

        // Step 1 & 2: Swap Quote & Economic Guard
        let quote = match self.fetch_and_validate_quote(profile).await {
            Some(q) => q,
            None => return,
        };

        let mut job = self.build_liquidation_job(profile, &quote);

        // Step 3: EVM Simulation
        let mut gas_used = match self.simulate_job(payload.block_number, &job).await {
            Some(gas) => gas,
            None => return,
        };

        // Step 4: JIT State Refresh Guard
        if !self.jit_refresh_guard(&payload, &mut job, &mut gas_used).await {
            return;
        }

        // Step 5: Broadcast Transaction
        self.execute_liquidation(&job, gas_used, start_time, payload.reader.name()).await;
    }

    // =========================================================================
    // Private Helper Functions
    // =========================================================================

    /// Step 0: Guard against zero amounts or malformed profiles
    fn validate_profile(profile: &BorrowerProfile) -> bool {
        if profile.seize_amount == 0.into() || profile.debt_to_cover == 0.into() {
            warn!(
                borrower = %profile.address,
                seize = %profile.seize_amount,
                debt = %profile.debt_to_cover,
                "❌ Rejected: Zero seize or debt amount"
            );
            return false;
        }
        true
    }

    /// Step 1 & 2: Fetches DEX quote and enforces the profitability threshold
    async fn fetch_and_validate_quote(&self, profile: &BorrowerProfile) -> Option<MarketQuote> {
        info!(
            borrower = %profile.address,
            collateral = ?profile.collateral_asset,
            debt = ?profile.debt_asset,
            seize = %profile.seize_amount,
            "Requesting swap quote"
        );

        let quote = match self
            .dex_finder
            .get_swap_quote(
                profile.collateral_asset,
                profile.debt_asset,
                profile.src_decimals,
                profile.dest_decimals,
                profile.seize_amount,
            )
            .await
        {
            Ok(q) => q,
            Err(e) => {
                warn!(
                    borrower = %profile.address,
                    error = ?e,
                    "❌ Rejected: DEX quote failed"
                );
                return None;
            }
        };

        // Economic Guard Check
        if quote.min_amt_out < profile.debt_to_cover {
            let shortfall = profile.debt_to_cover.saturating_sub(quote.min_amt_out);
            info!(
                borrower = %profile.address,
                debt = %profile.debt_to_cover,
                quote = %quote.min_amt_out,
                shortfall = %shortfall,
                "❌ Rejected: Unprofitable trade"
            );
            return None;
        }

        info!(
            borrower = %profile.address,
            min_out = %quote.min_amt_out,
            debt = %profile.debt_to_cover,
            profit = %quote.min_amt_out.saturating_sub(profile.debt_to_cover),
            "Quote received and validated"
        );

        Some(quote)
    }

    /// Constructs the initial LiquidationJob payload
    fn build_liquidation_job(&self, profile: &BorrowerProfile, quote: &MarketQuote) -> LiquidationJob {
        LiquidationJob {
            borrower: profile.address.clone(),
            debt_asset: profile.debt_asset,
            collateral_asset: profile.collateral_asset,
            debt_to_cover: profile.debt_to_cover,
            swap_target: quote.swap_target,
            swap_proxy: quote.token_transfer_proxy,
            swap_data: quote.swap_data.clone(),
            market_id: profile.market_id,
            repaid_shares: profile.repaid_shares,
            seized_assets: profile.seized_assets,
            protocol: profile.protocol,
            min_amt_out: quote.min_amt_out,
            identity: profile.identity.clone(),
            seize_amount: profile.seize_amount,
        }
    }

    /// Step 3: Runs EVM fork simulation against the target block number
    async fn simulate_job(&self, block_number: u64, job: &LiquidationJob) -> Option<u64> {
        info!(
            borrower = %job.borrower,
            block = block_number,
            "Starting EVM simulation"
        );

        match self.simulator.simulate_liquidation(block_number, job).await {
            Ok(gas) => {
                let profit = job.min_amt_out.saturating_sub(job.debt_to_cover);
                info!(
                    borrower = %job.borrower,
                    gas = gas,
                    profit = %profit,
                    "Simulation complete"
                );
                Some(gas)
            }
            Err(e) => {
                warn!(
                    borrower = %job.borrower,
                    error = ?e,
                    "❌ Rejected: Simulation reverted"
                );
                None
            }
        }
    }

    /// Step 4: Re-queries protocol state directly via `payload.reader` to detect and handle state changes
    async fn jit_refresh_guard(
        &self,
        payload: &LiqPayload,
        job: &mut LiquidationJob,
        gas_used: &mut u64,
    ) -> bool {
        match payload.reader.refresh_borrower(&job.identity).await {
            Ok(refreshed) => {
                let state_changed = refreshed.debt_to_cover != job.debt_to_cover
                    || refreshed.seize_amount != job.seize_amount;

                if !state_changed {
                    return true;
                }

                info!(
                    borrower = %job.borrower,
                    old_debt = %job.debt_to_cover,
                    new_debt = %refreshed.debt_to_cover,
                    "🔄 State shift detected! Fast-path re-quote & re-simulation"
                );

                match try_revalidate_and_simulate(
                    self.dex_finder.as_ref(),
                    self.simulator.as_ref(),
                    payload.block_number,
                    refreshed,
                    job.clone(),
                )
                .await
                {
                    Ok(updated_payload) => {
                        *job = updated_payload.job;
                        *gas_used = updated_payload.gas_used;
                        info!(borrower = %job.borrower, "Re-validation successful");
                        true
                    }
                    Err(e) => {
                        warn!(
                            borrower = %job.borrower,
                            error = ?e,
                            "🛑 Re-validation failed. Dropping transaction."
                        );
                        false
                    }
                }
            }
            Err(e) => {
                warn!(
                    borrower = %job.borrower,
                    error = ?e,
                    "🛑 Refresh failed or position healthy/closed. Aborting broadcast."
                );
                false
            }
        }
    }

    /// Step 5: Submits execution transaction to on-chain liquidator contract
    async fn execute_liquidation(
        &self,
        job: &LiquidationJob,
        gas_used: u64,
        start_time: Instant,
        protocol_name: &str,
    ) {
        info!(
            borrower = %job.borrower,
            protocol = protocol_name,
            gas_limit = gas_used,
            debt_to_cover = %job.debt_to_cover,
            elapsed_ms = start_time.elapsed().as_millis(),
            "🚀 Broadcasting liquidation transaction"
        );

        match self
            .liquidator
            .execute_liquidation(job.clone(), gas_used.into())
            .await
        {
            Ok(_) => {
                info!(
                    borrower = %job.borrower,
                    elapsed_ms = start_time.elapsed().as_millis(),
                    "✅ Liquidation submitted and confirmed"
                );
            }
            Err(e) => {
                error!(
                    borrower = %job.borrower,
                    error = ?e,
                    "💥 Broadcasting failed at RPC layer"
                );
            }
        }
    }
}

/// Standalone re-validation helper
async fn try_revalidate_and_simulate(
    dex_finder: &dyn DexRouteFinder,
    simulator: &dyn EvmSimulator,
    _block_number: u64,
    refreshed: BorrowerProfile,
    mut job: LiquidationJob,
) -> anyhow::Result<TxPayload> {
    let new_quote = dex_finder
        .get_swap_quote(
            refreshed.collateral_asset,
            refreshed.debt_asset,
            refreshed.src_decimals,
            refreshed.dest_decimals,
            refreshed.seize_amount,
        )
        .await?;

    if new_quote.min_amt_out < refreshed.debt_to_cover {
        anyhow::bail!("Refreshed quote below debt to cover");
    }

    job.debt_to_cover = refreshed.debt_to_cover;
    job.seize_amount = refreshed.seize_amount;
    job.repaid_shares = refreshed.repaid_shares;
    job.seized_assets = refreshed.seized_assets;
    job.min_amt_out = new_quote.min_amt_out;
    job.swap_target = new_quote.swap_target;
    job.swap_proxy = new_quote.token_transfer_proxy;
    job.swap_data = new_quote.swap_data;

    let gas_used = simulator.simulate_liquidation(0, &job).await?;

    Ok(TxPayload { job, gas_used })
}