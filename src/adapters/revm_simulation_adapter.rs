use async_trait::async_trait;
use dashmap::DashMap;
use ethers::{
    providers::Middleware,
    signers::Signer,
    types::{Address, BlockId},
};
use revm::{
    db::{CacheDB, DatabaseRef, EthersDB},
    primitives::{AccountInfo, KECCAK_EMPTY, U256 as rU256},
};
use std::sync::{Arc, Mutex};
use tracing::{debug, instrument, trace, warn};

use crate::simulation::{
    engine::{SimulationEngine, SimulationTx},
    snapshot::{BlockSnapshot, SharedEthersDB},
};
use crate::{
    common::abi_bindings::IFlashLiquidator,
    constants,
    core::{
        ports::EvmSimulator,
        types::{LiquidationJob, LiquidationParams},
    },
};

pub struct RevmAdapter<M>
where
    M: Middleware + Send + Sync + 'static,
    <M as Middleware>::Error: 'static,
{
    provider: Arc<M>,
    contract: Arc<IFlashLiquidator<M>>,
    engine: Arc<SimulationEngine>,
    snapshot_cache: DashMap<u64, Arc<BlockSnapshot<M>>>,
}

impl<M> RevmAdapter<M>
where
    M: Middleware + Send + Sync + 'static,
    <M as Middleware>::Error: 'static,
{
    pub fn new(provider: Arc<M>, contract: Arc<IFlashLiquidator<M>>) -> Self {
        Self {
            provider,
            contract,
            engine: Arc::new(SimulationEngine::new(constants::CHAIN_ID)),
            snapshot_cache: DashMap::new(),
        }
    }

    pub async fn build_snapshot(
        &self,
        block_number: u64,
        debt_asset: Address,
        collateral_asset: Address,
        borrower: Address,
    ) -> anyhow::Result<Arc<BlockSnapshot<M>>> {
        // Return cached snapshot directly if available for this block
        if let Some(snapshot) = self.snapshot_cache.get(&block_number) {
            trace!(block = block_number, "Cache hit for REVM block snapshot");
            return Ok(snapshot.clone());
        }

        debug!(block = block_number, "Building new REVM block snapshot from RPC");

        let block = self
            .provider
            .get_block(block_number)
            .await?
            .ok_or_else(|| anyhow::anyhow!("Block {} not found on RPC", block_number))?;

        let ethers_db = EthersDB::new(
            self.provider.clone(),
            Some(BlockId::Number(block_number.into())),
        )
        .ok_or_else(|| {
            anyhow::anyhow!("Failed to initialize EthersDB for block {}", block_number)
        })?;

        // Lock-free shared EthersDB wrapper
        let shared_ethers = SharedEthersDB(Arc::new(Mutex::new(ethers_db)));
        let mut db = CacheDB::new(shared_ethers);

        // Pre-fund Keeper wallet in the EVM state
        let keeper_addr = constants::WALLET.address();
        let nonce = self
            .provider
            .get_transaction_count(keeper_addr, Some(block_number.into()))
            .await?
            .as_u64();

        db.insert_account_info(
            keeper_addr.0.into(),
            AccountInfo {
                balance: rU256::MAX,
                nonce,
                code_hash: KECCAK_EMPTY,
                code: None,
            },
        );

        // Warm target accounts during initial construction
        let warm_targets = [
            *constants::FLASH_LIQUIDATOR,
            *constants::AAVE_V3_POOL,
            *constants::UNISWAPV3_ROUTER_02,
            *constants::MORPHO_BLUE,
            debt_asset,
            collateral_asset,
            borrower,
        ];
        self.warm_accounts(&mut db, &warm_targets);

        let snapshot = Arc::new(BlockSnapshot {
            block_number,
            block: Arc::new(block),
            db: Arc::new(db),
        });

        // Retain only current block and previous block snapshots
        self.snapshot_cache
            .retain(|&cached_block, _| cached_block >= block_number.saturating_sub(1));
        self.snapshot_cache.insert(block_number, snapshot.clone());

        Ok(snapshot)
    }

    fn warm_accounts(&self, db: &mut CacheDB<SharedEthersDB<M>>, targets: &[Address]) {
        for addr in targets {
            let r_addr = (*addr).0.into();
            if let Ok(Some(info)) = db.basic(r_addr) {
                if info.code_hash != KECCAK_EMPTY {
                    let _ = db.code_by_hash(info.code_hash);
                }
            }
        }
    }
}

#[async_trait]
impl<M> EvmSimulator for RevmAdapter<M>
where
    M: Middleware + Send + Sync + 'static,
    <M as Middleware>::Error: 'static,
{
    #[instrument(skip(self, job), fields(borrower = %job.borrower, debt = %job.debt_asset))]
    async fn simulate_liquidation(
        &self,
        block_number: u64,
        job: &LiquidationJob,
    ) -> anyhow::Result<u64> {
        debug!(block = block_number, "Starting REVM simulation for liquidation job");

        let snapshot = self
            .build_snapshot(
                block_number,
                job.debt_asset,
                job.collateral_asset,
                job.borrower,
            )
            .await?;

        let params = LiquidationParams::from(job.clone());
        let calldata = self
            .contract
            .execute_flash_liquidation(job.debt_to_cover, params)
            .calldata()
            .ok_or_else(|| anyhow::anyhow!("Failed to encode liquidation calldata"))?;

        let sim_tx = SimulationTx {
            from: constants::WALLET.address(),
            to: *constants::FLASH_LIQUIDATOR,
            calldata,
            value: ethers::types::U256::zero(),
        };

        let engine = self.engine.clone();

        // Execute REVM execution in spawn_blocking pool
        let sim_result = tokio::task::spawn_blocking(move || engine.simulate(&snapshot, sim_tx))
            .await
            .map_err(|e| anyhow::anyhow!("Blocking task spawn failed: {:?}", e))??;

        if !sim_result.success {
            let reason = sim_result.revert_reason.unwrap_or_else(|| "Unknown revert".to_string());
            warn!(
                block = block_number,
                borrower = %job.borrower,
                revert_reason = %reason,
                "❌ REVM Liquidation Simulation REVERTED"
            );
            anyhow::bail!("Simulation reverted: {}", reason);
        }

        debug!(
            block = block_number,
            gas_used = sim_result.gas_used,
            "✅ REVM Liquidation Simulation SUCCESSFUL"
        );

        Ok(sim_result.gas_used)
    }
}