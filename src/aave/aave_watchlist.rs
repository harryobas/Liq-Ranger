use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use async_trait::async_trait;
use bincode;
use dashmap::DashMap;
use ethers::{core::rand, types::Address};
use sled::{Db, Tree};

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

    /// Snapshot of all borrower→reserve pair
    pub fn snapshot(&self) -> HashMap<Address, HashSet<Address>> {
        let mut out = HashMap::with_capacity(self.cache.len());
        for entry in self.cache.iter() {
            let borrower = *entry.key();
            out.insert(borrower, entry.value().clone());
        }
        out
    }

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

    pub fn contains(&self, borrower: Address, reserve: Address) -> bool {
        self.cache
            .get(&borrower)
            .map(|set| set.contains(&reserve))
            .unwrap_or(false)
    }
}

#[async_trait]
impl WatchList<(Address, Address)> for AaveWatchList {
    async fn add(&self, (borrower, reserve): (Address, Address)) -> anyhow::Result<()> {
        let mut set = self.cache.entry(borrower).or_default();

        if !set.insert(reserve) {
            tracing::debug!(?borrower, ?reserve, "Reserve already tracked");
            return Ok(());
        }

        drop(set);

        self.persist(borrower).await?;
        Ok(())
    }

    async fn remove(&self, (borrower, reserve): (Address, Address)) -> anyhow::Result<()> {
        if let Some(mut entry) = self.cache.get_mut(&borrower) {
            if !entry.remove(&reserve) {
                tracing::debug!(?borrower, ?reserve, "Reserve not found during removal");
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
        tracing::warn!(?borrower, ?reserve, "Borrower not found during removal");
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn addr(n: u64) -> Address {
        Address::from_low_u64_be(n)
    }

    fn test_db() -> (tempfile::TempDir, Arc<Db>) {
        let dir = tempfile::tempdir().expect("tempdir");
        let db = Arc::new(sled::open(dir.path()).expect("open sled"));
        (dir, db)
    }

    #[tokio::test]
    async fn add_is_idempotent() {
        let (_dir, db) = test_db();
        let list = AaveWatchList::new(db).expect("watchlist");
        let borrower = addr(1);
        let reserve = addr(2);

        list.add((borrower, reserve)).await.expect("first add");
        list.add((borrower, reserve)).await.expect("second add");

        let snapshot = list.snapshot();
        assert_eq!(snapshot.len(), 1);
        assert_eq!(snapshot.get(&borrower).expect("borrower").len(), 1);
        assert!(list.contains(borrower, reserve));
    }

    #[tokio::test]
    async fn remove_last_reserve_deletes_borrower() {
        let (_dir, db) = test_db();
        let list = AaveWatchList::new(db).expect("watchlist");
        let borrower = addr(3);
        let reserve = addr(4);

        list.add((borrower, reserve)).await.expect("add");
        list.remove((borrower, reserve)).await.expect("remove");

        assert!(!list.contains(borrower, reserve));
        assert!(list.snapshot().is_empty());
    }

    #[tokio::test]
    async fn reload_restores_sled_state() {
        let (_dir, db) = test_db();
        let borrower = addr(5);
        let reserve = addr(6);

        let list = AaveWatchList::new(db.clone()).expect("watchlist");
        list.add((borrower, reserve)).await.expect("add");
        db.flush().expect("flush");

        let reloaded = AaveWatchList::new(db).expect("reload");
        assert!(reloaded.contains(borrower, reserve));
    }
}
