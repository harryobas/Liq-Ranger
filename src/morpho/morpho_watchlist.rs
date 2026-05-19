use async_trait::async_trait;
use std::collections::HashSet;
use std::sync::Arc;

use bincode;
use dashmap::DashMap;
use ethers::{
    core::rand,
    types::{Address, H256},
};
use sled::{Db, Tree};

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
            let market_ids = bincode::deserialize(&v)?;
            cache.insert(borrower, market_ids);
        }

        Ok(Self {
            db: Arc::new(tree),
            cache,
        })
    }

    /// Take a snapshot of all borrower → market_id pairs
    pub fn snapshot(&self) -> Vec<(Address, Vec<H256>)> {
        let out: Vec<(Address, Vec<H256>)> = self
            .cache
            .iter()
            .map(|entry| {
                let borrower = *entry.key();
                let markets = entry.value().iter().copied().collect::<Vec<H256>>();
                (borrower, markets)
            })
            .collect();

        out
    }

    /// Persist a specific borrower's set to sled
    async fn persist(&self, borrower: Address) -> anyhow::Result<()> {
        let db = self.db.clone();
        let maybe_set = { self.cache.get(&borrower).map(|v| v.value().clone()) };

        tokio::task::spawn_blocking(move || {
            if let Some(set) = maybe_set {
                let encoded = bincode::serialize(&set)?;
                db.insert(borrower.as_bytes(), encoded)?;
            } else {
                db.remove(borrower.as_bytes())?;
            }
            if rand::random::<u8>() % 32 == 0 {
                db.flush()?;
            }

            Ok::<_, anyhow::Error>(())
        })
        .await??;

        Ok(())
    }

    pub fn contains(&self, borrower: Address, market_id: H256) -> bool {
        self.cache
            .get(&borrower)
            .map(|set| set.contains(&market_id))
            .unwrap_or(false)
    }
}

#[async_trait]
impl WatchList<(Address, H256)> for MorphoWatchList {
    async fn add(&self, (borrower, market_id): (Address, H256)) -> anyhow::Result<()> {
        let mut set = self.cache.entry(borrower).or_default();
        if !set.insert(market_id) {
            tracing::warn!(?borrower, ?market_id, "Market already tracked");
            return Ok(());
        }
        drop(set);

        self.persist(borrower).await?;
        Ok(())
    }

    async fn remove(&self, (borrower, market_id): (Address, H256)) -> anyhow::Result<()> {
        if let Some(mut entry) = self.cache.get_mut(&borrower) {
            if !entry.remove(&market_id) {
                tracing::debug!(?borrower, ?market_id, "Market not found during removal");
                return Ok(());
            }

            let empty = entry.is_empty();
            drop(entry);

            if empty {
                self.cache.remove(&borrower);
            }

            self.persist(borrower).await?;
            return Ok(());
        }

        tracing::warn!(?borrower, ?market_id, "Borrower not found during removal");
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn addr(n: u64) -> Address {
        Address::from_low_u64_be(n)
    }

    fn market(n: u64) -> H256 {
        H256::from_low_u64_be(n)
    }

    fn test_db() -> (tempfile::TempDir, Arc<Db>) {
        let dir = tempfile::tempdir().expect("tempdir");
        let db = Arc::new(sled::open(dir.path()).expect("open sled"));
        (dir, db)
    }

    #[tokio::test]
    async fn add_market_is_idempotent() {
        let (_dir, db) = test_db();
        let list = MorphoWatchList::new(db).expect("watchlist");
        let borrower = addr(1);
        let market_id = market(2);

        list.add((borrower, market_id)).await.expect("first add");
        list.add((borrower, market_id)).await.expect("second add");

        let snapshot = list.snapshot();
        assert_eq!(snapshot.len(), 1);
        assert_eq!(snapshot[0].1.len(), 1);
        assert!(list.contains(borrower, market_id));
    }

    #[tokio::test]
    async fn remove_market_cleans_empty_borrower() {
        let (_dir, db) = test_db();
        let list = MorphoWatchList::new(db).expect("watchlist");
        let borrower = addr(3);
        let market_id = market(4);

        list.add((borrower, market_id)).await.expect("add");
        list.remove((borrower, market_id)).await.expect("remove");

        assert!(!list.contains(borrower, market_id));
        assert!(list.snapshot().is_empty());
    }

    #[tokio::test]
    async fn reload_restores_sled_state() {
        let (_dir, db) = test_db();
        let borrower = addr(5);
        let market_id = market(6);

        let list = MorphoWatchList::new(db.clone()).expect("watchlist");
        list.add((borrower, market_id)).await.expect("add");
        db.flush().expect("flush");

        let reloaded = MorphoWatchList::new(db).expect("reload");
        assert!(reloaded.contains(borrower, market_id));
    }
}
