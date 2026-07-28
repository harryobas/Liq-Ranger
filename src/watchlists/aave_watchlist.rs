use std::collections::HashSet;
use std::str::FromStr;
use std::sync::Arc;

use async_trait::async_trait;
use dashmap::DashMap;
use ethers::types::Address;
use sled::{Db, Tree};

use crate::core::ports::ProtocolWatchList;
use crate::core::types::TrackerIdentity;

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

    /// Primary Stage 1 Optimization:
    /// Returns unique borrowers directly to prevent duplicate portfolio health checks.
    pub fn unique_borrowers(&self) -> HashSet<Address> {
        self.cache.iter().map(|entry| *entry.key()).collect()
    }

    async fn persist(&self, borrower: Address) -> anyhow::Result<()> {
        let db = self.db.clone();

        // 1. Serialize in-memory data and drop DashMap reference immediately
        let maybe_encoded = match self.cache.get(&borrower) {
            Some(entry) => Some(bincode::serialize(entry.value())?),
            None => None,
        };

        // 2. Offload Sled write to blocking thread without blocking on disk flush
        tokio::task::spawn_blocking(move || -> anyhow::Result<()> {
            if let Some(encoded) = maybe_encoded {
                db.insert(borrower.as_bytes(), encoded)?;
            } else {
                db.remove(borrower.as_bytes())?;
            }
            // Sled manages background flushes automatically; no explicit db.flush() needed on hot path
            Ok(())
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

    pub fn contains_identity(&self, identity: &str) -> bool {
        if let Ok(TrackerIdentity::AaveV3 { borrower, reserve }) =
            TrackerIdentity::from_str(identity)
        {
            self.contains(borrower, reserve)
        } else {
            false
        }
    }
}

#[async_trait]
impl ProtocolWatchList for AaveWatchList {
    async fn add(&self, identity: &str) -> anyhow::Result<()> {
        let parsed = match TrackerIdentity::from_str(identity) {
            Ok(id) => id,
            Err(e) => {
                tracing::warn!(%identity, error = %e, "Failed to parse identity during add");
                return Ok(());
            }
        };

        if let TrackerIdentity::AaveV3 { borrower, reserve } = parsed {
            let was_inserted = {
                let mut set = self.cache.entry(borrower).or_default();
                set.insert(reserve)
            };

            if !was_inserted {
                tracing::debug!(?borrower, ?reserve, "Reserve already tracked");
                return Ok(());
            }

            self.persist(borrower).await?;
        }

        Ok(())
    }

    async fn remove(&self, identity: &str) -> anyhow::Result<()> {
        let parsed = match TrackerIdentity::from_str(identity) {
            Ok(id) => id,
            Err(e) => {
                tracing::warn!(%identity, error = %e, "Failed to parse identity during remove");
                return Ok(());
            }
        };

        if let TrackerIdentity::AaveV3 { borrower, reserve } = parsed {
            let (was_removed, should_delete_key) = {
                if let Some(mut entry) = self.cache.get_mut(&borrower) {
                    if entry.remove(&reserve) {
                        (true, entry.is_empty())
                    } else {
                        tracing::debug!(?borrower, ?reserve, "Reserve not found during removal");
                        (false, false)
                    }
                } else {
                    tracing::warn!(?borrower, ?reserve, "Borrower not found during removal");
                    (false, false)
                }
            };

            if was_removed {
                if should_delete_key {
                    self.cache.remove_if(&borrower, |_, set| set.is_empty());
                }

                self.persist(borrower).await?;
            }
        }
        Ok(())
    }

    fn snapshot(&self) -> Vec<String> {

        let mut out = Vec::with_capacity(self.cache.len());

        for entry in self.cache.iter() {
        let borrower = *entry.key();

        // Pick 1 representative reserve for this borrower
        if let Some(reserve) = entry.value().iter().next() {
            let identity = TrackerIdentity::AaveV3 {
                borrower,
                reserve: *reserve,
            }
            .to_string_id();
            out.push(identity);
        }
    }

    out
}


}

#[cfg(test)]
mod tests {
    use super::*;

    fn addr(n: u64) -> Address {
        Address::from_low_u64_be(n)
    }

    fn make_identity(borrower: Address, reserve: Address) -> String {
        TrackerIdentity::AaveV3 { borrower, reserve }.to_string_id()
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
        let identity = make_identity(borrower, reserve);

        list.add(&identity).await.expect("first add");
        list.add(&identity).await.expect("second add");

        let snapshot = list.snapshot();
        assert_eq!(snapshot.len(), 1);
        assert_eq!(snapshot[0], identity);
        assert!(list.contains(borrower, reserve));
    }

    #[tokio::test]
    async fn remove_last_reserve_deletes_borrower() {
        let (_dir, db) = test_db();
        let list = AaveWatchList::new(db).expect("watchlist");
        let borrower = addr(3);
        let reserve = addr(4);
        let identity = make_identity(borrower, reserve);

        list.add(&identity).await.expect("add");
        list.remove(&identity).await.expect("remove");

        assert!(!list.contains(borrower, reserve));
        assert!(list.snapshot().is_empty());
    }

    #[tokio::test]
    async fn reload_restores_sled_state() {
        let (_dir, db) = test_db();
        let borrower = addr(5);
        let reserve = addr(6);
        let identity = make_identity(borrower, reserve);

        let list = AaveWatchList::new(db.clone()).expect("watchlist");
        list.add(&identity).await.expect("add");

        let reloaded = AaveWatchList::new(db).expect("reload");
        assert!(reloaded.contains(borrower, reserve));
    }

    #[tokio::test]
    async fn unique_borrowers_deduplicates() {
        let (_dir, db) = test_db();
        let list = AaveWatchList::new(db).expect("watchlist");
        let borrower = addr(7);

        list.add(&make_identity(borrower, addr(10))).await.unwrap();
        list.add(&make_identity(borrower, addr(11))).await.unwrap();

        let unique = list.unique_borrowers();
        assert_eq!(unique.len(), 1);
        assert!(unique.contains(&borrower));
    }
}