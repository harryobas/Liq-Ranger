use std::sync::Arc;
use ethers::{types::U256, providers::Middleware};

use crate::{
    common::WatchList, 
    constants, 
    compound::{abi_bindings::IComet, compound_watchlist::CompoundWatchList}, 
};

use super::Bootstrap;

pub struct CompoundBootstrap<M> {
    pub compound: IComet<M>,
    pub watch_list: Arc<CompoundWatchList>,
}

impl <M: Middleware + 'static> CompoundBootstrap<M> {
    pub fn new(
        compound: IComet<M>,
        watch_list: Arc<CompoundWatchList>,
    ) -> Self {
        Self {
            compound,
            watch_list,
        }
    }
    
}

#[async_trait::async_trait]
impl<M: Middleware + 'static> Bootstrap for CompoundBootstrap<M> {
    async fn run(&self) -> anyhow::Result<()> {
        tracing::info!("starting compound bootstrap");

        for asset in constants::COMPOUND_RESERVES.iter() {
            let current_inventory = self.compound.get_collateral_reserves(*asset).await?;
            tracing::debug!("Asset {} inventory: {:?}", asset, current_inventory);
            if current_inventory > U256::zero() {
                self.watch_list.add((*asset, current_inventory)).await?;
                tracing::info!("Added asset {} with inventory {:?}", asset, current_inventory);
            }

        }
        
        Ok(())

    }

     fn name(&self) -> &'static str {
        "Compound"
    }
}