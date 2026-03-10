use std::sync::Arc;

use async_trait::async_trait;
use dashmap::DashMap;
use ethers::{core::rand, types::{Address, U256}};
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
        let maybe_amount = {
            self.cache.get(&asset).map(|v| *v)
        };

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