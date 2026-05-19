use ethers::{providers::Middleware, types::U256};
use std::sync::Arc;

use crate::{
    common::WatchList,
    compound::{abi_bindings::IComet, compound_watchlist::CompoundWatchList},
    constants,
};

use super::Bootstrap;

pub struct CompoundBootstrap<M> {
    pub compound: IComet<M>,
    pub watch_list: Arc<CompoundWatchList>,
}

impl<M: Middleware + 'static> CompoundBootstrap<M> {
    pub fn new(compound: IComet<M>, watch_list: Arc<CompoundWatchList>) -> Self {
        Self {
            compound,
            watch_list,
        }
    }
}

#[async_trait::async_trait]
impl<M: Middleware + 'static> Bootstrap for CompoundBootstrap<M> {
    async fn run(&self) -> anyhow::Result<()> {
        tracing::info!("Starting Compound Buy-Collateral Bootstrap");

        // Use the constants for the specific collateral assets supported by this Comet instance
        let assets = &*constants::COMPOUND_COLLATERAL_ASSETS;

        for &asset in assets {
            // Check protocol inventory
            match self.compound.get_collateral_reserves(asset).await {
                Ok(reserves) if reserves > U256::zero() => {
                    tracing::info!("Asset {:?} has {:?} available in reserves", asset, reserves);
                    self.watch_list.add((asset, reserves)).await?;
                }
                Ok(_) => tracing::debug!("No reserves for asset {:?}", asset),
                Err(e) => tracing::error!("Failed to fetch reserves for {:?}: {}", asset, e),
            }
        }

        tracing::info!("Compound Buy-Collateral bootstrap complete");
        Ok(())
    }

    fn name(&self) -> &'static str {
        "Compound"
    }
}
