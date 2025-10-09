

pub mod aave_liquidator;
pub mod morpho_blue_liquidator;


#[async_trait::async_trait]
pub trait Liquidator: Sync + Send {
    async fn run(&self) -> anyhow::Result<()>;

}

use tokio::sync::Semaphore;
use std::sync::Arc;
use crate::{abi_bindings::FlashLiquidator, config::Config, constants::CONCURRENCY_LIMIT, watch_list::WatchList};

use ethers::{types::Bytes, providers::Middleware};


pub async fn execute_flash_liquidation<M: Middleware + 'static>(
    candidates: Vec<Bytes>,
    is_aave: bool,
    flash_liq: &FlashLiquidator<M>,
) -> anyhow::Result<()> {
    let sem = Arc::new(Semaphore::new(CONCURRENCY_LIMIT));
    let mut handles = vec![];

    for calldata in candidates {
        let permit = sem.clone().acquire_owned().await?;
        let liquidator = flash_liq.clone();
        let calldata_clone = calldata.clone();

        let handle = tokio::spawn(async move {
            let _permit = permit; // keep permit until task finishes

            for attempt in 1..=2 {
                match liquidator
                    .execute_flash_liquidation(calldata_clone.clone(), is_aave)
                    .send()
                    .await
                {
                    Ok(pending_tx) => {
                        log::info!(
                            "✅ Successfully sent liquidation tx (attempt {}): {:?}",
                            attempt,
                            pending_tx.tx_hash()
                        );
                        break;
                    }
                    Err(e) => {
                        log::error!(
                            "❌ Liquidation failed (attempt {}): {:?}",
                            attempt,
                            e
                        );
                        if attempt == 2 {
                            log::error!("🚫 Giving up on this candidate");
                        } else {
                            tokio::time::sleep(tokio::time::Duration::from_secs(1)).await;
                        }
                    }
                }
            }
        });

        handles.push(handle);
    }

    // Wait for all tasks to finish
    for handle in handles {
        let _ = handle.await;
    }

    Ok(())
}