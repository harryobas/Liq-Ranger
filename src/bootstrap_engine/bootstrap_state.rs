use anyhow::Result;
use sled::{Db, Tree};
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

#[cfg(test)]
mod tests {
    use super::*;

    fn test_state() -> (tempfile::TempDir, BootstrapState) {
        let dir = tempfile::tempdir().expect("tempdir");
        let db = Arc::new(sled::open(dir.path()).expect("open sled"));
        let state = BootstrapState::new(db).expect("state");
        (dir, state)
    }

    #[tokio::test]
    async fn missing_protocol_returns_none() {
        let (_dir, state) = test_state();
        assert_eq!(
            state.load_last_block(Protocol::Aave).await.expect("load"),
            None
        );
    }

    #[tokio::test]
    async fn save_then_load_last_block() {
        let (_dir, state) = test_state();

        state
            .save_last_block(Protocol::Aave, 123_456)
            .await
            .expect("save");

        assert_eq!(
            state.load_last_block(Protocol::Aave).await.expect("load"),
            Some(123_456)
        );
    }

    #[tokio::test]
    async fn protocol_keys_do_not_collide() {
        let (_dir, state) = test_state();

        state
            .save_last_block(Protocol::Aave, 100)
            .await
            .expect("save aave");
        state
            .save_last_block(Protocol::Morpho, 200)
            .await
            .expect("save morpho");

        assert_eq!(
            state.load_last_block(Protocol::Aave).await.expect("load"),
            Some(100)
        );
        assert_eq!(
            state.load_last_block(Protocol::Morpho).await.expect("load"),
            Some(200)
        );
        assert_eq!(
            state
                .load_last_block(Protocol::Compound)
                .await
                .expect("load"),
            None
        );
    }
}
