use std::{collections::HashSet, sync::Arc};
use tokio::time::{sleep, Duration};

use ethers::{providers::Middleware, types::H256};

use crate::{
    common::abi_bindings::{
        i_morpho_blue::{BorrowFilter, LiquidateFilter, RepayFilter},
        IMorphoBlue,
    },
    constants,
    watchlists::{bootstrap_state::BootstrapState, morpho_watchlist::MorphoWatchList},
};

use crate::core::{
    ports::{Bootstrap, ProtocolWatchList},
    types::Protocol,
};

pub struct MorphoBootstrapAdapter<M> {
    morpho: IMorphoBlue<M>,
    watch_list: Arc<MorphoWatchList>,
    state: Arc<BootstrapState>,
    provider: Arc<M>,
    deploy_block: u64,
}

impl<M: Middleware + 'static> MorphoBootstrapAdapter<M> {
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
            deploy_block: constants::MORPHO_DEPLOY_BLOCK,
        }
    }

    async fn fetch_batch(
        &self,
        start_block: u64,
        end_block: u64,
    ) -> anyhow::Result<(Vec<BorrowFilter>, Vec<RepayFilter>, Vec<LiquidateFilter>)> {
        let mut attempts = 0;

        loop {
            let borrow_filter = self
                .morpho
                .borrow_filter()
                .from_block(start_block)
                .to_block(end_block);

            let repay_filter = self
                .morpho
                .repay_filter()
                .from_block(start_block)
                .to_block(end_block);

            let liq_filter = self
                .morpho
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
                    // exponential backoff
                    sleep(Duration::from_secs(2 * attempts)).await;
                }
            }
        }
    }
}

#[async_trait::async_trait]
impl<M: Middleware + 'static> Bootstrap for MorphoBootstrapAdapter<M> {
    async fn run(&self) -> anyhow::Result<()> {
        tracing::info!("Starting Morpho Bootstrap");

        let whitelist_markets = &*constants::MORPHO_MARKETS;
        let last_block = self.state.load_last_block(Protocol::Morpho).await?;

        let latest_block = self.provider.get_block_number().await?.as_u64();

        // Safe overlap adjustment window to avoid edge log skips across reboots
        let mut start_block = last_block.unwrap_or(self.deploy_block).saturating_sub(20);
        let batch_size = 2_000u64;

        while start_block <= latest_block {
            let current_end = (start_block + batch_size).min(latest_block);

            // Collect matching event combinations as unified identity tracking strings
            let mut entries: HashSet<String> = HashSet::new();

            tracing::info!(
                "Morpho bootstrap scanning {} -> {}",
                start_block,
                current_end
            );

            let (borrows, repays, liqs) = self.fetch_batch(start_block, current_end).await?;

            for ev in borrows.into_iter() {
                let market_id = H256::from(ev.id);
                if whitelist_markets.contains(&market_id) {
                    let identity = format!("morpho:{:?}:{:?}", ev.on_behalf, market_id);
                    entries.insert(identity);
                }
            }

            for ev in repays.into_iter() {
                let market_id = H256::from(ev.id);
                if whitelist_markets.contains(&market_id) {
                    let identity = format!("morpho:{:?}:{:?}", ev.on_behalf, market_id);
                    entries.insert(identity);
                }
            }

            for ev in liqs.into_iter() {
                let market_id = H256::from(ev.id);
                if whitelist_markets.contains(&market_id) {
                    let identity = format!("morpho:{:?}:{:?}", ev.borrower, market_id);
                    entries.insert(identity);
                }
            }

            let mut added_count = 0;

            for entry in entries.drain() {
                // Check if the unique identifier is tracked using the interface method
                if !self.watch_list.contains_identity(&entry) {
                    self.watch_list.add(&entry).await?;
                    added_count += 1;
                    tracing::debug!(identity = ?entry, "Added new active track identity");
                }
            }

            if added_count > 0 {
                tracing::info!("Successfully indexed {} new Morpho positions", added_count);
            }

            self.state
                .save_last_block(Protocol::Morpho, current_end)
                .await?;
            start_block = current_end + 1;
        }

        tracing::info!("Morpho bootstrap complete");
        Ok(())
    }

    fn name(&self) -> &'static str {
        "morpho"
    }
}
