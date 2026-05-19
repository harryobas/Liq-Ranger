use std::sync::Arc;

use async_trait::async_trait;
use dashmap::{DashMap, DashSet};
use ethers::{
    core::rand,
    types::{Address, U256},
};
use sled::{Db, Tree};

use crate::common::WatchList;

/// Compound v3 Absorb → BuyCollateral reserve book
///
/// Maintains:
///     collateral_asset → total_available_amount
///
/// Backed by:
///     - sled (persistent)
///     - DashMap (in-memory fast access)
///
pub struct CompoundWatchList {
    db: Arc<Tree>,
    cache: Arc<DashMap<Address, U256>>,
}

impl CompoundWatchList {
    /// Initialize watchlist from sled DB
    pub fn new(db: Arc<Db>) -> anyhow::Result<Self> {
        let tree = db.open_tree("compound:reserves")?;
        let cache = Arc::new(DashMap::new());

        // Load persisted state into memory
        for item in tree.iter() {
            let (k, v) = item?;
            let asset = Address::from_slice(&k);
            let amount = bytes_to_u256(&v);
            cache.insert(asset, amount);
        }

        Ok(Self {
            db: Arc::new(tree),
            cache,
        })
    }

    /// Snapshot for engine evaluation
    /// Used by CompoundAbsorbEngine
    pub fn snapshot(&self) -> Vec<(Address, U256)> {
        self.cache
            .iter()
            .map(|entry| (*entry.key(), *entry.value()))
            .collect()
    }

    /// Get single asset amount (fast path)
    pub fn get(&self, asset: Address) -> Option<U256> {
        self.cache.get(&asset).map(|v| *v)
    }

    /// Internal persist helper
    async fn persist(&self, asset: Address) -> anyhow::Result<()> {
        let db = self.db.clone();
        let maybe_amount = { self.cache.get(&asset).map(|v| *v) };

        tokio::task::spawn_blocking(move || {
            if let Some(amount) = maybe_amount {
                db.insert(asset.as_bytes(), &u256_to_bytes(amount))?;
            } else {
                db.remove(asset.as_bytes())?;
            }
            if rand::random::<u8>() % 32 == 0 {
                db.flush()?;
            }
            Ok::<_, anyhow::Error>(())
        })
        .await??;

        Ok(())
    }
}

#[async_trait]
impl WatchList<(Address, U256)> for CompoundWatchList {
    /// Called on AbsorbCollateral
    async fn add(&self, (asset, amount): (Address, U256)) -> anyhow::Result<()> {
        let mut entry = self.cache.entry(asset).or_insert(U256::zero());

        *entry += amount;

        drop(entry);

        self.persist(asset).await?;
        Ok(())
    }

    /// Called on BuyCollateral
    async fn remove(&self, (asset, amount): (Address, U256)) -> anyhow::Result<()> {
        if let Some(mut entry) = self.cache.get_mut(&asset) {
            if *entry <= amount {
                // Fully bought (or slight overshoot safety)
                drop(entry);
                self.cache.remove(&asset);
            } else {
                *entry -= amount;
                drop(entry);
            }

            self.persist(asset).await?;
        }

        Ok(())
    }
}

/// ---------------------------
/// Encoding Helpers
/// ---------------------------

fn u256_to_bytes(value: U256) -> [u8; 32] {
    let mut buf = [0u8; 32];
    value.to_big_endian(&mut buf);
    buf
}

fn bytes_to_u256(bytes: &[u8]) -> U256 {
    U256::from_big_endian(bytes)
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

    #[test]
    fn u256_round_trips_big_endian() {
        let value = U256::from_dec_str("123456789123456789123456789").expect("u256");
        assert_eq!(bytes_to_u256(&u256_to_bytes(value)), value);
    }

    #[tokio::test]
    async fn add_accumulates_amounts() {
        let (_dir, db) = test_db();
        let list = CompoundWatchList::new(db).expect("watchlist");
        let asset = addr(1);

        list.add((asset, U256::from(10u64))).await.expect("add");
        list.add((asset, U256::from(15u64))).await.expect("add");

        assert_eq!(list.get(asset), Some(U256::from(25u64)));
    }

    #[tokio::test]
    async fn remove_decrements_or_deletes() {
        let (_dir, db) = test_db();
        let list = CompoundWatchList::new(db).expect("watchlist");
        let asset = addr(2);

        list.add((asset, U256::from(25u64))).await.expect("add");
        list.remove((asset, U256::from(10u64)))
            .await
            .expect("partial remove");
        assert_eq!(list.get(asset), Some(U256::from(15u64)));

        list.remove((asset, U256::from(15u64)))
            .await
            .expect("full remove");
        assert_eq!(list.get(asset), None);
    }

    #[tokio::test]
    async fn reload_restores_amounts() {
        let (_dir, db) = test_db();
        let asset = addr(3);

        let list = CompoundWatchList::new(db.clone()).expect("watchlist");
        list.add((asset, U256::from(42u64))).await.expect("add");
        db.flush().expect("flush");

        let reloaded = CompoundWatchList::new(db).expect("reload");
        assert_eq!(reloaded.get(asset), Some(U256::from(42u64)));
    }
}
