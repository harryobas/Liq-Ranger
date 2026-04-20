use dotenv::dotenv;
use tracing_subscriber::{fmt, EnvFilter};

#[tokio::main(flavor = "multi_thread")]
async fn main() -> anyhow::Result<()> {
    dotenv().ok();

    fmt()
        .with_env_filter(EnvFilter::from_default_env())
        .init();

    tracing::info!("🚀 Starting liquidation mining");

    if let Err(e) = liq_ranger::start_liquidation_engines().await {
        tracing::error!("❌ Engine crashed: {:?}", e);
    }

    Ok(())
}
