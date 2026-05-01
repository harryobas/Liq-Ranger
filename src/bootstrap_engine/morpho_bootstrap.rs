use std::{collections::HashSet, sync::Arc};
use tokio::time::{sleep, Duration};
use ethers::{types::{H256, Address}, providers::Middleware};

use crate::{
    common::WatchList, constants, morpho::{abi_bindings::{IMorphoBlue, BorrowFilter, RepayFilter, LiquidateFilter}, 
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

    async fn fetch_batch(
        &self, 
        morpho: &IMorphoBlue<M>, 
        start_block: u64, 
        end_block: u64
    ) -> anyhow::Result<(Vec<BorrowFilter>, Vec<RepayFilter>, Vec<LiquidateFilter>)> {
        let mut attempts = 0;

        loop {
             let borrow_filter = morpho
            .borrow_filter()
            .from_block(start_block)
            .to_block(end_block);

            let repay_filter = morpho
            .repay_filter()
            .from_block(start_block)
            .to_block(end_block);

            let liq_filter = morpho
            .liquidate_filter()
            .from_block(start_block)
            .to_block(end_block);

            match tokio::try_join!(
                borrow_filter.query(),
                repay_filter.query(),
                liq_filter.query(),
            ) {
                Ok(res) => return Ok(res),
                Err(e) => {
                    attempts += 1;
                     tracing::warn!(
                    "⚠️ Morpho RPC error [{} → {}] (attempt {}): {:?}",
                    start_block,
                    end_block,
                    attempts,
                    e
                );
                    if attempts >= 5 {
                        return Err(anyhow::anyhow!(
                        "Morpho RPC failed after retries [{} → {}]: {}",
                        start_block,
                        end_block,
                        e
                      ));
                    }
                    tracing::warn!("Morpho RPC error at block {}: {}. Retrying... (Attempt {}/{})", start_block, e, attempts, 5);
                     // exponential backoff
                    sleep(Duration::from_secs(2 * attempts)).await;
                }
            }
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
        
        while  start_block <= latest_block {
            let current_end = (start_block + batch_size).min(latest_block);
            let mut entries: HashSet<(Address, H256)> = HashSet::new();
            
            tracing::info!("Morpho bootstrap scanning {} -> {}", start_block, current_end);

            let (borrows, repays, liqs) = self.fetch_batch(
                &self.morpho, 
                start_block, 
                current_end
            ).await?;

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

            let mut added_count = 0;

             for entry in entries.drain() {
                if !self.watch_list.contains(entry.0, entry.1) {
                    self.watch_list.add(entry).await?;
                    added_count += 1;
                    tracing::debug!("Added borrower {:?} (market_id {:?})", entry.0, entry.1);
                }
               
             }
             if added_count > 0 {
                tracing::info!("Successfully indexed {} new Morpho positions", added_count);
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

   
