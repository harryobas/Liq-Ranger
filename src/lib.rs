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

use crate::{
    common::{
        fetch_contracts, fetch_watchlists,
        task_manager::{shutdown_all_tasks, spawn_and_register},
        AdminCmd,
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
    let ws = Ws::connect(constants::RPC_URL.as_str()).await?;
    let provider = Provider::new(ws);

    let nonce_manager = NonceManagerMiddleware::new(provider.clone(), constants::WALLET.address());

    let client = Arc::new(SignerMiddleware::new(
        nonce_manager,
        constants::WALLET.clone(),
    ));
    let (shutdown_tx, shutdown_rx) = watch::channel(false);

    let (block_tx, block_rx) = broadcast::channel::<u64>(64);

    let (aave_tx, aave_rx) = mpsc::channel::<AdminCmd>(64);
    let (morpho_tx, morpho_rx) = mpsc::channel::<AdminCmd>(64);
    let (comet_tx, comet_rx) = mpsc::channel::<AdminCmd>(64);

    if let Some(parent) = Path::new(constants::SLED_PATH).parent() {
        if !parent.exists() {
            tracing::info!("Creating database directory at {:?}", parent);
            fs::create_dir_all(parent)?;
        }
    }

    let db = Arc::new(sled::open(constants::SLED_PATH)?);
    let sqlite_pool = db::connect(&*constants::DATABASE_URL).await?;

    let contracts = fetch_contracts(client.clone())?;
    let w_lists = fetch_watchlists(db)?;

    let bootstraps: Vec<Arc<dyn Bootstrap>> = vec![
        Arc::new(AaveBootstrap::new(
            contracts.aave.clone(),
            w_lists.aave_watchlist.clone(),
            w_lists.bootstrap_state.clone(),
            client.clone(),
        )),
        Arc::new(MorphoBootstrap::new(
            contracts.morpho.clone(),
            w_lists.morpho_watchlist.clone(),
            w_lists.bootstrap_state.clone(),
            client.clone(),
        )),
        Arc::new(CompoundBootstrap::new(
            contracts.comet.clone(),
            w_lists.comet_watchlist.clone(),
            
        )),
    ];

    tracing::info!("Running bootstraps...");
    bootstrap_engine::BootstrapExecutor { bootstraps }
        .run_all()
        .await?;

    //Start Aave + Morpho + Compound Engines
    let morpho_engine = morpho::start_engine(
        client.clone(),
        shutdown_rx.clone(),
        morpho_rx,
        w_lists.morpho_watchlist.clone(),
        contracts.flash_liq.clone(),
        contracts.morpho.clone(),
    )
    .await?;

    let aave_engine = aave::start_engine(
        client.clone(),
        shutdown_rx.clone(),
        aave_rx,
        w_lists.aave_watchlist.clone(),
        Arc::new(contracts.aave.clone()),
    )
    .await?;

    let compound_engine = compound::start_engine(
        client.clone(),
        w_lists.comet_watchlist.clone(),
        contracts.comet,
        shutdown_rx.clone(),
        comet_rx,
    )
    .await?;

    let liquidators = vec![morpho_engine, aave_engine, compound_engine];

    let executor = liquidation_executor::LiqExecutor::new(
        liquidators,
        block_rx.resubscribe(), // 👈 separate receiver
        shutdown_rx.clone(),
    );

    spawn_and_register(async move {
        if let Err(e) = executor.start().await {
            tracing::error!("❌ Liquidation executor failed: {:?}", e);
        }
    });

    let mut watchlist_pruner = WatchListPruner::new(
        aave_tx.clone(),
        morpho_tx.clone(),
        comet_tx.clone(),
        block_rx.resubscribe(),
        shutdown_rx.clone(),
        constants::PRUNE_INTERVAL,
    );

    spawn_and_register(async move {
        if let Err(e) = watchlist_pruner.start().await {
            tracing::error!("❌ Watchlist pruner failed: {:?}", e);
        }
    });

    let f_liq = Arc::new(contracts.flash_liq);

    let profit_distributor = Arc::new(
        ProfitDistributor::new(client.clone(), 
        f_liq.clone(), 
        sqlite_pool.clone())
    );

    spawn_and_register(async move {
        if let Err(e) = profit_distributor.start().await {
            tracing::error!("❌ Profit_distributor failed: {:?}", e);
        }
    });

    let liq_data_extractor = LiqDataExtractor::new(
        f_liq.clone(),
        sqlite_pool.clone(),
        shutdown_rx.clone(),
        client.clone(),
    );

    spawn_and_register(async move {
        if let Err(e) = liq_data_extractor.start().await {
            tracing::error!("❌ LiqDataExtractor failed: {:?}", e);
        }
    });

    let block_watcher =
        block_watcher::BlockWatcher::new(client.clone(), block_tx, shutdown_rx.clone());

    spawn_and_register(async move {
        if let Err(e) = block_watcher.start().await {
            tracing::error!("❌ Block watcher failed: {:?}", e);
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
