// src/core/engine.rs

use crate::core::ports::{
    DexRouteFinder, EvmSimulator, LendingProtocolReader, LiquidationContract,
};
use crate::core::types::{BorrowerProfile, LiquidationJob, TxPayload};
use futures_util::{
    future::join_all,
    stream::{self, StreamExt},
};
use std::sync::Arc;
use tokio::sync::mpsc;

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
        tracing::info!("Starting liquidation cycle for block: {}", block_number);

        // 1. Concurrent Protocol Intake Plane
        // Using join_all over standard iterator to avoid higher-ranked trait bound (HRTB) lifetime errors
        let fetch_futures = self
            .protocol_readers
            .iter()
            .cloned()
            .map(|reader| async move {
                match reader.fetch_liquidation_candidates().await {
                    Ok(candidates) => candidates,
                    Err(e) => {
                        tracing::error!(
                            "Failed to fetch candidates from protocol adapter: {:?}",
                            e
                        );
                        Vec::new()
                    }
                }
            });

        let all_candidates: Vec<BorrowerProfile> = join_all(fetch_futures)
            .await
            .into_iter()
            .flatten()
            .collect();

        if all_candidates.is_empty() {
            tracing::debug!("No unhealthy positions found across monitored protocols.");
            return Ok(());
        }

        // 2. High-capacity MPSC Channel for payload delivery
        let (tx_sender, mut tx_receiver) = mpsc::channel::<TxPayload>(100);

        // 3. Spawn Transaction Manager Worker (Consumer Loop)
        let liquidator = self.liquidator.clone();
        let tx_manager_task = tokio::spawn(async move {
            while let Some(payload) = tx_receiver.recv().await {
                tracing::info!(
                    borrower = ?payload.job.borrower,
                    gas_used = payload.gas_used,
                    "Dispatching liquidation payload to mempool..."
                );

                if let Err(e) = liquidator
                    .execute_liquidation(payload.job, payload.gas_used.into())
                    .await
                {
                    tracing::error!("Mempool broadcast execution failed: {:?}", e);
                }
            }
        });

        // 4. Concurrent Processing Pipeline (Producer Loop)
        stream::iter(all_candidates)
            .for_each_concurrent(2, |borrower| {
                let tx_sender = tx_sender.clone();
                async move {
                    let src_decimals = borrower.src_decimals;
                    let dest_decimals = borrower.dest_decimals;

                    // Step A: Async DEX Route Finding
                    let quote = match self.dex_finder.get_swap_quote(
                        borrower.collateral_asset,
                        borrower.debt_asset,
                        src_decimals,
                        dest_decimals,
                        borrower.seize_amount,
                    ).await {
                        Ok(q) => q,
                        Err(e) => {
                            tracing::debug!("No routing found for target account {:?}: {:?}", borrower.address, e);
                            return;
                        }
                    };

                    // Step B: Gross Economic Guard
                    if quote.min_amt_out < borrower.debt_to_cover {
                        tracing::debug!(
                            "Skipping target {:?}: expected swap yield {} insufficient to clear debt {}",
                            borrower.address, quote.min_amt_out, borrower.debt_to_cover
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
                    };

                    // Step C: Concurrent EVM State Fork Simulation
                    let gas_used = match self.simulator.simulate_liquidation(block_number, &job).await {
                        Ok(gas) => gas,
                        Err(e) => {
                            tracing::warn!("Simulation failed for target {:?}: {:?}", borrower.address, e);
                            return;
                        }
                    };

                    // Step D: Dispatch to Transaction Manager
                    tracing::info!(?borrower.address, gas_used, "Simulation successful. Queueing job...");
                    let payload = TxPayload { job, gas_used };
                    if let Err(e) = tx_sender.send(payload).await {
                        tracing::error!("Failed to route job to execution task channel: {:?}", e);
                    }
                }
            })
            .await;

        // 5. Drop master channel producer so worker task completes naturally
        drop(tx_sender);

        if let Err(e) = tx_manager_task.await {
            tracing::error!("Transaction manager task panicked: {:?}", e);
        }

        Ok(())
    }
}
