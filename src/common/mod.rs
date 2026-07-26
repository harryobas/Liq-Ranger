pub mod abi_bindings;
pub mod task_manager;

use ethers::{
    providers::{Middleware, PubsubClient},
    types::{Address, H256},
};

use crate::{
    block_watcher::BlockWatcher,
    config,
    constants::{self, TOKEN_DECIMAL_CACHE, TOKEN_SYMBOL_CACHE},
    core::services::liquidation_engine::LiquidationEngine,
    liq_data_extractor::LiqDataExtractor,
    liquidation_executor::LiqExecutor,
    profit_distributor::ProfitDistributor,
    watchlist_pruner::WatchListPruner,
};
use std::sync::Arc;

use crate::{
    adapters::anvil_simulation_sandbox::AnvilSandbox,
    common::abi_bindings::{
        AaveOracle, IAaveV3Pool, IFlashLiquidator, IMorphoBlue, IQuoterV2, ISwapRouter,
        UiPoolDataProvider, IERC20,
    },
    watchlists::{
        aave_watchlist::AaveWatchList,
        bootstrap_state::BootstrapState,
        morpho_watchlist::MorphoWatchList,
        watchlist_updaters::{
            aave_watchlist_updater::AaveWatchListUpdater,
            morpho_watchlist_updater::MorphoWatchListUpdater,
        },
    },
};

use crate::config::AaveConfig;
use tokio::sync::{
    broadcast::{Receiver, Sender},
    mpsc, watch,
};

use sled::Db;
pub trait Config: Send + Sync {
    fn load() -> anyhow::Result<Self>
    where
        Self: Sized;

    fn keeper_address(&self) -> Address;
    fn chain_id(&self) -> u64;
}

pub enum AdminCmd {
    Prune,
    StatusCheck,
}
#[derive(Debug, Clone)]
pub struct CoreContracts<M> {
    pub aave: IAaveV3Pool<M>,
    pub aave_oracle: AaveOracle<M>,
    pub ui_pool_data_provider: UiPoolDataProvider<M>,
    pub morpho: IMorphoBlue<M>,
    pub flash_liq: IFlashLiquidator<M>,
    pub quoter: IQuoterV2<M>,
    pub swaper: ISwapRouter<M>,
}

pub struct WatchLists {
    pub aave_watchlist: Arc<AaveWatchList>,
    pub morpho_watchlist: Arc<MorphoWatchList>,
    pub bootstrap_state: Arc<BootstrapState>,
}

pub async fn get_token_decimals<M: Middleware + 'static>(
    token: Address,
    provider: Arc<M>,
) -> anyhow::Result<u8> {
    if let Some(dec) = TOKEN_DECIMAL_CACHE.get(&token) {
        return Ok(dec.value().clone());
    }

    let contract = IERC20::new(token, provider.clone());
    let result = contract.decimals().call().await?;

    TOKEN_DECIMAL_CACHE.insert(token, result);
    Ok(result)
}

pub async fn get_token_symbol<M: Middleware + 'static>(
    token: Address,
    provider: Arc<M>,
) -> anyhow::Result<String> {
    if let Some(dec) = TOKEN_SYMBOL_CACHE.get(&token) {
        return Ok(dec.value().clone());
    }

    let contract = IERC20::new(token, provider.clone());
    let result = contract.symbol().call().await?;

    TOKEN_SYMBOL_CACHE.insert(token, result.clone());
    Ok(result)
}

pub fn fetch_contracts<M: Middleware + 'static>(
    client: Arc<M>,
) -> anyhow::Result<CoreContracts<M>> {
    let liq_addr = *constants::FLASH_LIQUIDATOR;
    let aave_addr = *constants::AAVE_V3_POOL;
    let morpho_addr = *constants::MORPHO_BLUE;
    let oracle_addr = *constants::AAVE_ORACLE;
    let ui_pool_data_addr = *constants::UIPOOL_DATA_PROVIDER;
    let quoter_addr = *constants::UNISWAPV3_QUOTER_V2;
    let swaper_addr = *constants::UNISWAPV3_ROUTER_02;

    let flash_liq = IFlashLiquidator::new(liq_addr, client.clone());
    let aave = IAaveV3Pool::new(aave_addr, client.clone());
    let morpho = IMorphoBlue::new(morpho_addr, client.clone());
    let aave_oracle = AaveOracle::new(oracle_addr, client.clone());
    let ui_pool_data_provider = UiPoolDataProvider::new(ui_pool_data_addr, client.clone());
    let quoter = IQuoterV2::new(quoter_addr, client.clone());
    let swaper = ISwapRouter::new(swaper_addr, client.clone());

    Ok(CoreContracts {
        aave,
        morpho,
        flash_liq,
        aave_oracle,
        ui_pool_data_provider,
        quoter,
        swaper,
    })
}

