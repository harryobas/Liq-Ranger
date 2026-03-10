use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

use anyhow::{Result, Context};
use ethers::{
    signers::Signer,
    providers::Middleware,
    types::Address,
    utils::format_ether,
};
use tokio_cron_scheduler::{Job, JobScheduler};

use crate::{
    common::{self, abi_bindings::IFlashLiquidator},
    constants,
};

/// ProfitDistributor agent with cron-based weekly execution
///
/// - Reorg safe (waits confirmations)
/// - Non-overlapping
/// - Production-safe settlement executor
///
pub struct ProfitDistributor<M: Middleware + 'static> {
    client: Arc<M>,
    contract: Arc<IFlashLiquidator<M>>,
    running: AtomicBool,
}

impl<M: Middleware + 'static> ProfitDistributor<M> {
    const CONFIRMATIONS: usize = 3;

    pub fn new(
        client: Arc<M>,
        contract: Arc<IFlashLiquidator<M>>,
    ) -> Self {
        Self {
            client,
            contract,
            running: AtomicBool::new(false),
        }
    }

    /// Start weekly cron job
    pub async fn start(self: Arc<Self>) -> Result<()> {
        let sched = JobScheduler::new().await?;

        // Every Sunday at 02:00 UTC
        let job = Job::new_async("0 0 2 * * Sun", move |_uuid, _l| {
            let distributor = self.clone();

            Box::pin(async move {
                if distributor
                    .running
                    .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
                    .is_err()
                {
                    tracing::warn!("⚠️ Previous ProfitDistributor run still active. Skipping.");
                    return;
                }

                if let Err(e) = distributor.execute().await {
                    tracing::error!("ProfitDistributor execution failed: {:?}", e);
                }

                distributor.running.store(false, Ordering::SeqCst);
            })
        })?;

        sched.add(job).await?;
        sched.start().await?;

        tracing::info!("📅 ProfitDistributor scheduled (Sunday 02:00 UTC)");

        Ok(())
    }

    /// Main logic
    async fn execute(&self) -> Result<()> {
        tracing::info!("📦 Weekly ProfitDistributor triggered");

        self.ensure_gas_balance().await?;
        self.distribute_all_assets().await?;

        tracing::info!("✅ Weekly settlement completed.");
        Ok(())
    }

    /// ---------------------------
    /// Gas Top-Up Logic
    /// ---------------------------
    async fn ensure_gas_balance(&self) -> Result<()> {
        let gas_balance = self
            .client
            .get_balance(constants::WALLET.address(), None)
            .await?;

        if gas_balance >= *constants::GAS_THRESHOLD {
            return Ok(());
        }

        tracing::info!(
            "⛽ Gas balance {} below threshold {}. Refilling...",
            format_ether(gas_balance),
            format_ether(*constants::GAS_THRESHOLD)
        );

        let refuel_amt = constants::REFUEL_AMT.saturating_sub(gas_balance);

        if refuel_amt.is_zero() {
            return Ok(());
        }

        let call = self.contract
            .refuel_gas(refuel_amt);
    
        let pending = call.send().await.context("Refuel tx submission failed")?;


        let receipt = pending
            .confirmations(Self::CONFIRMATIONS)
            .await
            .context("Refuel tx confirmation failed")?
            .ok_or_else(|| anyhow::anyhow!("Refuel transaction dropped or reorged"))?;


        tracing::info!(
            "✅ Gas refueled. Tx: {:?}",
            receipt.transaction_hash
        );

        Ok(())
    }

    /// ---------------------------
    /// Profit Distribution Logic
    /// ---------------------------
    async fn distribute_all_assets(&self) -> Result<()> {
        for asset in constants::PROFIT_DIST_ASSETS.iter() {
            let profit_amount = self
                .contract
                .accumulated_profits(*asset)
                .call()
                .await?;

            if profit_amount.is_zero() {
                continue;
            }

            let breet_addr = self.breet_address_for(*asset);
            let asset_sym = common::get_token_symbol(
                *asset, 
                self.client.clone()
            )
            .await?;

            tracing::info!(
                "💰 Distributing {} of asset {:?}",
                profit_amount,
                asset_sym
            );

            let call = self.contract.distribute_profits(*asset, breet_addr);
            let pending = call.send().await.context("Distribution tx submission failed")?;

            let receipt = pending
                .confirmations(Self::CONFIRMATIONS)
                .await
                .context("Distribution tx dropped or reorged")?
                .ok_or_else(|| anyhow::anyhow!("Transaction dropped or reorged"))?;

            tracing::info!(
                "✅ Distribution confirmed. Tx: {:?}",
                receipt.transaction_hash
            );
        }

        Ok(())
    }

    /// Resolve Breet address
    fn breet_address_for(&self, asset: Address) -> Address {
        if asset == *constants::USDC {
            *constants::BREET_USDC
        } else if asset == *constants::USDT {
            *constants::BREET_USDT
        } else {
            Address::zero()
        }
    }
}