use std::{
    collections::HashSet, 
    str::FromStr, 
    sync::{Arc, atomic::{AtomicBool, Ordering}}
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

use sqlx::Row;

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
    pool: sqlx::Pool<sqlx::Sqlite>,
}

impl<M: Middleware + 'static> ProfitDistributor<M> {
    const CONFIRMATIONS: usize = 3;

    pub fn new(
        client: Arc<M>,
        contract: Arc<IFlashLiquidator<M>>,
        pool: sqlx::Pool<sqlx::Sqlite>,
    ) -> Self {
        Self {
            client,
            contract,
            running: AtomicBool::new(false),
            pool,
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

        let accumulated_wpol = self.contract.accumulated_profits(*constants::WPOL).call().await?;
        let refuel_amt = constants::REFUEL_AMT.saturating_sub(gas_balance);

        if accumulated_wpol < refuel_amt {
        tracing::warn!(
            "⚠️ Gas low, but contract only has {} WPOL (need {}). Skipping refuel.",
            format_ether(accumulated_wpol),
            format_ether(refuel_amt)
        );
        return Ok(());
        }

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
        let active_assets = self.discover_active_assets().await?;

        tracing::info!("🔍 Scanning {} unique assets for profits...", active_assets.len());


        for asset in active_assets {
            let profit_amount = self
                .contract
                .accumulated_profits(asset)
                .call()
                .await?;

            if profit_amount.is_zero() {
                continue;
            }

            if asset == *constants::WPOL {
                // Skip WPOL since it's used for gas
                tracing::info!(
                    "💰 Skipping WPOL profit of {} (used for gas)",
                    format_ether(profit_amount)
                );
                continue;
            }

            let breet_addr = self.breet_address_for(asset);
            let asset_sym = common::get_token_symbol(
                asset, 
                self.client.clone()
            )
            .await?;

            tracing::info!(
                "💰 Distributing {} of asset {:?}",
                profit_amount,
                asset_sym
            );

            let call = self.contract.distribute_profits(asset, breet_addr);
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

    async fn discover_active_assets(&self) -> Result<HashSet<Address>> {
        let mut assets  = HashSet::new();

        assets.extend(constants::PROFIT_DIST_ASSETS.iter().cloned());

        let sql = "
        SELECT profit_asset FROM liquidations 
        UNION 
        SELECT collateral_asset FROM liquidations";

        let rows = sqlx::query(sql).fetch_all(&self.pool).await?;

        for row in rows {
            if let Some(addr_str) = row.try_get::<String, _>("profit_asset").ok() {
                 if let Ok(addr) = Address::from_str(&addr_str) {
                    assets.insert(addr);
                }
            }
        }

        Ok(assets)

    }

    /// Resolve Breet address
    fn breet_address_for(&self, asset: Address) -> Address {
        if asset == *constants::USDC || asset == *constants::USDT {
            *constants::BREET
        } else {
            Address::zero()
        }
    }
}