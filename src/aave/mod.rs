pub mod aave_liquidator;
pub mod helpers;
pub mod types;
pub mod aave_config;
pub mod watchlist_updater;
pub mod aave_watchlist;
pub mod abi_bindings;

use std::sync::Arc;

 use aave_config::AaveConfig;
 use aave_watchlist::AaveWatchList;
 use watchlist_updater::AaveWatchListUpdater;
 use aave_liquidator::AaveLiquidator;

use crate::{aave::abi_bindings::IAaveV3Pool, common::{
     AdminCmd, Config, Liquidator, task_manager::spawn_and_register}};
use tokio::sync::{mpsc, watch};
use ethers::providers::Middleware;


pub async fn start_engine<M: Middleware  + 'static>(
    client: Arc<M>,
    shutdown_rx: watch::Receiver<bool>,
    prune_rx: mpsc::Receiver<AdminCmd>,
    watch_list: Arc<AaveWatchList>,
    pool: Arc<IAaveV3Pool<M>>
    
) -> anyhow::Result<Arc<dyn Liquidator>>{
    let mut aave_config = AaveConfig::load()?;

     aave_config.populate_tokens(client.clone()).await?;

    let aave_config = Arc::new(aave_config);

    let aave_liq = AaveLiquidator::new(
        aave_config.clone(),
        client.clone(),
        watch_list.clone()
    );
    

    let aave_updater = AaveWatchListUpdater::new(
        watch_list.clone(),
        pool.clone(),
        aave_config.clone(),
        shutdown_rx.clone(), 
        prune_rx
    );

    spawn_and_register(async move {
        if let Err(e) = aave_updater.start().await {
            tracing::error!("❌ Aave watch list updater failed: {:?}", e);
        }

    });


    Ok(Arc::new(aave_liq))
}