pub fn fetch_watchlists(db: Arc<Db>) -> anyhow::Result<WatchLists> {
    Ok(WatchLists {
        aave_watchlist: Arc::new(AaveWatchList::new(db.clone())?),
        morpho_watchlist: Arc::new(MorphoWatchList::new(db.clone())?),
        bootstrap_state: Arc::new(BootstrapState::new(db)?),
    })
}

pub async fn has_outstanding_debt<M: Middleware + 'static>(
    borrower: Address,
    reserve: Address,
    pool: &IAaveV3Pool<M>,
    config: &AaveConfig,
) -> anyhow::Result<bool> {
    let vdebt = config
        .vdebt_tokens
        .get(&reserve)
        .ok_or_else(|| anyhow::anyhow!("missing vDebt token"))?;

    let token = IERC20::new(*vdebt, pool.client());
    let debt = token.balance_of(borrower).call().await?;

    Ok(!debt.is_zero())
}

pub async fn has_outstanding_debt_morpho<M: Middleware + 'static>(
    morpho: Arc<IMorphoBlue<M>>,
    borrower: Address,
    market: H256,
) -> anyhow::Result<bool> {
    let market = market.to_fixed_bytes();
    let (_supply_shares, borrow_shares, _collateral) =
        morpho.position(market, borrower).call().await?;

    Ok(borrow_shares != 0)
}

pub async fn start_aave_watchlist_updater<M: Middleware + 'static>(
    watch_list: Arc<AaveWatchList>,
    pool: Arc<IAaveV3Pool<M>>,
    config: Arc<config::AaveConfig>,
    shoutdown: watch::Receiver<bool>,
    cmd_rx: mpsc::Receiver<AdminCmd>,
) -> anyhow::Result<()> {
    let mut aave_updater = AaveWatchListUpdater::new(watch_list, pool, config, shoutdown, cmd_rx);
    task_manager::spawn_named_and_register("aave_watchlist_updater", async move {
        if let Err(e) = aave_updater.start().await {
            tracing::error!("aave watchlist updater failed: {:?}", e);
        }
    })
    .await;

    Ok(())
}

pub async fn start_morpho_watchlist_updater<M: Middleware + 'static>(
    list: Arc<MorphoWatchList>,
    morpho: Arc<IMorphoBlue<M>>,
    config: Arc<config::MorphoConfig>,
    shutdown: watch::Receiver<bool>,
    cmd_rx: mpsc::Receiver<AdminCmd>,
) -> anyhow::Result<()> {
    let morpho_updater = MorphoWatchListUpdater::new(list, morpho, config, shutdown, cmd_rx);
    task_manager::spawn_named_and_register("morpho_watchlist_updater", async move {
        if let Err(e) = morpho_updater.start().await {
            tracing::error!("morpho watchlist updater failed: {:?}", e);
        }
    })
    .await;

    Ok(())
}
pub async fn start_block_watcher<M>(
    client: Arc<M>,
    tx: Sender<u64>,
    shutdown: watch::Receiver<bool>,
) -> anyhow::Result<()>
where
    M: Middleware + 'static,
    <M as Middleware>::Provider: PubsubClient,
{
    let block_watcher = BlockWatcher::new(client, tx, shutdown);

    task_manager::spawn_named_and_register("block_watcher", async move {
        if let Err(e) = block_watcher.start().await {
            tracing::error!("❌ Block watcher task failed: {:?}", e);
        }
    })
    .await;

    Ok(())
}

pub async fn start_liquidation_executor<M: Middleware + 'static>(
    engine: LiquidationEngine,
    client: Arc<M>,
    receiver: Receiver<u64>,
    shutdown: watch::Receiver<bool>,
) -> anyhow::Result<()> {
    let mut executor = LiqExecutor::new(engine, client, receiver, shutdown);
    task_manager::spawn_named_and_register("liq_executor", async move {
        if let Err(e) = executor.start().await {
            tracing::error!("❌ Liquidation executor task failed: {:?}", e);
        }
    })
    .await;

    Ok(())
}

pub async fn start_watchlist_pruner(
    aave_cmd: mpsc::Sender<AdminCmd>,
    morpho_cmd: mpsc::Sender<AdminCmd>,
    block_rx: Receiver<u64>,
    shutdown: watch::Receiver<bool>,
) -> anyhow::Result<()> {
    let mut pruner = WatchListPruner::new(aave_cmd, morpho_cmd, block_rx, shutdown);
    task_manager::spawn_named_and_register("watchlist_pruner", async move {
        if let Err(e) = pruner.start().await {
            tracing::error!("❌ Watcher list pruner task failed: {:?}", e);
        }
    })
    .await;

    Ok(())
}

