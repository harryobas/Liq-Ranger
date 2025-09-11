
mod liquidators;
mod models;
mod constants;
mod abi_bindings;
mod utils;
mod block_watcher;
mod liquidation_worker;
mod watch_list;
mod watch_list_updaters;
mod config;

use std::sync::Arc;
use dotenv::dotenv;

use ethers::{
    middleware::SignerMiddleware,
    providers::{ Provider, Ws}, 
};

use models::LiquidationCommand;
use liquidators::{Liquidator, aave_liquidator::AaveLiquidator};

use crate::{
    watch_list::aave_watch_list::AaveWatchList,
     watch_list_updaters::aave_watch_list_updater::AaveWatchListUpdater,
     config::aave_config::AaveConfig
    };

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    env_logger::init();
    dotenv().ok();

    log::info!("Starting liquidation mining....");

    let aave_config = Arc::new(AaveConfig::load()?);

    let ws = Ws::connect(aave_config.rpc_url.clone()).await?;
    let provider = Provider::new(ws);

    let client = Arc::new(
        SignerMiddleware::new(provider, aave_config.private_key.clone())
    );

     let aave_watch_list = Arc::new(AaveWatchList::new());
     
    let aave_bot = AaveLiquidator::new(
        aave_config.clone(),
        client.clone(),
        aave_watch_list.clone()
    );

    let aave_updater = AaveWatchListUpdater::new(
        aave_watch_list.clone(), 
        Arc::new(aave_bot.lending_pool.clone()),
        aave_config.clone()

    );

    tokio::spawn(async move {
        if let Err(e) = aave_updater.start().await {
            log::error!("AaveWatchListUpdater exited with error: {:?}", e)
        }
    });

   

    let (tx, rx) = tokio::sync::mpsc::unbounded_channel::<LiquidationCommand>();

    let block_watcher = block_watcher::BlockWatcher::new(
        client.clone(),
         tx
    );

    tokio::spawn(async move {
        if let Err(e) = block_watcher.start().await {
            log::error!("watcher exited with error: {:?}", e)
        }
    });

    let aave_bot: Arc<dyn Liquidator + Send + Sync> = Arc::new(aave_bot);

    let mut worker = liquidation_worker::LiquidationWorker::new(aave_bot.clone(), rx);

    tokio::spawn(async move {
        if let Err(e) = worker.start().await {
            log::error!("Worker exited with error: {:?}", e);
        }
    });

    Ok(())
    
}
