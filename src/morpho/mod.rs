use ethers::providers::Middleware;
use std::sync::Arc;

pub mod abi_bindings;
pub mod helpers;
pub mod morpho_config;
pub mod morpho_liquidator;
pub mod morpho_math;
pub mod morpho_watchlist;
pub mod types;
pub mod watchlist_updater;

use morpho_config::MorphoConfig;
use morpho_liquidator::MorphoLiquidator;
use morpho_watchlist::MorphoWatchList;
use watchlist_updater::WatchListUpdater;

use crate::{
    common::{
        abi_bindings::IFlashLiquidator, task_manager::spawn_and_register, AdminCmd, Config,
        Liquidator,
    },
    morpho::abi_bindings::IMorphoBlue,
};

use tokio::sync::{mpsc, watch};

pub async fn start_engine<M: Middleware + 'static>(
    client: Arc<M>,
    shutdown_rx: watch::Receiver<bool>,
    prune_rx: mpsc::Receiver<AdminCmd>,
    watch_list: Arc<MorphoWatchList>,
    f_liq: IFlashLiquidator<M>,
    morpho: IMorphoBlue<M>,
) -> anyhow::Result<Arc<dyn Liquidator>> {
    let config = Arc::new(MorphoConfig::load()?);

    let morpho_liq = Arc::new(MorphoLiquidator::new(
        morpho.clone(),
        f_liq.clone(),
        watch_list.clone(),
        client.clone(),
        config.clone(),
    ));

    let updater = WatchListUpdater::new(
        watch_list.clone(),
        Arc::new(morpho),
        config.clone(),
        shutdown_rx,
        prune_rx,
    );

    spawn_and_register(async move {
        tracing::info!("Morpho watch list updater starting...");
        if let Err(e) = updater.start().await {
            tracing::error!("❌ Morpho watch list updater failed: {:?}", e);
        }
    });

    Ok(morpho_liq)
}
