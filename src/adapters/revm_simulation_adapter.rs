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
use std::sync::{Arc, RwLock};
use tokio::time::{timeout, Duration};
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

/// Hard RPC timeout for snapshot creation to prevent hanging async tasks
const RPC_TIMEOUT: Duration = Duration::from_millis(800);

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

        let block_number = if block_number == 0 {
           timeout(RPC_TIMEOUT, self.provider.get_block_number())
            .await
            .map_err(|_| anyhow::anyhow!("RPC fetch timed out for get_block_number"))??
            .as_u64()
        } else {
            block_number
    };

        // Return cached snapshot directly if available for this specific block
        if let Some(snapshot) = self.snapshot_cache.get(&block_number) {
            trace!(block = block_number, "Cache hit for REVM block snapshot");
            return Ok(snapshot.clone());
        }

        debug!(
            block = block_number,
            "Building new REVM block snapshot from RPC"
        );

        // Wrap async RPC fetches with strict timeouts to avoid blocking the event loop
        let provider = self.provider.clone();
        let keeper_addr = constants::WALLET.address();

        let (block, nonce) = timeout(RPC_TIMEOUT, async {
            let block_fut = provider.get_block(block_number);
            let nonce_fut = provider.get_transaction_count(keeper_addr, Some(block_number.into()));
            tokio::try_join!(block_fut, nonce_fut)
        })
        .await
        .map_err(|_| anyhow::anyhow!("RPC fetch timed out for block {}", block_number))??;

        let block = block.ok_or_else(|| anyhow::anyhow!("Block {} not found on RPC", block_number))?;
        let nonce = nonce.as_u64();

        // Offload blocking state warming to a blocking worker thread pool.
        // `EthersDB` performs synchronous, blocking HTTP RPC calls under the hood!
        let provider_for_db = self.provider.clone();
        let snapshot = tokio::task::spawn_blocking(move || -> anyhow::Result<BlockSnapshot<M>> {
            let ethers_db = EthersDB::new(
                provider_for_db,
                Some(BlockId::Number(block_number.into())),
            )
            .ok_or_else(|| anyhow::anyhow!("Failed to initialize EthersDB for block {}", block_number))?;

            let shared_ethers = SharedEthersDB(Arc::new(RwLock::new(ethers_db)));
            let mut db = CacheDB::new(shared_ethers);

            // Pre-fund Keeper wallet in EVM memory state
            db.insert_account_info(
                keeper_addr.0.into(),
                AccountInfo {
                    balance: rU256::MAX,
                    nonce,
                    code_hash: KECCAK_EMPTY,
                    code: None,
                },
            );

            // Dynamic target set for warming state accounts
            let mut warm_targets = vec![
                *constants::FLASH_LIQUIDATOR,
                *constants::AAVE_V3_POOL,
                *constants::UNISWAPV3_ROUTER_02,
                *constants::MORPHO_BLUE,
                debt_asset,
                collateral_asset,
                borrower,
            ];

            // Avoid adding identical debt & collateral assets to warm target set
            if collateral_asset != debt_asset {
                warm_targets.push(collateral_asset);
            }

            // Perform warm fetches synchronously on the dedicated blocking worker thread
            for addr in warm_targets {
                let r_addr = addr.0.into();
                if let Ok(Some(info)) = db.basic(r_addr) {
                    if info.code_hash != KECCAK_EMPTY {
                        let _ = db.code_by_hash(info.code_hash);
                    }
                }
            }

            Ok(BlockSnapshot {
                block_number,
                block: Arc::new(block),
                db: Arc::new(db),
            })
        })
        .await
        .map_err(|e| anyhow::anyhow!("Snapshot construction worker thread panicked: {:?}", e))??;

        let snapshot = Arc::new(snapshot);

        // Retain current block and immediately discard outdated snapshots
        self.snapshot_cache
            .retain(|&cached_block, _| cached_block >= block_number.saturating_sub(1));
        self.snapshot_cache.insert(block_number, snapshot.clone());

        Ok(snapshot)
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
        // Issue #2 Bypass: Same-token collateral/debt self-swaps
        if job.collateral_asset == job.debt_asset {
            warn!(
                borrower = %job.borrower,
                asset = %job.debt_asset,
                "Bypassing EVM simulation: Collateral and debt assets are identical"
            );
            anyhow::bail!("Self-liquidation swap shortcut: collateral equals debt asset");
        }

        debug!(
            block = block_number,
            "Starting REVM simulation for liquidation job"
        );

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

        // Issue #1 Fix: Run pure simulation inside spawn_blocking pool with non-blocking join
        let sim_result = tokio::task::spawn_blocking(move || engine.simulate(&snapshot, sim_tx))
            .await
            .map_err(|e| anyhow::anyhow!("Blocking task execution failed: {:?}", e))??;

        if !sim_result.success {
            let reason = sim_result
                .revert_reason
                .unwrap_or_else(|| "Unknown revert".to_string());
            debug!(
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