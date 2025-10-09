use std::sync::Arc;
use ethers::{
    contract::Event,
    providers::Middleware,
    types::{Address, U256},
};
use futures_util::stream::StreamExt;

use crate::{
    abi_bindings::{AaveV3Pool, AaveV3PoolEvents, IERC20},
    config::aave_config::AaveConfig,
    watch_list::{aave_watch_list::AaveWatchList, SubgraphBootstrap, WatchList},
};

pub struct AaveWatchListUpdater<M: Middleware + 'static> {
    watch_list: Arc<AaveWatchList>,
    pool: Arc<AaveV3Pool<M>>,
    config: Arc<AaveConfig>,
}

impl<M: Middleware> AaveWatchListUpdater<M> {
    pub fn new(
        watch_list: Arc<AaveWatchList>,
        pool: Arc<AaveV3Pool<M>>,
        config: Arc<AaveConfig>,
    ) -> Self {

        Self {
            watch_list,
            pool,
            config,
        }
    }

    pub async fn start(&self) -> anyhow::Result<()> {
        // bootstrap from subgraph
        self.watch_list.bootstrap_from_subgraph(self.config.clone()).await?;
        
        self.prune_watchlist().await?;

        // subscribe to events
        let aave_events: Event<Arc<M>, M, AaveV3PoolEvents> = self.pool.events();
        let mut aave_event_stream = aave_events.stream().await?;

        log::info!("AaveWatchListUpdater started...");

        while let Some(item) = aave_event_stream.next().await {
            match item {
                Ok(evt) => {
                    if let Err(e) = self.handle_event(evt).await {
                        log::error!("Failed to handle event: {:?}", e);
                    }
                }
                Err(e) => {
                    log::error!("Stream error: {:?}", e);
                    return Err(e.into()); // let supervisor restart
                }
            }
        }

        Ok(())
    }

    async fn add_borrow(&self, borrower: Address, reserve: Address) -> anyhow::Result<()> {
        log::info!("Adding borrow: borrower={:?}, reserve={:?}", borrower, reserve);
        self.watch_list.add((borrower, reserve)).await?;
        log::debug!("Successfully added to watchlist");
        Ok(())
    }

    async fn remove_borrow(&self, borrower: Address, reserve: Address) -> anyhow::Result<()> {
        if !self.has_outstanding_debt(borrower, reserve).await? {
            log::info!(
                "Removing borrow: borrower={:?}, reserve={:?}",
                borrower,
                reserve
            );
            self.watch_list.remove((borrower, reserve)).await?;
        } else {
            log::debug!(
                "Borrower {:?} still has debt on reserve {:?}",
                borrower,
                reserve
            );
        }
        Ok(())
    }

    async fn handle_event(&self, event: AaveV3PoolEvents) -> anyhow::Result<()> {
        match event {
            AaveV3PoolEvents::BorrowFilter(f) => {
                if !self.config.reserves.contains(&f.reserve) {
                    log::debug!("Reserve not tracked: {:?}", f.reserve);
                    return Ok(());
                }
                self.add_borrow(f.on_behalf_of, f.reserve).await?;
            }
            AaveV3PoolEvents::RepayFilter(f) => {
                if !self.config.reserves.contains(&f.reserve) {
                    log::debug!("Reserve not tracked: {:?}", f.reserve);
                    return Ok(());
                }
                self.remove_borrow(f.user, f.reserve).await?;
            }
        }
        Ok(())
    }

    async fn has_outstanding_debt(
        &self,
        borrower: Address,
        reserve: Address,
    ) -> anyhow::Result<bool> {
        if let Some(vdebt) = self.config.vdebt_tokens.get(&reserve) {
            let token = IERC20::new(*vdebt, self.pool.client());
            let debt: U256 = token.balance_of(borrower).call().await?;
            Ok(!debt.is_zero())
        } else {
            log::warn!("No vDebtToken configured for reserve {:?}", reserve);
            Ok(false)
        }
    }

    async fn prune_watchlist(&self) -> anyhow::Result<()> {
        let watch_list = self.watch_list.snapshot().await?;
        for (borrower, reserve) in watch_list {
            if let Err(e) = self.remove_borrow(borrower, reserve).await {
                log::error!("Failed to remove borrow position: {}", e);
            }
        }
        Ok(())
    }
}
