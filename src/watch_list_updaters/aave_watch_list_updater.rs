use std::sync::Arc;
use ethers::{
    contract::Event, 
    providers::Middleware
};

use crate::{
    abi_bindings::{
        AaveV3Pool, 
        AaveV3PoolEvents
    }, 
        config::aave_config::AaveConfig, 
        utils::aave_bootstrap_from_subgraph, 
        watch_list::{
            aave_watch_list::AaveWatchList, 
            WatchList
        }
};

use futures_util::stream::StreamExt;

pub struct AaveWatchListUpdater<M: Middleware + 'static> {
    
    watchlist: Arc<AaveWatchList>,
    pool: Arc<AaveV3Pool<M>>,
    config: Arc<AaveConfig>
}

impl <M: Middleware> AaveWatchListUpdater<M>{

    pub fn new(watchlist: Arc<AaveWatchList>, pool: Arc<AaveV3Pool<M>>, config: Arc<AaveConfig> ) -> Self {
        Self{watchlist, pool, config}
    }

    pub async fn start(&self) -> anyhow::Result<()> {
        aave_bootstrap_from_subgraph(&self.watchlist, &self.config).await?;

        let aave_events: Event<Arc<M>, M, AaveV3PoolEvents> = self.pool.events();
        let  mut aave_event_stream  = aave_events
            .stream()
            .await?;
        
        while let Some(Ok(event)) = aave_event_stream.next().await {
            match event {
                AaveV3PoolEvents::BorrowFilter(f) => {
                    let borrower = f.on_behalf_of;
                    let reserve = f.reserve;
                    if let Err(e) = self.watchlist.add((borrower, reserve)).await {
                        log::error!("Failed to update AawatchList {:?}:", e);
                    }
                },
                AaveV3PoolEvents::RepayFilter(f) => {
                    todo!()
                },
                
            }
        }
        
        Ok(())
    }

}
