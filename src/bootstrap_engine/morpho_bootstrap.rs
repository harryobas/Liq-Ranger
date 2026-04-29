use std::{collections::HashSet, sync::Arc};
use ethers::{types::{H256, Address}, providers::Middleware};

use crate::{
    common::WatchList, constants, morpho::{abi_bindings::IMorphoBlue, 
        morpho_watchlist::MorphoWatchList}
    };

use super::{
    bootstrap_state::BootstrapState,
    Bootstrap,
    Protocol
};

pub struct MorphoBootstrap<M> {
    morpho: IMorphoBlue<M>,
    watch_list: Arc<MorphoWatchList>,
    state: Arc<BootstrapState>,
    provider: Arc<M>,
    deploy_block: u64,

}

impl<M: Middleware + 'static> MorphoBootstrap<M> {

    pub fn new(
        morpho: IMorphoBlue<M>,
        watch_list: Arc<MorphoWatchList>,
        state: Arc<BootstrapState>,
        provider: Arc<M>,
    ) -> Self {
        Self {
            morpho,
            watch_list,
            state,
            provider,
            deploy_block: constants::MORPHO_DEPLOY_BLOCK
        }
    }

}

#[async_trait::async_trait]
impl<M: Middleware + 'static> Bootstrap for MorphoBootstrap<M>  {

    async fn run(&self) -> anyhow::Result<()> {
        tracing::info!("Starting Morpho Bootstrap");

        let whitelist_markets = &*constants::MORPHO_MARKETS;
        let last_block = self.state.load_last_block(Protocol::Morpho).await?;

        let latest_block = self.provider.get_block_number().await?.as_u64();

        let mut start_block = last_block
            .unwrap_or(self.deploy_block)
            .saturating_sub(20);

        let batch_size = 2_000u64;
        let mut entries: HashSet<(Address, H256)> = HashSet::new();

        while  start_block <= latest_block {
            let current_end = (start_block + batch_size).min(latest_block);
            
            tracing::info!("Morpho bootstrap scanning {} -> {}", start_block, current_end);

            let borrow_filter = self.morpho
                .borrow_filter()
                .from_block(start_block)
                .to_block(current_end);

            let repay_filter = self.morpho
                .repay_filter()
                .from_block(start_block)
                .to_block(current_end);

            let liq_filter = self.morpho
                .liquidate_filter()
                .from_block(start_block)
                .to_block(current_end);

            let (borrows, repays, liqs) = tokio::try_join!(
                borrow_filter.query(),
                repay_filter.query(),
                liq_filter.query(),
            )?;

            for ev in borrows.into_iter() {
                let market_id = H256::from(ev.id);

                if whitelist_markets.contains(&market_id) {
                    entries.insert((ev.on_behalf, market_id));
                }
            }

            for ev in repays.into_iter() {
                let market_id = H256::from(ev.id);

                if whitelist_markets.contains(&market_id) {
                    entries.insert((ev.on_behalf, market_id));
                }
            }

            for ev in liqs.into_iter() {
                 let market_id = H256::from(ev.id);

                 if whitelist_markets.contains(&market_id) {
                    entries.insert((ev.borrower, market_id));
                }

            }

             for entry in entries.drain() {
                self.watch_list.add(entry).await?;
             }

            self.state.save_last_block(Protocol::Morpho, current_end).await?;
            start_block = current_end + 1;

        }

        tracing::info!("Morpho bootstrap complete");

        Ok(())    

    }
    
    fn name(&self) -> &'static str {
        "morpho"
    }
}

   
