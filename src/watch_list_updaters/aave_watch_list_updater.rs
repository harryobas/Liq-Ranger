use std::sync::Arc;
use ethers::{
     contract::Event, providers::Middleware, types::{Address, U256}
};

use crate::{
    abi_bindings::{
        AaveV3Pool, 
        AaveV3PoolEvents, IERC20
    }, 
        config::aave_config::AaveConfig, 
        utils::aave_bootstrap_from_subgraph, 
        watch_list::{
            aave_watch_list::AaveWatchList, WatchList, 
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

    pub async fn start(&self) -> anyhow::Result<()>  {
        aave_bootstrap_from_subgraph(&self.watchlist, &self.config).await?;
        self.prune_watchlist().await?;

        let aave_events: Event<Arc<M>, M, AaveV3PoolEvents> = self.pool.events();
        let  mut aave_event_stream  = aave_events
            .stream()
            .await?;

        while let Some(Ok(evt)) = aave_event_stream.next().await {
            self.handle_event(evt).await?;
            
        }

        Ok(())

        
    }

    async fn add_borrow(&self, borrower: Address, reserve: Address) -> anyhow::Result<()> {
        log::info!("Adding borrow: borrower={:?}, reserve={:?}", borrower, reserve);
        self.watchlist.add((borrower, reserve)).await?;
        log::debug!("Successfully added to watchlist");
        Ok(())
    }

    async fn remove_borrow(&self, borrower: Address, reserve: Address) -> anyhow::Result<()> {
        if !self.has_outstanding_debt(borrower, reserve).await? {
        log::info!("Removing borrow: borrower={:?}, reserve={:?}", borrower, reserve);
        self.watchlist.remove((borrower, reserve)).await?;
    } else {
        log::debug!("Borrower {:?} still has debt on reserve {:?}", borrower, reserve);
    }

    Ok(())

    }

    async fn handle_event(&self, event: AaveV3PoolEvents) -> anyhow::Result<()> {
        match event {
            AaveV3PoolEvents::BorrowFilter(f) => {
                if !self.config.reserves.contains(&f.reserve) {
                    log::debug!("Reserve not in list of tracked reserves: {:?}", f.reserve);
                    return Ok(());
                }

                if let Err(e) = self.add_borrow(f.on_behalf_of, f.reserve).await{
                    log::error!("Failed to add borrow position: {}", e);
                }
            }
            AaveV3PoolEvents::RepayFilter(f) => {
                if !self.config.reserves.contains(&f.reserve) {
                    log::debug!("Reserve not in list of tracked reserves: {:?}", f.reserve);
                    return Ok(());
                }

                if let Err(e) = self.remove_borrow(f.user, f.reserve).await{
                    log::error!("Faild to remove borrow position: {}", e);
                }
            }
        }
        Ok(())
    }

    async fn has_outstanding_debt(&self, borrower: Address, reserve: Address) -> anyhow::Result<bool> {
        if let Some(vdebt) = self.config.vdebt_tokens.get(&reserve) {
            let token = IERC20::new(*vdebt, self.pool.client());
            let debt: U256 = token.balance_of(borrower).call().await?;
            Ok(!debt.is_zero())
        }else{
            log::warn!("No vDebtToken configured for reserve {:?}", reserve);
            Ok(false)
        }
    }

    async fn prune_watchlist(&self) -> anyhow::Result<()> {
        let watch_list = self.watchlist.snapshot().await?;

        for (borrower, reserve) in watch_list {
            if let Err(e) = self.remove_borrow(borrower, reserve).await {
                log::error!("Failed to renove borrow position: {}", e);
            }
        }


        Ok(())
    }
}


  





