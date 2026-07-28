use super::types::{BorrowerProfile, LiquidationJob, MarketQuote};
use async_trait::async_trait;
use ethers::types::{Address, H256, U256};



#[async_trait]
pub trait LendingProtocolReader: Send + Sync {
    async fn fetch_liquidation_candidates(&self) -> anyhow::Result<Vec<BorrowerProfile>>;
    async fn refresh_borrower(&self, identity: &str) -> anyhow::Result<BorrowerProfile>;
    fn name(&self) -> &'static str;
}

#[async_trait]
pub trait DexRouteFinder: Send + Sync {
    async fn get_swap_quote(
        &self,
        src_token: Address,
        dest_token: Address,
        src_decimals: u8,
        dest_decimals: u8,
        amount: U256,
    ) -> anyhow::Result<MarketQuote>;
}

#[async_trait]
pub trait EvmSimulator: Send + Sync {
    async fn simulate_liquidation(
        &self,
        block_number: u64,
        job: &LiquidationJob,
    ) -> anyhow::Result<u64>;
}

#[async_trait]
pub trait LiquidationContract: Send + Sync {
    async fn execute_liquidation(
        &self,
        job: LiquidationJob,
        gas_limit: U256,
    ) -> anyhow::Result<H256>;
}

#[async_trait::async_trait]
pub trait ProtocolWatchList: Sync + Send {
    async fn remove(&self, identity: &str) -> anyhow::Result<()>;
    async fn add(&self, identity: &str) -> anyhow::Result<()>;
    fn snapshot(&self) -> Vec<String>;
}

#[async_trait::async_trait]
pub trait Bootstrap: Send + Sync {
    async fn run(&self) -> anyhow::Result<()>;
    fn name(&self) -> &'static str;
}
