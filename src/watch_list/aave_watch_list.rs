use std::collections::HashSet;
use tokio::sync::RwLock;
use std::sync::Arc;

use ethers::{types::Address};
use crate::{
    config::{aave_config::AaveConfig},
    watch_list::SubgraphBootstrap,
    constants,
    models::borrow::BorrowsData
};
use super::WatchList;

pub struct AaveWatchList {
    inner: Arc<RwLock<HashSet<(Address, Address)>>>,

}

impl AaveWatchList {
    pub fn new() -> Self {
        let inner = Arc::new(RwLock::new(HashSet::new()));
        Self { inner }
    }
}

#[async_trait::async_trait]
impl WatchList<(Address, Address)> for AaveWatchList {

    async fn remove(&self, item: (Address, Address)) -> anyhow::Result<()> {

        let mut inner = self.inner.write().await;
        if inner.contains(&item) {
            inner.remove(&item);
            Ok(())
        }else {
            anyhow::bail!("Item {:?} not found in AaveWatchList", item)
        }

    }

    async fn add(&self, item: (Address, Address)) -> anyhow::Result<()> {
        let mut inner = self.inner.write().await;
        if inner.insert(item) {
            Ok(())
        }else {
            anyhow::bail!("Failed to add {:?} item to AaveWatchList", item);
        }

    }

    async fn snapshot(&self) -> anyhow::Result<Vec<(Address, Address)>> {
        let snap_shot: Vec<(Address, Address)> = self.inner
            .read()
            .await
            .iter()
            .cloned()
            .collect();

        Ok(snap_shot)

    }
    
}

#[async_trait::async_trait]
impl SubgraphBootstrap<Arc<AaveConfig>> for AaveWatchList {

    async fn bootstrap_from_subgraph(&self, config: Arc<AaveConfig>) -> anyhow::Result<()> {

        log::info!("Bootstrapping AaveWatchList from subgraph: {}", config.subgraph_url);

         let borrows_query = serde_json::json!({
            "query": constants::BORROWERS_QUERY_AAVE, 
            "variables": serde_json::json!({})
        });

      let resp = reqwest::Client::new()
            .post(config.subgraph_url.as_str())
            .header("Authorization", &format!("Bearer {}", config.subgraph_api_key.as_str()))
            .json(&borrows_query)
            .send()
            .await?
            .error_for_status()?
            .json::<serde_json::Value>()
            .await?;

    let raw_borrows = resp.get("data")
        .ok_or(anyhow::anyhow!("failed to get data"))?;

    let raw_borrows: BorrowsData = serde_json::from_value(raw_borrows.clone())?;

    raw_borrows.borrows.into_iter().for_each(|b| {
    match (b.account.id.parse::<Address>(), b.asset.id.parse::<Address>()) {
        (Ok(account), Ok(asset)) => {
            let item = (account, asset);
            self.add(item);
        }
        (Err(e_acc), Err(e_asset)) => {
            log::warn!("Failed to parse both account ({}) and asset ({}): {}, {}", 
                      b.account.id, b.asset.id, e_acc, e_asset);
        }
        (Err(e), _) => {
            log::warn!("Failed to parse account {}: {}", b.account.id, e);
        }
        (_, Err(e)) => {
            log::warn!("Failed to parse asset {}: {}", b.asset.id, e);
        }
    }
});

    Ok(())
}


}

