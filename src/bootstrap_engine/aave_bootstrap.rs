use std::{collections::HashSet, sync::Arc};
use ethers::{types::Address, providers::Middleware};

use crate::{
    common::WatchList, constants, aave::{abi_bindings::IAaveV3Pool, 
        aave_watchlist::AaveWatchList}
    };

use super::{
    bootstrap_state::BootstrapState,
    Bootstrap,
    Protocol
};

pub struct AaveBootstrap<M> {
    aave: IAaveV3Pool<M>,
    watch_list: Arc<AaveWatchList>,
    state: Arc<BootstrapState>,
    provider: Arc<M>,
    deploy_block: u64
}

impl<M: Middleware + 'static> AaveBootstrap<M> {
    pub fn new(
        aave: IAaveV3Pool<M>,
        watch_list: Arc<AaveWatchList>,
        state: Arc<BootstrapState>,
        provider: Arc<M>,
    ) -> Self {
        Self {
            aave,
            watch_list,
            state,
            provider,
            deploy_block: constants::AAVE_DEPLOY_BLOCK
        }
    }
}

#[async_trait::async_trait]
impl<M: Middleware + 'static> Bootstrap  for AaveBootstrap<M> {
    async fn run(&self) -> anyhow::Result<()> {
        tracing::info!("starting aave bootstrap");
        let whitelist_reserves = &*constants::AAVE_RESERVES;
        let last_block = self.state.load_last_block(Protocol::Aave).await?;

        let latest_block = self.provider.get_block_number().await?.as_u64();

        let mut start_block = last_block
            .unwrap_or(self.deploy_block)
            .saturating_sub(20);

        let batch_size = 1_500u64;
        let mut entries: HashSet<(Address, Address)> = HashSet::new();


        while start_block <= latest_block {
             let current_end = (start_block + batch_size).min(latest_block);

             tracing::info!("Aave bootstrap scanning {} -> {}", start_block, current_end);

             let borrow_filter = self.aave
                .borrow_filter()
                .from_block(start_block)
                .to_block(current_end);

            let repay_filter = self.aave
                .repay_filter()
                .from_block(start_block)
                .to_block(current_end);

            let liq_filter = self.aave
                .liquidation_call_filter()
                .from_block(start_block)
                .to_block(current_end);

            let (borrows, repays, liqs) = tokio::try_join!(
                borrow_filter.query(),
                repay_filter.query(),
                liq_filter.query(),
            ).map_err(|e| anyhow::anyhow!("Aave RPC error at block {}: {}", start_block, e))?;

            for ev in borrows.into_iter() {
                if whitelist_reserves.contains(&ev.reserve) {
                    entries.insert((ev.on_behalf_of, ev.reserve));
                }
            }

            for ev in repays.into_iter() {
                if whitelist_reserves.contains(&ev.reserve) {
                    entries.insert((ev.user, ev.reserve));
                }
            }

            for ev in liqs.into_iter() {
                if whitelist_reserves.contains(&ev.debt_asset) {
                    entries.insert((ev.user, ev.debt_asset));
                }
            }
            let mut added_count = 0;
            for entry in entries.drain() {
                if !self.watch_list.contains(entry.0, entry.1) {
                    self.watch_list.add(entry).await?;
                    added_count += 1;
                    tracing::debug!("Added borrower {:?} (reserve {:?})", entry.0, entry.1);
                   
                }
             }
             if added_count > 0 {
                tracing::info!("Successfully indexed {} new Aave positions", added_count);
            }

            self.state.save_last_block(Protocol::Aave, current_end).await?;
            start_block = current_end + 1;


        }

        tracing::info!("Aave bootstrap complete");
        Ok(())
    }

    fn name(&self) -> &'static str {
        "aave"
    }
    
}