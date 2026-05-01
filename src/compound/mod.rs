use ethers::providers::Middleware;
use std::sync::Arc;
use tokio::sync::{mpsc, watch};

use crate::{
    common::{task_manager::spawn_named_and_register, AdminCmd, Liquidator},
    compound::{
        abi_bindings::IComet, compound_liquidator::CompoundLiquidator,
        compound_watchlist::CompoundWatchList,
        compound_watchlist_updater::CompoundWatchListUpdater,
    },
};

pub mod abi_bindings;
pub mod compound_liquidator;
pub mod compound_watchlist;
pub mod compound_watchlist_updater;
pub mod helpers;
pub mod types;

pub async fn start_engine<M: Middleware + 'static>(
    client: Arc<M>,
    watch_list: Arc<CompoundWatchList>,
    comet: IComet<M>,
    shutdown_rx: watch::Receiver<bool>,
    prune_rx: mpsc::Receiver<AdminCmd>,
) -> anyhow::Result<Arc<dyn Liquidator>> {
    let comet_liq = Arc::new(
        CompoundLiquidator::new(client.clone(), watch_list.clone())
    );

    spawn_named_and_register("compound_watchlist_updater", async move {
        
        let updater = CompoundWatchListUpdater::new(
            watch_list.clone(), 
            Arc::new(comet), 
            shutdown_rx, 
            prune_rx
        );

        if let Err(e) = updater.start().await {
            tracing::error!("Compound WatchListUpdater error: {:?}", e);
        }
    }).await;

    Ok(comet_liq)
}
