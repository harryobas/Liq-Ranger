pub mod aave_liquidator;
pub mod morpho_blue_liquidator;


#[async_trait::async_trait]
pub trait Liquidator: Sync + Send {
    async fn run(&self) -> anyhow::Result<()>;

}