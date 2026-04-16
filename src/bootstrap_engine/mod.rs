pub mod aave_bootstrap;
pub mod morpho_bootstrap;
pub mod compound_bootstrap;
pub mod bootstrap_state;

use std::sync::Arc;
use serde::{Serialize, Deserialize};

#[async_trait::async_trait] 
pub trait Bootstrap: Send + Sync { 
    async fn run(&self) -> anyhow::Result<()>; 
    fn name(&self) -> &'static str; 
}


#[derive(Serialize, Deserialize, Debug, Clone, Copy)]
pub enum Protocol {
    Morpho,
    Aave,
    Compound,
}

pub struct BootstrapExecutor {
    pub bootstraps: Vec<Arc<dyn Bootstrap>>,
}

impl BootstrapExecutor {
    pub async fn run_all(&self) -> anyhow::Result<()> {
        let mut handles = Vec::new();

        for bootstrap in &self.bootstraps {
            let b = bootstrap.clone();
            let name = b.name();

            tracing::info!("Spawning {} bootstrap", name);

            let handle = tokio::spawn(async move {
                let result = b.run().await;
                (name, result)
            });

            handles.push(handle);
        }

        for handle in handles {
            match handle.await {
                Ok((name, Ok(_))) => tracing::info!("Finished {} bootstrap", name),
                Ok((name, Err(e))) => tracing::error!("Bootstrap failed {}: {:?}", name, e),
                Err(e) => tracing::error!("Task panicked: {:?}", e),
            }
        }

        Ok(())
    }

    
}