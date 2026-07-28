// src/core/engine.rs

use crate::core::ports::{
    DexRouteFinder, EvmSimulator, LendingProtocolReader, LiquidationContract,
};
use crate::core::types::{BorrowerProfile, LiquidationJob, Protocol, TxPayload};
use futures_util::{
    future::join_all,
    stream::{self, StreamExt},
};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Instant;
use tokio::sync::mpsc;
use tracing::{debug, error, info, warn};

/// Tracks pipeline progress and bottlenecks across the entire cycle
#[derive(Default)]
struct CycleStats {
    candidates: AtomicUsize,
    quoted: AtomicUsize,
    quote_failed: AtomicUsize,
    unprofitable: AtomicUsize,
    simulated: AtomicUsize,
    simulation_failed: AtomicUsize,
    queued: AtomicUsize,
    jit_failed: AtomicUsize,
    submitted: AtomicUsize,
    confirmed: AtomicUsize,
}

#[derive(Clone)]
pub struct LiquidationEngine {
    pub protocol_readers: Vec<Arc<dyn LendingProtocolReader + Send + Sync>>,
    pub dex_finder: Arc<dyn DexRouteFinder + Send + Sync>,
    pub simulator: Arc<dyn EvmSimulator + Send + Sync>,
    pub liquidator: Arc<dyn LiquidationContract + Send + Sync>,
}

impl LiquidationEngine {
    pub fn new(
        protocol_readers: Vec<Arc<dyn LendingProtocolReader + Send + Sync>>,
        dex_finder: Arc<dyn DexRouteFinder + Send + Sync>,
        simulator: Arc<dyn EvmSimulator + Send + Sync>,
        liquidator: Arc<dyn LiquidationContract + Send + Sync>,
    ) -> Self {
        Self {
            protocol_readers,
            dex_finder,
            simulator,
            liquidator,
        }
    }

