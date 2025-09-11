use std::collections::HashSet;
use tokio::sync::RwLock;
use std::sync::Arc;

use ethers::types::Address;

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
        if inner.remove(&item) {
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