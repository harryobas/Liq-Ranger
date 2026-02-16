use std::sync::Arc;
use ethers::providers::Middleware;

pub mod morpho_liquidator;
pub mod morpho_config;
pub mod helpers;
pub mod abi_bindings;
pub mod morpho_watchlist;
pub mod watchlist_updater;
pub mod types;
pub mod morpho_math;

use morpho_config::MorphoConfig;
use morpho_watchlist::MorphoWatchList;
use morpho_liquidator::MorphoLiquidator;
use watchlist_updater::WatchListUpdater;

use crate::common::{Config, Liquidator, task_manager::spawn_and_register, AdminCmd};

use tokio::sync::{mpsc, watch};

pub async fn start_engine<M: Middleware + 'static>(
    client: Arc<M>,
    shutdown_rx: watch::Receiver<bool>,
    prune_rx: mpsc::Receiver<AdminCmd> 
) -> anyhow::Result<Arc<dyn Liquidator>>{

    let config = Arc::new(MorphoConfig::load()?);
    let db = Arc::new(sled::open(&config.db_path)?);

    let watch_list = Arc::new(MorphoWatchList::new(db)?);

    let (morpho_blue, flash_liq) = helpers::fetch_contracts(
        client.clone(), 
        config.clone()
    );

    let morpho_liq = Arc::new(MorphoLiquidator::new(
        morpho_blue.clone(), 
        flash_liq.clone(), 
        watch_list.clone(), 
        client.clone(), 
        config.clone()
    ));

    let updater = WatchListUpdater::new(
        watch_list.clone(), 
        Arc::new(morpho_blue.clone()), 
        config.clone(),
        shutdown_rx,
        prune_rx
    );

    spawn_and_register(async move {
        if let Err(e) = updater.start().await {
            tracing::error!("❌ Morpho watch list updater failed: {:?}", e);
        }

    });


    Ok(morpho_liq.clone())
}