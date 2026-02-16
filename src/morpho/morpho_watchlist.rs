use std::collections::HashSet;
use std::sync::Arc;
use async_trait::async_trait;

use dashmap::DashMap;
use ethers::{types::{Address, H256}};
use sled::{Db, Tree};
use bincode;

use crate::common::WatchList;

/// A watchlist for Morpho positions: borrower → set of market_ids
pub struct MorphoWatchList {
    db: Arc<Tree>,
    cache: Arc<DashMap<Address, HashSet<H256>>>,
}

impl MorphoWatchList {
    pub fn new(db: Arc<Db>) -> anyhow::Result<Self> {
        let tree = db.open_tree("morpho:watchlist")?;
        let cache = Arc::new(DashMap::new());

        // Load all entries from sled into memory
        for item in tree.iter() {
            let (k, v) = item?;
            let borrower = Address::from_slice(&k);
            let market_ids =  bincode::deserialize(&v)?;
            cache.insert(borrower, market_ids);
        }

        Ok(Self {
            db: Arc::new(tree),
            cache,
        })
    }

    /// Take a snapshot of all borrower → market_id pairs
    pub fn snapshot(&self) -> Vec<(Address, H256)> {
        let mut out = Vec::new();
        for entry in self.cache.iter() {
            let borrower = *entry.key();
            for market_id in entry.value().iter() {
                out.push((borrower, *market_id));
            }
        }
        out
    }

    /// Persist a specific borrower's set to sled
    async fn persist(&self, borrower: Address) -> anyhow::Result<()> {
        let db = self.db.clone();
        let maybe_set = self.cache.get(&borrower).map(|v| v.clone());

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
impl WatchList<(Address, H256)> for MorphoWatchList {
    async fn add(&self, (borrower, market_id): (Address, H256)) -> anyhow::Result<()> {
        let mut set = self.cache.entry(borrower).or_default();
        if !set.insert(market_id) {
            anyhow::bail!("Market_id already exists for borrower");
        }
        drop(set);

        self.persist(borrower).await?;
        Ok(())
    }

    async fn remove(&self, (borrower, market_id): (Address, H256)) -> anyhow::Result<()> {
        if let Some(mut entry) = self.cache.get_mut(&borrower) {
            if !entry.remove(&market_id) {
                anyhow::bail!("Market_id not found for borrower");
            }

            let empty = entry.is_empty();
            drop(entry);

            if empty {
                self.cache.remove(&borrower);
            }

            self.persist(borrower).await?;
            return Ok(());
        }

        anyhow::bail!("Borrower not found")
    }
    
}