pub async fn start_profit_distributor<M: Middleware + 'static>(
    client: Arc<M>,
    contract: Arc<IFlashLiquidator<M>>,
    pool: sqlx::Pool<sqlx::Sqlite>,
    shutdown: watch::Receiver<bool>,
) -> anyhow::Result<()> {
    let distributor = Arc::new(ProfitDistributor::new(client, contract, pool));

    task_manager::spawn_named_and_register("profit_distributor", async move {
        if let Err(e) = distributor.start(shutdown).await {
            tracing::error!("❌ ProfitDistributor task error: {:?}", e);
        }
    })
    .await;

    Ok(())
}

pub async fn start_liq_data_extractor<M: Middleware + 'static>(
    flash_liquidator: Arc<IFlashLiquidator<M>>,
    db_pool: sqlx::SqlitePool,
    shutdown: watch::Receiver<bool>,
    provider: Arc<M>,
) -> anyhow::Result<()> {
    let data_extractor = LiqDataExtractor::new(flash_liquidator, db_pool, shutdown, provider);
    task_manager::spawn_named_and_register("liq_data_extracto", async move {
        if let Err(e) = data_extractor.start().await {
            tracing::error!("❌ liq data extractor task failed: {:?}", e);
        }
    })
    .await;

    Ok(())
}

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct LiquidationRecord {
    pub timestamp: i64,
    pub block_number: i64,
    pub protocol: String,
    pub borrower: Address,
    pub collateral_asset: Address,
    pub profit_asset: Address,
    pub profit_amount: f64,
    pub profit_symbol: String,
    pub collateral_symbol: String,
    pub tx_hash: String,
}

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct DistributionRecord {
    pub tx_hash: String,
    pub asset: String,
    pub asset_symbol: String,
    pub amount: f64,
    pub owner_share: f64,
    pub breet_share: f64,
    pub timestamp: i64,
}

impl LiquidationRecord {
    pub async fn save(&self, pool: &sqlx::SqlitePool) -> anyhow::Result<()> {
        sqlx::query(
            r#"
            INSERT OR IGNORE INTO liquidations (
                tx_hash, protocol, borrower, profit_asset, profit_symbol,
                collateral_asset, collateral_symbol, profit_amount, block_number, timestamp
            ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
            "#,
        )
        .bind(&self.tx_hash)
        .bind(&self.protocol)
        .bind(self.borrower.to_string())
        .bind(self.profit_asset.to_string())
        .bind(&self.profit_symbol)
        .bind(self.collateral_asset.to_string())
        .bind(&self.collateral_symbol)
        .bind(self.profit_amount)
        .bind(self.block_number)
        .bind(self.timestamp)
        .execute(pool)
        .await?;

        Ok(())
    }
}

impl DistributionRecord {
    pub async fn save(&self, pool: &sqlx::SqlitePool) -> anyhow::Result<()> {
        sqlx::query(
            r#"
            INSERT OR IGNORE INTO distributions (
                tx_hash, asset, asset_symbol, amount, owner_share, breet_share, timestamp
            ) VALUES (?, ?, ?, ?, ?, ?, ?)
            "#,
        )
        .bind(&self.tx_hash)
        .bind(&self.asset)
        .bind(&self.asset_symbol)
        .bind(self.amount)
        .bind(self.owner_share)
        .bind(self.breet_share)
        .bind(self.timestamp)
        .execute(pool)
        .await?;

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn addr(n: u64) -> Address {
        Address::from_low_u64_be(n)
    }

    async fn temp_pool() -> (tempfile::TempDir, sqlx::SqlitePool) {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("history.db");
        let url = format!("sqlite://{}", path.display());
        let pool = crate::db::connect(&url).await.expect("connect sqlite");
        (dir, pool)
    }

    #[tokio::test]
    async fn liquidation_record_save_is_idempotent_by_tx_hash() {
        let (_dir, pool) = temp_pool().await;
        let record = LiquidationRecord {
            timestamp: 1,
            block_number: 2,
            protocol: "Aave".to_string(),
            borrower: addr(1),
            collateral_asset: addr(2),
            profit_asset: addr(3),
            profit_amount: 4.5,
            profit_symbol: "USDC".to_string(),
            collateral_symbol: "WETH".to_string(),
            tx_hash: "0xabc".to_string(),
        };

        record.save(&pool).await.expect("first save");
        record.save(&pool).await.expect("duplicate save");

        let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM liquidations")
            .fetch_one(&pool)
            .await
            .expect("count");
        assert_eq!(count, 1);
    }

    #[tokio::test]
    async fn distribution_record_save_is_idempotent_by_tx_hash() {
        let (_dir, pool) = temp_pool().await;
        let record = DistributionRecord {
            tx_hash: "0xdef".to_string(),
            asset: addr(4).to_string(),
            asset_symbol: "DAI".to_string(),
            amount: 10.0,
            owner_share: 7.0,
            breet_share: 3.0,
            timestamp: 11,
        };

        record.save(&pool).await.expect("first save");
        record.save(&pool).await.expect("duplicate save");

        let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM distributions")
            .fetch_one(&pool)
            .await
            .expect("count");
        assert_eq!(count, 1);
    }
}
