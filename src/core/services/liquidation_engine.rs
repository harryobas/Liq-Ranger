use crate::common::get_token_symbol;
use crate::core::ports::{DexRouteFinder, EvmSimulator, LiquidationContract};
use crate::core::types::{BorrowerProfile, LiqPayload, LiquidationJob, MarketQuote, TxPayload};
use std::sync::Arc;
use std::time::Instant;
use ethers::providers::Middleware;
use tokio::sync::{mpsc, watch, Semaphore};
use tracing::{error, info, warn};

#[derive(Clone)]
pub struct PipelineEngine<M> {
    pub dex_finder: Arc<dyn DexRouteFinder + Send + Sync>,
    pub simulator: Arc<dyn EvmSimulator + Send + Sync>,
    pub liquidator: Arc<dyn LiquidationContract + Send + Sync>,
    pub provider: Arc<M>
}

impl<M: Middleware + 'static> PipelineEngine<M> {
    pub fn new(
        dex_finder: Arc<dyn DexRouteFinder + Send + Sync>,
        simulator: Arc<dyn EvmSimulator + Send + Sync>,
        liquidator: Arc<dyn LiquidationContract + Send + Sync>,
        provider: Arc<M>
    ) -> Self {
        Self {
            dex_finder,
            simulator,
            liquidator,
            provider
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

         let (debt_sym, coll_sym) = tokio::try_join!(
            get_token_symbol(profile.debt_asset, self.provider.clone()),
            get_token_symbol(profile.collateral_asset, self.provider.clone())
        ).unwrap_or(("UNKNOWN".to_string(), "UNKNOWN".to_string()));

        let debt_fmt = format_token_amount(profile.debt_to_cover, profile.dest_decimals, &debt_sym);
        let seize_fmt = format_token_amount(profile.seize_amount, profile.src_decimals, &coll_sym);

        info!(
            borrower = %profile.address,
            protocol = payload.reader.name(),
            block = payload.block_number,
            pair = %format!("{} -> {}", coll_sym, debt_sym),
            debt = %debt_fmt,
            seize = %seize_fmt,
            "📥 Liquidation payload received in execution pipeline"
        );


        // Step 0: Basic Parameter Guard
        if !Self::validate_profile(profile, &coll_sym, &debt_sym) {
            return;
        }


        // Step 1 & 2: Swap Quote & Economic Guard
        let quote = match self.fetch_and_validate_quote(profile, &coll_sym, &debt_sym).await {
            Some(q) => q,
            None => return,
        };

        let mut job = self.build_liquidation_job(profile, &quote);

        // Step 3: EVM Simulation
        let mut gas_used = match self.simulate_job(payload.block_number, &job, &debt_sym, profile.dest_decimals).await {
            Some(gas) => gas,
            None => return,
        };

        // Step 4: JIT State Refresh Guard
        if !self.jit_refresh_guard(&payload, &mut job, &mut gas_used, &debt_sym, profile.dest_decimals).await {
            return;
        }

        // Step 5: Broadcast Transaction
        self.execute_liquidation(&job, gas_used, start_time, payload.reader.name(), &debt_sym, profile.dest_decimals).await;
    }


    // =========================================================================
    // Private Helper Functions
    // =========================================================================

    /// Step 0: Guard against zero amounts or malformed profiles
    fn validate_profile(profile: &BorrowerProfile, coll_sym: &str, debt_sym: &str) -> bool {
        if profile.seize_amount.is_zero() || profile.debt_to_cover.is_zero() {
            let seize_fmt = format_token_amount(profile.seize_amount, profile.src_decimals, coll_sym);
            let debt_fmt = format_token_amount(profile.debt_to_cover, profile.dest_decimals, debt_sym);

            warn!(
                borrower = %profile.address,
                seize = %seize_fmt,
                debt = %debt_fmt,
                "❌ Rejected: Zero seize or debt amount"
            );
            return false;
        }
        true
    }


    /// Step 1 & 2: Fetches DEX quote and enforces the profitability threshold
    async fn fetch_and_validate_quote(
        &self,
        profile: &BorrowerProfile,
        coll_sym: &str,
        debt_sym: &str,
    ) -> Option<MarketQuote> {
        let seize_fmt = format_token_amount(profile.seize_amount, profile.src_decimals, coll_sym);

        info!(
            borrower = %profile.address,
            pair = %format!("{} -> {}", coll_sym, debt_sym),
            seize = %seize_fmt,
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
            let debt_fmt = format_token_amount(profile.debt_to_cover, profile.dest_decimals, debt_sym);
            let quote_fmt = format_token_amount(quote.min_amt_out, profile.dest_decimals, debt_sym);
            let shortfall_fmt = format_token_amount(shortfall, profile.dest_decimals, debt_sym);

            info!(
                borrower = %profile.address,
                debt = %debt_fmt,
                quote = %quote_fmt,
                shortfall = %shortfall_fmt,
                "❌ Rejected: Unprofitable trade"
            );
            return None;
        }

        let min_out_fmt = format_token_amount(quote.min_amt_out, profile.dest_decimals, debt_sym);
        let debt_fmt = format_token_amount(profile.debt_to_cover, profile.dest_decimals, debt_sym);
        let profit = quote.min_amt_out.saturating_sub(profile.debt_to_cover);
        let profit_fmt = format_token_amount(profit, profile.dest_decimals, debt_sym);

        info!(
            borrower = %profile.address,
            min_out = %min_out_fmt,
            debt = %debt_fmt,
            profit = %profit_fmt,
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
    async fn simulate_job(
        &self,
        block_number: u64,
        job: &LiquidationJob,
        debt_sym: &str,
        debt_decimals: u8,
    ) -> Option<u64> {
        info!(
            borrower = %job.borrower,
            block = block_number,
            "Starting EVM simulation"
        );

        match self.simulator.simulate_liquidation(block_number, job).await {
            Ok(gas) => {
                let profit = job.min_amt_out.saturating_sub(job.debt_to_cover);
                let profit_fmt = format_token_amount(profit, debt_decimals, debt_sym);
                info!(
                    borrower = %job.borrower,
                    gas = gas,
                    profit = %profit_fmt,
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
        debt_sym: &str,
        debt_decimals: u8,
    ) -> bool {
        match payload.reader.refresh_borrower(&job.identity).await {
            Ok(refreshed) => {
                let state_changed = refreshed.debt_to_cover != job.debt_to_cover
                    || refreshed.seize_amount != job.seize_amount;

                if !state_changed {
                    return true;
                }

                let old_debt_fmt = format_token_amount(job.debt_to_cover, debt_decimals, debt_sym);
                let new_debt_fmt = format_token_amount(refreshed.debt_to_cover, debt_decimals, debt_sym);

                info!(
                    borrower = %job.borrower,
                    old_debt = %old_debt_fmt,
                    new_debt = %new_debt_fmt,
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
        debt_sym: &str,
        debt_decimals: u8,
    ) {
        let debt_fmt = format_token_amount(job.debt_to_cover, debt_decimals, debt_sym);

        info!(
            borrower = %job.borrower,
            protocol = protocol_name,
            gas_limit = gas_used,
            debt_to_cover = %debt_fmt,
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

use ethers::{utils::format_units, types::U256};

fn format_token_amount(amount: U256, decimals: u8, symbol: &str) -> String {
    if amount.is_zero() {
        return format!("0 {}", symbol);
    }

    match format_units(amount, decimals as u32) {
        Ok(formatted) => {
            // Trim to max 4 decimal places for clean log scannability
            let formatted_str = if let Some((integer, fractional)) = formatted.split_once('.') {
                let truncated = &fractional[..fractional.len().min(4)];
                let trimmed = truncated.trim_end_matches('0');
                if trimmed.is_empty() {
                    integer.to_string()
                } else {
                    format!("{}.{}", integer, trimmed)
                }
            } else {
                formatted
            };

            format!("{} {}", formatted_str, symbol)
        }
        Err(_) => format!("{} {}", amount, symbol),
    }
}