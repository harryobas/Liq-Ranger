mod adapters;
mod block_watcher;
mod common;
mod config;
mod constants;
mod core;
mod db;
mod liq_data_extractor;
mod liquidation_executor;
mod profit_distributor;
mod watchlist_pruner;
mod watchlists;

use adapters::{
    aave_bootstrap_adapter::AaveBootstrapAdapter, aave_protocol_adapter::AaveProtocolAdapter,
    anvil_simulation_sandbox::AnvilSandbox, flash_liquidator_adapter::FlashLiquidatorAdapter,
    morpho_bootstrap_adapter::MorphoBootstrapAdapter,
    morpho_protocol_adapter::MorphoProtocolAdapter, paraswap_adapter::ParaSwapAdapter,
};
use core::{
    ports::{Bootstrap, LendingProtocolReader},
    services::{bootstrap_executor::BootstrapExecutor, liquidation_engine::LiquidationEngine},
};
use std::{fs, path::Path, sync::Arc};

use ethers::{
    middleware::{NonceManagerMiddleware, SignerMiddleware},
    providers::{Http, Provider, Ws},
    signers::Signer,
};
use tokio::sync::{broadcast, mpsc, watch};
use url::Url;

use crate::common::{
    fetch_contracts,
    fetch_watchlists,
    start_aave_watchlist_updater,
    start_block_watcher,
    start_liq_data_extractor,
    start_liquidation_executor,
    start_morpho_watchlist_updater,
    start_profit_distributor,
    start_watchlist_pruner,
    task_manager::shutdown_all_tasks,
    AdminCmd,
    Config,
};

pub async fn start_liquidation_engine() -> anyhow::Result<()> {
    // 1. WebSocket Client: High-speed data streaming (BlockWatcher)
    let ws = Ws::connect(constants::RPC_URL.as_str()).await?;
    let ws_client = Arc::new(Provider::new(ws));

    // 2. HTTP Client: Execution (Bootstraps, Engines, Executors)
    let http = Http::new(Url::parse(&*constants::RPC_URL_HTTP)?);
    let http_client = Arc::new(SignerMiddleware::new(
        NonceManagerMiddleware::new(Arc::new(Provider::new(http)), constants::WALLET.address()),
        constants::WALLET.clone(),
    ));

    // --- Communication Channels ---
    let (shutdown_tx, shutdown_rx) = watch::channel(false);
    let (block_tx, block_rx) = broadcast::channel::<u64>(64);

    // Command channels for watchlist management
    let (aave_tx, aave_rx) = mpsc::channel::<AdminCmd>(64);
    let (morpho_tx, morpho_rx) = mpsc::channel::<AdminCmd>(64);

    // --- Database Setup ---
    if let Some(parent) = Path::new(constants::SLED_PATH).parent() {
        if !parent.exists() {
            tracing::info!("Creating database directory at {:?}", parent);
            fs::create_dir_all(parent)?;
        }
    }
    let sled_db = Arc::new(sled::open(constants::SLED_PATH)?);
    let sqlite_pool = db::connect(&*constants::DATABASE_URL).await?;

    // --- Setup Contracts & Watchlists ---
    let contracts = fetch_contracts(http_client.clone())?;
    let w_lists = fetch_watchlists(sled_db)?;

    // Shared reference to flash liquidator contract
    let flash_liq_contract = Arc::new(contracts.flash_liq);

    // --- Bootstraps (Using HTTP Client) ---
    let bootstrapers: Vec<Arc<dyn Bootstrap>> = vec![
        Arc::new(AaveBootstrapAdapter::new(
            contracts.aave.clone(),
            w_lists.aave_watchlist.clone(),
            w_lists.bootstrap_state.clone(),
            http_client.clone(),
        )),
        Arc::new(MorphoBootstrapAdapter::new(
            contracts.morpho.clone(),
            w_lists.morpho_watchlist.clone(),
            w_lists.bootstrap_state.clone(),
            http_client.clone(),
        )),
    ];

    tracing::info!("Running protocol bootstraps...");
    BootstrapExecutor { bootstrapers }.run_all().await?;

    // --- Adapters & Core Engine ---
    let liquidator = Arc::new(
        FlashLiquidatorAdapter::new(flash_liq_contract.clone())
    );

    let mut aave_config = config::AaveConfig::load()?;
    aave_config.populate_vdebt_tokens(http_client.clone()).await?;

    let aave_config = Arc::new(aave_config);
    let morpho_config = Arc::new(config::MorphoConfig::load()?);

    let aave_protocol_reader = AaveProtocolAdapter::new(
        contracts.aave.clone(),
        contracts.aave_oracle,
        w_lists.aave_watchlist.clone(),
        contracts.ui_pool_data_provider,
        http_client.clone(),
        (*aave_config).clone(),
    );

    let morpho_protocol_reader = MorphoProtocolAdapter::new(
        contracts.morpho.clone(),
        w_lists.morpho_watchlist.clone(),
        http_client.clone(),
        (*morpho_config).clone(),
    );

    let protocol_readers: Vec<Arc<dyn LendingProtocolReader + Send + Sync>> = vec![
        Arc::new(aave_protocol_reader),
        Arc::new(morpho_protocol_reader),
    ];

    let dex_finder = Arc::new(ParaSwapAdapter::new(
        flash_liq_contract.address(),
        constants::CHAIN_ID,
    ));

    let simulator = Arc::new(AnvilSandbox::new(
        &constants::RPC_URL_HTTP,
        0,
        flash_liq_contract.clone(),
    )?);

    let liq_engine = LiquidationEngine::new(protocol_readers, dex_finder, simulator, liquidator);

    // --- Spawn Background Services ---
    start_aave_watchlist_updater(
        w_lists.aave_watchlist.clone(),
        Arc::new(contracts.aave.clone()),
        aave_config,
        shutdown_rx.clone(),
        aave_rx,
    )
    .await?;

    start_morpho_watchlist_updater(
        w_lists.morpho_watchlist.clone(),
        Arc::new(contracts.morpho.clone()),
        morpho_config,
        shutdown_rx.clone(),
        morpho_rx,
    )
    .await?;

    start_liquidation_executor(
        liq_engine,
        http_client.clone(),
        block_rx.resubscribe(),
        shutdown_rx.clone(),
    )
    .await?;

    start_watchlist_pruner(
        aave_tx,
        morpho_tx,
        block_rx.resubscribe(),
        shutdown_rx.clone(),
    )
    .await?;

    start_profit_distributor(
        http_client.clone(),
        flash_liq_contract.clone(),
        sqlite_pool.clone(),
        shutdown_rx.clone(),
    )
    .await?;

    start_liq_data_extractor(
        flash_liq_contract.clone(),
        sqlite_pool.clone(),
        shutdown_rx.clone(),
        http_client.clone(),
    )
    .await?;

    // Start BlockWatcher LAST after all tasks are subscribed and listening
    start_block_watcher(ws_client.clone(), block_tx, shutdown_rx.clone()).await?;

    tracing::info!("🚀 Liquidation system started successfully");

    // Lifecycle Management
    tokio::signal::ctrl_c().await?;
    tracing::info!("🛑 Shutdown signal received");

    let _ = shutdown_tx.send(true);
    shutdown_all_tasks().await;
    tracing::info!("👋 Shutdown complete");

    Ok(())
}
