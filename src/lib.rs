mod aave;
mod morpho;
mod compound;
mod common;
mod block_watcher;
mod liquidation_executor;
mod watchlist_pruner;
mod constants;
mod profit_distributor;

use std::sync::Arc;

use ethers::{
    middleware::{SignerMiddleware, NonceManagerMiddleware},
    providers::{Provider, Ws},
    signers::Signer,
};

use tokio::sync::{watch, mpsc, broadcast};

use crate::{
    common::{
        AdminCmd, 
        task_manager::{shutdown_all_tasks, spawn_and_register}}, 
        profit_distributor::ProfitDistributor, 
        watchlist_pruner::WatchListPruner
    };

pub async fn start_liquidation_engines() -> anyhow::Result<()> {

    let ws = Ws::connect(constants::RPC_URL.as_str()).await?;
    let provider = Provider::new(ws);

    let nonce_manager =
        NonceManagerMiddleware::new(provider.clone(), constants::WALLET.address());

    let client = Arc::new(
        SignerMiddleware::new(nonce_manager, constants::WALLET.clone())
    );
    let (shutdown_tx, shutdown_rx) = watch::channel(false);

    let (block_tx, block_rx) = broadcast::channel::<u64>(16);

    let (aave_tx, aave_rx) = mpsc::channel::<AdminCmd>(16);
    let (morpho_tx, morpho_rx) = mpsc::channel::<AdminCmd>(16);

    // Start Aave + Morpho + Compound Engines
    let morpho_engine = morpho::start_engine(
        client.clone(),
        shutdown_rx.clone(),
        morpho_rx,                    
    ).await?;

    let aave_engine = aave::start_engine(
        client.clone(),
        shutdown_rx.clone(),
        aave_rx,                      
    ).await?;

    let liquidators = vec![morpho_engine, aave_engine];

    let block_watcher = block_watcher::BlockWatcher::new(
        client.clone(),
        block_tx,
        shutdown_rx.clone(),
    );

    spawn_and_register(async move {
        if let Err(e) = block_watcher.start().await {
            tracing::error!("❌ Block watcher failed: {:?}", e);
        }
    });

    let executor = liquidation_executor::LiqExecutor::new(
        liquidators,
        block_rx.resubscribe(),   // 👈 separate receiver
        shutdown_rx.clone(),
        constants::BLOCK_INTERVAL,
    );

    spawn_and_register(async move {
        if let Err(e) = executor.start().await {
            tracing::error!("❌ Liquidation executor failed: {:?}", e);
        }
    });

    let mut watchlist_pruner = WatchListPruner::new(
        aave_tx.clone(),
        morpho_tx.clone(),
        block_rx.resubscribe(),
        shutdown_rx.clone(),
        constants::PRUNE_INTERVAL
    );

    spawn_and_register(async move {
        if let Err(e) = watchlist_pruner.start().await {
            tracing::error!("❌ Watchlist pruner failed: {:?}", e);
        }
    });

    let f_liq = common::fetch_contracts(client.clone())?.flash_liq; 

    let profit_distributor = Arc::new(
        ProfitDistributor::new(
        client.clone(), 
        Arc::new(f_liq))
    );

    spawn_and_register(async move {
        if let Err(e) = profit_distributor.start().await {
             tracing::error!("❌ Profit_distributor failed: {:?}", e);

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