    pub async fn run_liquidation_cycle(&self, block_number: u64) -> anyhow::Result<()> {
        let cycle_start = Instant::now();
        let stats = Arc::new(CycleStats::default());

        info!(block = block_number, "Starting liquidation cycle");

        // ---------------------------------------------------------------------
        // Stage 1: Instrumented Protocol Candidate Discovery
        // ---------------------------------------------------------------------
        let stage_timer = Instant::now();

        let fetch_futures = self
            .protocol_readers
            .iter()
            .cloned()
            .map(|reader| async move {
                let start = Instant::now();

                match reader.fetch_liquidation_candidates().await {
                    Ok(candidates) => {
                        info!(
                            protocol = reader.name(),
                            candidates = candidates.len(),
                            elapsed_ms = start.elapsed().as_millis(),
                            "Protocol scan complete"
                        );
                        candidates
                    }
                    Err(e) => {
                        error!(
                            protocol = reader.name(),
                            error = ?e,
                            "Protocol scan failed"
                        );
                        Vec::new()
                    }
                }
            });

        let candidates: Vec<BorrowerProfile> = join_all(fetch_futures)
            .await
            .into_iter()
            .flatten()
            .collect();

        stats.candidates.store(candidates.len(), Ordering::Relaxed);

        info!(
            total_candidates = candidates.len(),
            elapsed_ms = stage_timer.elapsed().as_millis(),
            "Candidate discovery complete"
        );

        if candidates.is_empty() {
            info!(
                block = block_number,
                candidates = 0,
                quoted = 0,
                quote_failed = 0,
                unprofitable = 0,
                simulated = 0,
                simulation_failed = 0,
                queued = 0,
                jit_failed = 0,
                submitted = 0,
                confirmed = 0,
                elapsed_ms = cycle_start.elapsed().as_millis(),
                "Liquidation cycle summary"
            );
            return Ok(());
        }

        // ---------------------------------------------------------------------
        // Stage 2: Adaptive Transaction Manager Task
        // ---------------------------------------------------------------------
        let (tx_sender, mut tx_receiver) = mpsc::channel::<TxPayload>(100);
        let liquidator = self.liquidator.clone();
        let protocol_readers = self.protocol_readers.clone();
        let dex_finder = self.dex_finder.clone();
        let simulator = self.simulator.clone();
        let tx_stats = Arc::clone(&stats);

        let tx_manager_task = tokio::spawn(async move {
            while let Some(mut payload) = tx_receiver.recv().await {
                let job = &payload.job;

                // --- Ultra-Low Latency State Refresh ---
                if let Some(reader) = protocol_readers
                    .iter()
                    .find(|r| r.name() == protocol_name(job.protocol))
                {
                    match reader.refresh_borrower(&job.identity).await {
                        Ok(refreshed) => {
                            let state_changed = refreshed.debt_to_cover != job.debt_to_cover
                                || refreshed.seize_amount != job.seize_amount;

                            if state_changed {
                                info!(
                                    borrower = %job.borrower,
                                    old_debt = %job.debt_to_cover,
                                    new_debt = %refreshed.debt_to_cover,
                                    "🔄 State shift detected! Triggering fast-path re-quote & re-simulation"
                                );

                                match try_revalidate_and_simulate(
                                    dex_finder.as_ref(),
                                    simulator.as_ref(),
                                    refreshed,
                                    job.clone(),
                                    block_number,
                                )
                                .await
                                {
                                    Ok(updated_payload) => {
                                        payload = updated_payload;
                                        debug!(
                                            borrower = %payload.job.borrower,
                                            "Re-validation successful"
                                        );
                                    }
                                    Err(e) => {
                                        tx_stats.jit_failed.fetch_add(1, Ordering::Relaxed);
                                        info!(
                                            borrower = %job.borrower,
                                            error = ?e,
                                            "🛑 Re-validation failed. Dropping transaction."
                                        );
                                        continue;
                                    }
                                }
                            }
                        }
                        Err(e) => {
                            tx_stats.jit_failed.fetch_add(1, Ordering::Relaxed);
                            warn!(
                                borrower = %job.borrower,
                                error = ?e,
                                "🛑 Refresh failed or position healthy/closed. Aborting broadcast."
                            );
                            continue;
                        }
                    }
                }

                // --- Broadcast Transaction ---
                tx_stats.submitted.fetch_add(1, Ordering::Relaxed);

                info!(
                    borrower = %payload.job.borrower,
                    protocol = ?payload.job.protocol,
                    gas_limit = payload.gas_used,
                    debt_to_cover = %payload.job.debt_to_cover,
                    "Broadcasting liquidation"
                );

                match liquidator
                    .execute_liquidation(payload.job.clone(), payload.gas_used.into())
                    .await
                {
                    Ok(_) => {
                        tx_stats.confirmed.fetch_add(1, Ordering::Relaxed);
                        info!(borrower = %payload.job.borrower, "Liquidation confirmed");
                    }
                    Err(e) => {
                        error!(borrower = %payload.job.borrower, error = ?e, "Broadcasting failed");
                    }
                }
            }
        });

        // ---------------------------------------------------------------------
        // Stage 3: Candidate Pipeline (Routing -> Sim -> Queue)
        // ---------------------------------------------------------------------
        stream::iter(candidates.clone())
            .for_each_concurrent(10, |borrower| {
                let tx_sender = tx_sender.clone();
                let dex_finder = self.dex_finder.clone();
                let simulator = self.simulator.clone();
                let stats = Arc::clone(&stats);

                async move {
                    info!(
                        borrower = %borrower.address,
                        protocol = ?borrower.protocol,
                        debt = %borrower.debt_to_cover,
                        collateral = ?borrower.collateral_asset,
                        seize = %borrower.seize_amount,
                        "Borrower entered pipeline"
                    );

                    // Step A: Request Quote
                    debug!(
                        borrower = %borrower.address,
                        collateral = ?borrower.collateral_asset,
                        debt = ?borrower.debt_asset,
                        seize = %borrower.seize_amount,
                        "Requesting swap quote"
                    );

                    stats.quoted.fetch_add(1, Ordering::Relaxed);

                    let quote = match dex_finder
                        .get_swap_quote(
                            borrower.collateral_asset,
                            borrower.debt_asset,
                            borrower.src_decimals,
                            borrower.dest_decimals,
                            borrower.seize_amount,
                        )
                        .await
                    {
                        Ok(q) => {
                            info!(
                                borrower = %borrower.address,
                                min_out = %q.min_amt_out,
                                debt = %borrower.debt_to_cover,
                                profit = %q.min_amt_out.saturating_sub(borrower.debt_to_cover),
                                "Quote received"
                            );
                            q
                        }
                        Err(e) => {
                            stats.quote_failed.fetch_add(1, Ordering::Relaxed);
                            info!(
                                borrower = %borrower.address,
                                error = ?e,
                                reason = "DEX quote request failed",
                                "Borrower rejected"
                            );
                            return;
                        }
                    };

                    // Step B: Pricing Economic Guard
                    if quote.min_amt_out < borrower.debt_to_cover {
                        stats.unprofitable.fetch_add(1, Ordering::Relaxed);
                        let shortfall = borrower.debt_to_cover.saturating_sub(quote.min_amt_out);

                        info!(
                            borrower = %borrower.address,
                            debt = %borrower.debt_to_cover,
                            quote = %quote.min_amt_out,
                            shortfall = %shortfall,
                            reason = "quote below debt",
                            "Borrower rejected"
                        );
                        return;
                    }

                    let job = LiquidationJob {
                        borrower: borrower.address,
                        debt_asset: borrower.debt_asset,
                        collateral_asset: borrower.collateral_asset,
                        debt_to_cover: borrower.debt_to_cover,
                        swap_target: quote.swap_target,
                        swap_proxy: quote.token_transfer_proxy,
                        swap_data: quote.swap_data,
                        market_id: borrower.market_id,
                        repaid_shares: borrower.repaid_shares,
                        seized_assets: borrower.seized_assets,
                        protocol: borrower.protocol,
                        min_amt_out: quote.min_amt_out,
                        identity: borrower.identity,
                        seize_amount: borrower.seize_amount,
                    };

                    // Step C: EVM Simulation
                    stats.simulated.fetch_add(1, Ordering::Relaxed);

                    info!(
                        borrower = %borrower.address,
                        protocol = protocol_name(borrower.protocol),
                        "Starting EVM simulation"
                    );

                    let gas_used = match simulator.simulate_liquidation(block_number, &job).await {
                        Ok(gas) => {
                            let profit = quote.min_amt_out.saturating_sub(borrower.debt_to_cover);
                            info!(
                                borrower = %borrower.address,
                                gas = gas,
                                profit = %profit,
                                "Simulation complete"
                            );
                            gas
                        }
                        Err(e) => {
                            stats.simulation_failed.fetch_add(1, Ordering::Relaxed);
                            warn!(
                                borrower = %borrower.address,
                                error = ?e,
                                "Simulation reverted"
                            );
                            return;
                        }
                    };

                    // Step D: Queue for Dispatch
                    stats.queued.fetch_add(1, Ordering::Relaxed);

                    info!(
                        borrower = %borrower.address,
                        protocol = ?borrower.protocol,
                        gas = gas_used,
                        "Queueing liquidation payload"
                    );

                    let payload = TxPayload { job, gas_used };
                    if let Err(e) = tx_sender.send(payload).await {
                        error!(
                            borrower = %borrower.address,
                            error = ?e,
                            "Failed to deliver payload to transaction manager channel"
                        );
                    }
                }
            })
            .await;

        // ---------------------------------------------------------------------
        // Stage 4: Execution Drain & Comprehensive Cycle Summary
        // ---------------------------------------------------------------------
        drop(tx_sender);
        let _ = tx_manager_task.await;

        info!(
            block = block_number,
            candidates = stats.candidates.load(Ordering::Relaxed),
            quoted = stats.quoted.load(Ordering::Relaxed),
            quote_failed = stats.quote_failed.load(Ordering::Relaxed),
            unprofitable = stats.unprofitable.load(Ordering::Relaxed),
            simulated = stats.simulated.load(Ordering::Relaxed),
            simulation_failed = stats.simulation_failed.load(Ordering::Relaxed),
            queued = stats.queued.load(Ordering::Relaxed),
            jit_failed = stats.jit_failed.load(Ordering::Relaxed),
            submitted = stats.submitted.load(Ordering::Relaxed),
            confirmed = stats.confirmed.load(Ordering::Relaxed),
            elapsed_ms = cycle_start.elapsed().as_millis(),
            "Liquidation cycle summary"
        );

        Ok(())
    }
}

