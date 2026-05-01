mod aave;
mod block_watcher;
mod bootstrap_engine;
mod common;
mod compound;
mod constants;
mod liquidation_executor;
mod morpho;
mod profit_distributor;
mod watchlist_pruner;
mod liq_data_extractor;
mod db;

use std::{fs, path::Path, sync::Arc};

use ethers::{
    middleware::{NonceManagerMiddleware, SignerMiddleware},
    providers::{Provider, Ws, Http},
    signers::Signer,
};

use tokio::sync::{broadcast, mpsc, watch};

use url::Url;

use crate::{
    common::{
        fetch_contracts, fetch_watchlists,
        task_manager::{shutdown_all_tasks, spawn_and_register},
        AdminCmd,
        Liquidator,
    },
    profit_distributor::ProfitDistributor,
    watchlist_pruner::WatchListPruner,
    liq_data_extractor::LiqDataExtractor,
};
use bootstrap_engine::{
    Bootstrap,
    aave_bootstrap::AaveBootstrap, 
    morpho_bootstrap::MorphoBootstrap, 
    compound_bootstrap::CompoundBootstrap,
};

pub async fn start_liquidation_engines() -> anyhow::Result<()> {
    // 1. WebSocket Client: For high-speed data streaming (BlockWatcher)
    let ws = Ws::connect(constants::RPC_URL.as_str()).await?;
    let ws_provider = Provider::new(ws);
    let ws_client = Arc::new(ws_provider);

    // 2. HTTP Client: For execution (Bootstraps, Engines, Executors)
    let http = Http::new(Url::parse(&*constants::RPC_URL_HTTP)?);
    let http_provider = Provider::new(http);
    let http_provider_arc = Arc::new(http_provider);

    // Middleware Layer: Nonce Management
    let nonce_manager = NonceManagerMiddleware::new(
        http_provider_arc.clone(), 
        constants::WALLET.address()
    );

    // Middleware Layer: Signer
    let http_client = Arc::new(SignerMiddleware::new(
        nonce_manager,
        constants::WALLET.clone(),
    ));

    // --- Communication Channels ---
    let (shutdown_tx, shutdown_rx) = watch::channel(false);
    let (block_tx, block_rx) = broadcast::channel::<u64>(64);
    let (aave_tx, aave_rx) = mpsc::channel::<AdminCmd>(64);
    let (morpho_tx, morpho_rx) = mpsc::channel::<AdminCmd>(64);
    let (comet_tx, comet_rx) = mpsc::channel::<AdminCmd>(64);

    // --- Database Setup ---
    if let Some(parent) = Path::new(constants::SLED_PATH).parent() {
        if !parent.exists() {
            tracing::info!("Creating database directory at {:?}", parent);
            fs::create_dir_all(parent)?;
        }
    }
    let db = Arc::new(sled::open(constants::SLED_PATH)?);
    let sqlite_pool = db::connect(&*constants::DATABASE_URL).await?;

    // --- Setup Contracts & Watchlists ---
    // Use http_client for initial setup calls
    let contracts = fetch_contracts(http_client.clone())?;
    let w_lists = fetch_watchlists(db)?;

    // --- Bootstraps (Using HTTP Client) ---
    let bootstraps: Vec<Arc<dyn Bootstrap>> = vec![
        Arc::new(AaveBootstrap::new(
            contracts.aave.clone(),
            w_lists.aave_watchlist.clone(),
            w_lists.bootstrap_state.clone(),
            http_client.clone(),
        )),
        Arc::new(MorphoBootstrap::new(
            contracts.morpho.clone(),
            w_lists.morpho_watchlist.clone(),
            w_lists.bootstrap_state.clone(),
            http_client.clone(),
        )),
        Arc::new(CompoundBootstrap::new(
            contracts.comet.clone(),
            w_lists.comet_watchlist.clone(),
        
        )),
    ];

    tracing::info!("Running bootstraps...");
    spawn_and_register(async move {
    if let Err(e) = (bootstrap_engine::BootstrapExecutor { bootstraps }).run_all().await {
        tracing::error!("❌ Bootstrap executor failed: {:?}", e);
    }
});

 let block_watcher = block_watcher::BlockWatcher::new(
        ws_client.clone(), 
        block_tx, 
        shutdown_rx.clone()
    );

    spawn_and_register(async move {
        tracing::info!("Starting block watcher...");
        if let Err(e) = block_watcher.start().await {
            tracing::error!("❌ Block watcher failed: {:?}", e);
        }
    });

    let morpho_fut = morpho::start_engine(
        http_client.clone(),
        shutdown_rx.clone(),
        morpho_rx,
        w_lists.morpho_watchlist.clone(),
        contracts.flash_liq.clone(),
        contracts.morpho.clone(),
    );

    let aave_fut = aave::start_engine(
        http_client.clone(),
        shutdown_rx.clone(),
        aave_rx,
        w_lists.aave_watchlist.clone(),
        Arc::new(contracts.aave.clone()),
    );

    let compound_fut = compound::start_engine(
        http_client.clone(),
        w_lists.comet_watchlist.clone(),
        contracts.comet,
        shutdown_rx.clone(),
        comet_rx,
    );

    let (morpho_res, aave_res, compound_res): (anyhow::Result<Arc<dyn Liquidator>>, anyhow::Result<Arc<dyn Liquidator>>, anyhow::Result<Arc<dyn Liquidator>>) = tokio::join!(morpho_fut, aave_fut, compound_fut);


    let morpho_engine: Arc<dyn Liquidator> = morpho_res?;
    let aave_engine: Arc<dyn Liquidator> = aave_res?;
    let compound_engine: Arc<dyn Liquidator> = compound_res?;

    // --- Executor ---
    let liquidators = vec![morpho_engine, aave_engine, compound_engine];
    let executor = liquidation_executor::LiqExecutor::new(
        liquidators,
        block_rx.resubscribe(),
        shutdown_rx.clone(),
    );

    spawn_and_register(async move {
        tracing::info!("Starting liquidation executor...");
        if let Err(e) = executor.start().await {
            tracing::error!("❌ Liquidation executor failed: {:?}", e);
        }
    });

    // --- Other Components ---
    let mut watchlist_pruner = WatchListPruner::new(
        aave_tx.clone(),
        morpho_tx.clone(),
        comet_tx.clone(),
        block_rx.resubscribe(),
        shutdown_rx.clone(),
        constants::PRUNE_INTERVAL,
    );
    spawn_and_register(async move {
        tracing::info!("Starting watchlist pruner...");
        if let Err(e) = watchlist_pruner.start().await {
            tracing::error!("❌ Watchlist pruner failed: {:?}", e);
        }
    });

    let f_liq = Arc::new(contracts.flash_liq);
    let profit_distributor = Arc::new(ProfitDistributor::new(
        http_client.clone(), 
        f_liq.clone(), 
        sqlite_pool.clone()
    ));
    spawn_and_register(async move {
        tracing::info!("Starting profit distributor...");
        if let Err(e) = profit_distributor.start().await {
            tracing::error!("❌ Profit_distributor failed: {:?}", e);
        }
    });

    let liq_data_extractor = LiqDataExtractor::new(
        f_liq.clone(),
        sqlite_pool.clone(),
        shutdown_rx.clone(),
        http_client.clone(),
    );
    spawn_and_register(async move {
        tracing::info!("Starting liquidation data extractor...");
        if let Err(e) = liq_data_extractor.start().await {
            tracing::error!("❌ LiqDataExtractor failed: {:?}", e);
        }
    });

 

    tracing::info!("🚀 Liquidation system started");
    tokio::signal::ctrl_c().await?;
    tracing::info!("🛑 Shutdown signal received");
    let _ = shutdown_tx.send(true);
    shutdown_all_tasks().await;
    tracing::info!("👋 Shutdown complete");

    Ok(())
}