use std::{collections::HashSet, sync::Arc};
use tokio::time::{sleep, Duration};

use ethers::providers::Middleware;

use crate::{
    common::abi_bindings::{
        i_aave_v3_pool::{BorrowFilter, RepayFilter},
        IAaveV3Pool, LiquidationCallFilter,
    },
    constants,
    watchlists::{aave_watchlist::AaveWatchList, bootstrap_state::BootstrapState},
};

use crate::core::{
    ports::{Bootstrap, ProtocolWatchList},
    types::Protocol,
};

pub struct AaveBootstrapAdapter<M> {
    aave: IAaveV3Pool<M>,
    watch_list: Arc<AaveWatchList>,
    state: Arc<BootstrapState>,
    provider: Arc<M>,
    deploy_block: u64,
}

impl<M: Middleware + 'static> AaveBootstrapAdapter<M> {
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
            deploy_block: constants::AAVE_DEPLOY_BLOCK,
        }
    }

    async fn fetch_batch(
        &self,
        start_block: u64,
        end_block: u64,
    ) -> anyhow::Result<(
        Vec<BorrowFilter>,
        Vec<RepayFilter>,
        Vec<LiquidationCallFilter>,
    )> {
        let mut attempts = 0;

        loop {
            let borrow_filter = self
                .aave
                .borrow_filter()
                .from_block(start_block)
                .to_block(end_block);

            let repay_filter = self
                .aave
                .repay_filter()
                .from_block(start_block)
                .to_block(end_block);

            let liq_filter = self
                .aave
                .liquidation_call_filter()
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
                        "⚠️ Aave RPC error [{} → {}] (attempt {}): {:?}",
                        start_block,
                        end_block,
                        attempts,
                        e
                    );
                    if attempts >= 5 {
                        return Err(anyhow::anyhow!(
                            "Aave RPC failed after retries [{} → {}]: {}",
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
impl<M: Middleware + 'static> Bootstrap for AaveBootstrapAdapter<M> {
    async fn run(&self) -> anyhow::Result<()> {
        tracing::info!("Starting aave bootstrap");
        let whitelist_reserves = &*constants::AAVE_RESERVES;
        let last_block = self.state.load_last_block(Protocol::Aave).await?;

        let latest_block = self.provider.get_block_number().await?.as_u64();

        // Safe 20-block window overlap adjustment to verify marginal logs across restarts
        let mut start_block = last_block.unwrap_or(self.deploy_block).saturating_sub(20);
        let batch_size = 1_000u64;

        while start_block <= latest_block {
            let current_end = (start_block + batch_size).min(latest_block);
            let mut entries: HashSet<String> = HashSet::new();

            tracing::info!("Aave bootstrap scanning {} -> {}", start_block, current_end);

            let (borrows, repays, liqs) = self.fetch_batch(start_block, current_end).await?;

            for ev in borrows.into_iter() {
                if whitelist_reserves.contains(&ev.reserve) {
                    // Assuming target identity text mapping mirrors your make_identity layout
                    let identity = format!("aave:{:?}:{:?}", ev.on_behalf_of, ev.reserve);
                    entries.insert(identity);
                }
            }

            for ev in repays.into_iter() {
                if whitelist_reserves.contains(&ev.reserve) {
                    let identity = format!("aave:{:?}:{:?}", ev.user, ev.reserve);
                    entries.insert(identity);
                }
            }

            for ev in liqs.into_iter() {
                if whitelist_reserves.contains(&ev.debt_asset) {
                    let identity = format!("aave:{:?}:{:?}", ev.user, ev.debt_asset);
                    entries.insert(identity);
                }
            }

            let mut added_count = 0;
            for entry in entries.drain() {
                // Check direct low-level state before mutating
                if !self.watch_list.contains_identity(&entry) {
                    self.watch_list.add(&entry).await?;
                    added_count += 1;

                    // Safe tracing that avoids split parsing logic entirely
                    tracing::debug!(identity = ?entry, "Added new active track identity");
                }
            }

            if added_count > 0 {
                tracing::info!("Successfully indexed {} new Aave positions", added_count);
            }

            self.state
                .save_last_block(Protocol::Aave, current_end)
                .await?;
            start_block = current_end + 1;
        }

        tracing::info!("Aave bootstrap complete");
        Ok(())
    }

    fn name(&self) -> &'static str {
        "aave"
    }
}
