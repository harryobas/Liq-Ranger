pub mod aave_watch_list;
pub mod morpho_watch_list;

#[async_trait::async_trait]
pub trait WatchList<T>: Sync + Send {
     async fn remove(&self, item: T) -> anyhow::Result<()>;
     async fn add(&self, item: T) -> anyhow::Result<()>;
     async fn snapshot(&self) -> anyhow::Result<Vec<T>>;
}
