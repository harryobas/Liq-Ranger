use anyhow::Result;
use sled::{Tree, Db};
use std::sync::Arc;

use super::Protocol;

pub struct BootstrapState {
    db: Arc<Tree>,
}

impl BootstrapState {

    pub fn new(db: Arc<Db>) -> Result<Self> {
        let tree = db.open_tree("bootstrap:state")?;
        Ok(Self { db: Arc::new(tree) })
    }

    fn key(protocol: Protocol) -> Result<Vec<u8>> {
        Ok(bincode::serialize(&protocol)?)
    }

    pub async fn load_last_block(&self, protocol: Protocol) -> Result<Option<u64>> {
        let db = self.db.clone();
        let key = Self::key(protocol)?;

        tokio::task::spawn_blocking(move || -> Result<Option<u64>> {
            if let Some(bytes) = db.get(key)? {
                Ok(Some(bincode::deserialize(&bytes)?))
            } else {
                Ok(None)
            }
        })
        .await?
    }

    pub async fn save_last_block(&self, protocol: Protocol, block: u64) -> Result<()> {
        let db = self.db.clone();
        let key = Self::key(protocol)?;
        let value = bincode::serialize(&block)?;

        tokio::task::spawn_blocking(move || -> Result<()> {
            db.insert(key, value)?;
            Ok(())
        })
        .await?
    }
}