/// Re-evaluates an updated borrower profile by requesting a fresh DEX quote
/// and running an EVM simulation. Returns the updated payload if still viable and profitable.
async fn try_revalidate_and_simulate(
    dex_finder: &dyn DexRouteFinder,
    simulator: &dyn EvmSimulator,
    refreshed: BorrowerProfile,
    mut job: LiquidationJob,
    block_number: u64,
) -> anyhow::Result<TxPayload> {
    // 1. Fast-Path Re-Quote
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

    // Update job parameters
    job.debt_to_cover = refreshed.debt_to_cover;
    job.seize_amount = refreshed.seize_amount;
    job.repaid_shares = refreshed.repaid_shares;
    job.seized_assets = refreshed.seized_assets;
    job.min_amt_out = new_quote.min_amt_out;
    job.swap_target = new_quote.swap_target;
    job.swap_proxy = new_quote.token_transfer_proxy;
    job.swap_data = new_quote.swap_data;

    // 2. Fast-Path Re-Simulation
    let gas_used = simulator.simulate_liquidation(block_number, &job).await?;

    Ok(TxPayload { job, gas_used })
}

fn protocol_name(protocol: Protocol) -> &'static str {
    match protocol {
        Protocol::Aave => "aave",
        Protocol::Morpho => "morpho",
    }
}
