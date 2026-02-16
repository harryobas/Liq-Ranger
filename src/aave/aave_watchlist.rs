use std::collections::HashSet;
use std::sync::Arc;

use dashmap::DashMap;
use ethers::types::Address;
use sled::{Db, Tree};
use async_trait::async_trait;
use bincode;

use crate::common::WatchList;

pub struct AaveWatchList {
    db: Arc<Tree>,
    cache: Arc<DashMap<Address, HashSet<Address>>>,
}

impl AaveWatchList {
    pub fn new(db: Arc<Db>) -> anyhow::Result<Self> {
        let tree = db.open_tree("aave:watchlist")?;
        let cache = Arc::new(DashMap::new());

        // Load all Sled rows into memory
        for item in tree.iter() {
            let (k, v) = item?;
            let borrower = Address::from_slice(&k);
            let reserves: HashSet<Address> = bincode::deserialize(&v)?;
            cache.insert(borrower, reserves);
        }

        Ok(Self {
            db: Arc::new(tree),
            cache,
        })
    }

    /// Snapshot of all borrower→reserve pairs
    pub fn snapshot(&self) -> Vec<(Address, Address)> {
        let mut out = Vec::new();
        for entry in self.cache.iter() {
            let borrower = *entry.key();
            for reserve in entry.value().iter() {
                out.push((borrower, *reserve));
            }
        }
        out
    }

      async fn persist(&self, borrower: Address) -> anyhow::Result<()> {
        let db = self.db.clone();
        let maybe_set = self.cache
            .get(&borrower).map(|v| v.clone());

        tokio::task::spawn_blocking(move || {
            if let Some(set) = maybe_set {
                let encoded = bincode::serialize(&set)?;
                db.insert(borrower.as_bytes(), encoded)?;
            } else {
                db.remove(borrower.as_bytes())?;
            }
            Ok::<_, anyhow::Error>(())
        })
        .await??;

        Ok(())
    }
}

#[async_trait]
impl WatchList<(Address, Address)> for AaveWatchList {
    async fn add(&self, (borrower, reserve): (Address, Address)) -> anyhow::Result<()> {
        let mut set = self.cache.entry(borrower).or_default();

        if !set.insert(reserve) {
            //anyhow::bail!("Already exists.");
            return Ok(());
        }

        drop(set);

        self.persist(borrower).await?;
        Ok(())
    }

    async fn remove(&self, (borrower, reserve): (Address, Address)) -> anyhow::Result<()> {

        if let Some(mut entry) = self.cache.get_mut(&borrower) {

            if !entry.remove(&reserve) {
                return Ok(());
            }

            let empty = entry.is_empty();
             drop(entry);

            if empty {
                self.cache.remove(&borrower);
            }

            self.persist(borrower).await?;
        }

        Ok(())
    }

}


