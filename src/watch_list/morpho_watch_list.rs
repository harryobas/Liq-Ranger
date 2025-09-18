use std::collections::HashSet;
use tokio::sync::RwLock;
use std::sync::Arc;

use ethers::types::{Address, H256};

use super::WatchList;

pub struct MorphoWatchList {
    inner: Arc<RwLock<HashSet<(Address, H256)>>>,

}

impl MorphoWatchList {
    pub fn new() -> Self {
        let inner = Arc::new(RwLock::new(HashSet::new()));
        Self { inner }
    }
}

#[async_trait::async_trait]
impl WatchList<(Address, H256)> for MorphoWatchList {

    async fn remove(&self, item: (Address, H256)) -> anyhow::Result<()> {

        let mut inner = self.inner.write().await;
        if inner.contains(&item) {
            inner.remove(&item);
            Ok(())
        }else {
            anyhow::bail!("Item {:?} not found in MorphoWatchList", item)
        }

    }

    async fn add(&self, item: (Address, H256)) -> anyhow::Result<()> {
        let mut inner = self.inner.write().await;
        if inner.insert(item) {
            Ok(())
        }else {
            anyhow::bail!("Failed to add {:?} item to AaveWatchList", item);
        }

    }

    async fn snapshot(&self) -> anyhow::Result<Vec<(Address, H256)>> {
        let snap_shot: Vec<(Address, H256)> = self.inner
            .read()
            .await
            .iter()
            .cloned()
            .collect();

        Ok(snap_shot)

    }



    
}