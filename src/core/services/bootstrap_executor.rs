use crate::core::ports::Bootstrap;
use futures_util::future::try_join_all;
use std::sync::Arc;

pub struct BootstrapExecutor {
    pub bootstrapers: Vec<Arc<dyn Bootstrap>>,
}

impl BootstrapExecutor {
    pub async fn run_all(&self) -> anyhow::Result<()> {
        tracing::info!("Starting bootstrap process for all protocols concurrently...");

        // Step 1: Map every dynamic bootstrap instance directly to a scoped, tracked future task
        let tasks: Vec<_> = self
            .bootstrapers
            .iter()
            .map(|bootstrap| {
                let b = bootstrap.clone();
                let name = b.name();

                tracing::info!("Spawning {} bootstrap worker", name);

                // Wrap each execution loop into a dedicated thread-safe Tokio runtime task
                tokio::spawn(async move {
                    b.run().await.map_err(|e| {
                        tracing::error!(
                            "Bootstrap worker failure on chain variant [{name}]: {:?}",
                            e
                        );
                        anyhow::anyhow!("Protocol system [{name}] failed to initialize: {e}")
                    })
                })
            })
            .collect();

        // Step 2: Await execution joins simultaneously.
        // If any thread panics during runtime, catch it instantly via the internal JoinHandle layer.
        let join_results = try_join_all(tasks).await?;

        // Step 3: Iterate through individual execution returns to enforce fail-fast constraints.
        // The first error encountered terminates the application immediately.
        for result in join_results {
            result?;
        }

        tracing::info!("🔥 All protocol data bootstraps verified and synchronized successfully.");
        Ok(())
    }
}
