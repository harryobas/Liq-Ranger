use async_trait::async_trait;
use dashmap::DashMap;
use ethers::{
    providers::Middleware,
    signers::Signer,
    types::{Address, BlockId},
};
use revm::{
    db::{CacheDB, EthersDB},
    primitives::{AccountInfo, Bytecode, KECCAK_EMPTY, U256 as rU256},
};
use std::sync::{Arc, RwLock};
use tokio::time::{timeout, Duration};
use tracing::{debug, error, instrument, trace, warn};

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

/// Hard budget for snapshot creation & RPC warming
const SNAPSHOT_BUILD_TIMEOUT: Duration = Duration::from_millis(2000);
/// Individual RPC call timeout
const RPC_CALL_TIMEOUT: Duration = Duration::from_millis(800);

pub struct RevmAdapter<M>
where
    M: Middleware + Send + Sync + 'static,
    <M as Middleware>::Error: 'static,
{
    provider: Arc<M>,
    contract: Arc<IFlashLiquidator<M>>,
    engine: Arc<SimulationEngine>,
    snapshot_cache: DashMap<u64, Arc<BlockSnapshot<M>>>,
    code_cache: DashMap<Address, (AccountInfo, Option<Bytecode>)>,
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
            code_cache: DashMap::new(),
        }
    }

    pub async fn build_snapshot(
        &self,
        block_number: u64,
        debt_asset: Address,
        collateral_asset: Address,
        borrower: Address,
    ) -> anyhow::Result<Arc<BlockSnapshot<M>>> {
        // Determine target block height
        let resolved_block = if block_number == 0 {
            timeout(RPC_CALL_TIMEOUT, self.provider.get_block_number())
                .await
                .map_err(|_| anyhow::anyhow!("RPC timeout: get_block_number"))??
                .as_u64()
        } else {
            block_number
        };

        // Cache Hit Check
        if let Some(snapshot) = self.snapshot_cache.get(&resolved_block) {
            trace!(
                block = resolved_block,
                "Cache hit for REVM block snapshot"
            );
            return Ok(snapshot.clone());
        }

        debug!(
            block = resolved_block,
            "Building new REVM block snapshot from RPC"
        );

        // Run entire snapshot build within unified hard timeout budget
        let snapshot = timeout(
            SNAPSHOT_BUILD_TIMEOUT,
            self.build_snapshot_internal(resolved_block, debt_asset, collateral_asset, borrower),
        )
        .await
        .map_err(|_| {
            anyhow::anyhow!(
                "⏱️ Snapshot construction timed out exceeding {}ms budget for block {}",
                SNAPSHOT_BUILD_TIMEOUT.as_millis(),
                resolved_block
            )
        })??;

        let snapshot = Arc::new(snapshot);

        // Prune older cached snapshots to keep memory footprint bounded
        self.snapshot_cache
            .retain(|&cached_block, _| cached_block >= resolved_block.saturating_sub(1));
        self.snapshot_cache.insert(resolved_block, snapshot.clone());

        Ok(snapshot)
    }

    /// Internal async worker executing async RPC warmups BEFORE jumping into blocking REVM thread
    async fn build_snapshot_internal(
        &self,
        block_number: u64,
        debt_asset: Address,
        collateral_asset: Address,
        borrower: Address,
    ) -> anyhow::Result<BlockSnapshot<M>> {
        let provider = self.provider.clone();
        let keeper_addr = constants::WALLET.address();

        // 1. Concurrent fetch of Block Header & Keeper Nonce
        let block_fut = provider.get_block(block_number);
        let nonce_fut = provider.get_transaction_count(keeper_addr, Some(block_number.into()));

        let (block, nonce) = tokio::try_join!(block_fut, nonce_fut)?;
        let block = block.ok_or_else(|| anyhow::anyhow!("Block {} not found on RPC", block_number))?;
        let nonce = nonce.as_u64();

        // 2. Identify addresses to warm up
        let mut warm_targets = vec![
            *constants::FLASH_LIQUIDATOR,
            *constants::AAVE_V3_POOL,
            *constants::UNISWAPV3_ROUTER_02,
            *constants::MORPHO_BLUE,
            debt_asset,
            borrower,
        ];

        if collateral_asset != debt_asset {
            warm_targets.push(collateral_asset);
        }

        // 3. Parallel Async Pre-Warm via RPC before entering blocking thread
        let mut prewarmed_accounts = Vec::with_capacity(warm_targets.len());
        let mut un_cached_targets = vec![];

        for addr in warm_targets {
            if let Some(cached) = self.code_cache.get(&addr) {
                prewarmed_accounts.push((addr, cached.0.clone(), cached.1.clone()));
            } else {
                un_cached_targets.push(addr);
            }
        }

        if !un_cached_targets.is_empty() {
            let futures = un_cached_targets.into_iter().map(|addr| {
                let p = provider.clone();
                async move {
                    let code_fut = p.get_code(addr, Some(block_number.into()));
                    let bal_fut = p.get_balance(addr, Some(block_number.into()));
                    let nonce_fut = p.get_transaction_count(addr, Some(block_number.into()));

                    if let Ok((code, balance, nonce)) = tokio::try_join!(code_fut, bal_fut, nonce_fut) {
                        let r_balance = rU256::from_limbs(balance.0);
                        let r_nonce = nonce.as_u64();

                        let (code_hash, bytecode) = if code.is_empty() {
                            (KECCAK_EMPTY, None)
                        } else {
                            let revm_code = Bytecode::new_raw(revm::primitives::Bytes::copy_from_slice(&code.0));
                            (revm_code.hash_slow(), Some(revm_code))
                        };

                        let info = AccountInfo {
                            balance: r_balance,
                            nonce: r_nonce,
                            code_hash,
                            code: bytecode.clone(),
                        };

                        Ok((addr, info, bytecode))
                    } else {
                        Err(addr)
                    }
                }
            });

            let results = futures_util::future::join_all(futures).await;
            for res in results.into_iter().flatten() {
                // Populate in-memory cache for static contracts
                self.code_cache.insert(res.0, (res.1.clone(), res.2.clone()));
                prewarmed_accounts.push(res);
            }
        }

        // 4. Offload CacheDB population and REVM state build to blocking worker thread
        let provider_for_db = self.provider.clone();
        tokio::task::spawn_blocking(move || -> anyhow::Result<BlockSnapshot<M>> {
            let ethers_db = EthersDB::new(
                provider_for_db,
                Some(BlockId::Number(block_number.into())),
            )
            .ok_or_else(|| anyhow::anyhow!("Failed to initialize EthersDB for block {}", block_number))?;

            // Instantiate SharedEthersDB via constructor/wrapper trait
            let shared_ethers = SharedEthersDB(Arc::new(RwLock::new(ethers_db)));
            let mut db = CacheDB::new(shared_ethers);

            // Pre-fund Keeper wallet in EVM state
            db.insert_account_info(
                keeper_addr.0.into(),
                AccountInfo {
                    balance: rU256::MAX,
                    nonce,
                    code_hash: KECCAK_EMPTY,
                    code: None,
                },
            );

            // Inject parallel pre-warmed state directly into CacheDB
            for (addr, info, bytecode) in prewarmed_accounts {
                let r_addr = addr.0.into();
                db.insert_account_info(r_addr, info);
                if let Some(code) = bytecode {
                    db.contracts.insert(code.hash_slow(), code);
                }
            }

            Ok(BlockSnapshot {
                block_number,
                block: Arc::new(block),
                db: Arc::new(db),
            })
        })
        .await
        .map_err(|e| anyhow::anyhow!("Blocking snapshot worker panicked: {:?}", e))?
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
        // Guard: Self-collateral swap bypass
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
            borrower = %job.borrower,
            "Starting REVM simulation for liquidation job"
        );

        let snapshot = match self
            .build_snapshot(
                block_number,
                job.debt_asset,
                job.collateral_asset,
                job.borrower,
            )
            .await
        {
            Ok(s) => s,
            Err(e) => {
                warn!(
                    borrower = %job.borrower,
                    error = %e,
                    "❌ REVM Snapshot construction failed"
                );
                return Err(e);
            }
        };

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

        // Run pure REVM execution on dedicated blocking worker
        let sim_result = tokio::task::spawn_blocking(move || engine.simulate(&snapshot, sim_tx))
            .await
            .map_err(|e| anyhow::anyhow!("REVM execution worker panicked: {:?}", e))??;

        if !sim_result.success {
            let reason = sim_result
                .revert_reason
                .unwrap_or_else(|| "Unknown revert".to_string());
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
            borrower = %job.borrower,
            gas_used = sim_result.gas_used,
            "✅ REVM Liquidation Simulation SUCCESSFUL"
        );

        Ok(sim_result.gas_used)
    }
}