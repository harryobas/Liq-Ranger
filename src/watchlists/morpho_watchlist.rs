use async_trait::async_trait;
use std::collections::HashSet;
use std::str::FromStr;
use std::sync::Arc;

use dashmap::DashMap;
use ethers::types::{Address, H256};
use sled::{Db, Tree};

use crate::core::{ports::ProtocolWatchList, types::TrackerIdentity};

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
            let market_ids: HashSet<H256> = bincode::deserialize(&v)?;
            cache.insert(borrower, market_ids);
        }

        Ok(Self {
            db: Arc::new(tree),
            cache,
        })
    }
    /// Persist a specific borrower's set to sled
    async fn persist(&self, borrower: Address) -> anyhow::Result<()> {
        let db = self.db.clone();
        let maybe_set = self.cache.get(&borrower).map(|v| v.value().clone());

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

    /// Low-level direct lookup optimized for hot trading loops
    pub fn contains(&self, borrower: Address, market_id: H256) -> bool {
        self.cache
            .get(&borrower)
            .map(|set| set.contains(&market_id))
            .unwrap_or(false)
    }

    /// High-level string lookup for network boundary parsing
    pub fn contains_identity(&self, identity: &str) -> bool {
        if let Ok(TrackerIdentity::MorphoBlue {
            market_id,
            borrower,
        }) = TrackerIdentity::from_str(identity)
        {
            self.contains(borrower, market_id)
        } else {
            false
        }
    }
}

#[async_trait]
impl ProtocolWatchList for MorphoWatchList {
    async fn add(&self, identity: &str) -> anyhow::Result<()> {
        if let Ok(TrackerIdentity::MorphoBlue {
            market_id,
            borrower,
        }) = TrackerIdentity::from_str(identity)
        {
            let was_inserted = {
                let mut set = self.cache.entry(borrower).or_default();
                set.insert(market_id)
            };

            if !was_inserted {
                tracing::warn!(?borrower, ?market_id, "Market already tracked");
                return Ok(());
            }

            self.persist(borrower).await?;
        }
        Ok(())
    }

    async fn remove(&self, identity: &str) -> anyhow::Result<()> {
        if let Ok(TrackerIdentity::MorphoBlue {
            market_id,
            borrower,
        }) = TrackerIdentity::from_str(identity)
        {
            let mut was_removed = false;
            let mut should_delete_key = false;

            if let Some(mut entry) = self.cache.get_mut(&borrower) {
                was_removed = entry.remove(&market_id);
                should_delete_key = entry.is_empty();
            }

            if was_removed {
                if should_delete_key {
                    // Drop map entry lock before calling remove_if to avoid deadlocks
                    self.cache.remove_if(&borrower, |_, set| set.is_empty());
                }
                self.persist(borrower).await?;
            } else {
                tracing::debug!(?borrower, ?market_id, "Market not found during removal");
            }
        }
        Ok(())
    }

    fn snapshot(&self) -> Vec<String> {
        let mut out = Vec::with_capacity(self.cache.len());

        for entry in self.cache.iter() {
            let borrower = *entry.key();
            for market_id in entry.value().iter() {
                let identity = TrackerIdentity::MorphoBlue {
                    borrower,
                    market_id: *market_id,
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

    fn market(n: u64) -> H256 {
        H256::from_low_u64_be(n)
    }

    fn make_identity(borrower: Address, market_id: H256) -> String {
        TrackerIdentity::MorphoBlue {
            borrower,
            market_id,
        }
        .to_string_id()
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
        let identity = make_identity(borrower, market_id);

        list.add(&identity).await.expect("first add");
        list.add(&identity).await.expect("second add");

        let snapshot = list.snapshot();
        assert_eq!(snapshot.len(), 1);
        assert_eq!(snapshot[0], identity);
        assert!(list.contains(borrower, market_id));
        assert!(list.contains_identity(&identity));
    }

    #[tokio::test]
    async fn remove_market_cleans_empty_borrower() {
        let (_dir, db) = test_db();
        let list = MorphoWatchList::new(db).expect("watchlist");
        let borrower = addr(3);
        let market_id = market(4);
        let identity = make_identity(borrower, market_id);

        list.add(&identity).await.expect("add");
        list.remove(&identity).await.expect("remove");

        assert!(!list.contains(borrower, market_id));
        assert!(!list.contains_identity(&identity));
        assert!(list.snapshot().is_empty());
    }

    #[tokio::test]
    async fn reload_restores_sled_state() {
        let (_dir, db) = test_db();
        let borrower = addr(5);
        let market_id = market(6);
        let identity = make_identity(borrower, market_id);

        let list = MorphoWatchList::new(db.clone()).expect("watchlist");
        list.add(&identity).await.expect("add");
        db.flush().expect("flush");

        let reloaded = MorphoWatchList::new(db).expect("reload");
        assert!(reloaded.contains(borrower, market_id));
    }
}
