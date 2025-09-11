use super::Liquidator;

use anyhow:: Result;
use ethers::providers::Middleware;
use ethers::types::{Address, U256};
use ethers::prelude::{SignerMiddleware, Provider, Ws, LocalWallet};
use log;


use crate::models::liquidation::LiquidationCandidate;


use crate::{
    watch_list::{aave_watch_list::AaveWatchList, WatchList},
    config::aave_config::AaveConfig
};



use std::sync::Arc;
use crate::abi_bindings::{
    aave_v3_pool, 
    AaveOracle, 
    AaveV3Pool, 
    Dex, 
    FlashLiquidator,
    UiPoolDataProvider
};

use crate::utils::*;

pub struct AaveLiquidator<M: Middleware + 'static> {
    pub lending_pool: AaveV3Pool<M>,
    pub flash_liquidator: FlashLiquidator<M>,
    pub aave_oracle: AaveOracle<M>,
    pub dex: Dex<M>,
    pub user_data: UiPoolDataProvider<M>,
    pub client: Arc<M>,
    pub watch_list: Arc<AaveWatchList>,
    pub config: Arc<AaveConfig>
}

impl<M: Middleware> AaveLiquidator<M> {
    pub fn new(
        config: Arc<AaveConfig>, 
        client: Arc<M>, 
        watch_list: Arc<AaveWatchList>
    ) -> Self {
        let client = client;

        let lending_pool = AaveV3Pool::new(config.lending_pool, client.clone());

        let flash_liquidator = FlashLiquidator::new(
            config.flash_liquidator,  
            client.clone()
        );

        let aave_oracle = AaveOracle::new(
            config.aave_oracle,
            client.clone()
        );
        let dex = Dex::new(config.dex_router, client.clone());
        let user_data = UiPoolDataProvider::new(
            config.ui_pool_data, 
            client.clone()
        );

        Self { 
            lending_pool, 
            flash_liquidator, 
            aave_oracle, 
            dex,
            user_data,
            client,
            watch_list,
            config

         }
    }


async fn generate_liquidations(&self) -> Result<Vec<LiquidationCandidate>> {
    let borrows = self.watch_list.snapshot().await?;
    let mut borrows_with_hf = vec![];

    // --- Fetch HF for each borrow ---
    for (account, asset) in borrows {
        let (_, _, _, _, _, health_factor) =
            self.lending_pool.get_user_account_data(account)
                .call()
                .await?;

        // Only liquidatable accounts
        if health_factor < U256::exp10(18) {
            borrows_with_hf.push((account, asset, health_factor));
            
        }
    }

    // --- Sort ascending by HF (lowest first) ---
    borrows_with_hf.sort_by(|a, b| a.2.cmp(&b.2));

    let mut candidates = vec![];

    for (account, asset, health_factor) in borrows_with_hf {
        let reserve_data: aave_v3_pool::ReserveData = self.lending_pool
            .get_reserve_data(asset)
            .call()
            .await?;

        let debt_token_addr: Address = reserve_data.variable_debt_token_address;
    

        let debt_to_cover = get_debt_to_cover(
            debt_token_addr,
            account,
            self.client.clone(),
            health_factor
        ).await?;

        let (collateral_asset, lb, _usd_value) = select_collateral(
            account,
            &self.lending_pool,
            &self.user_data,
            &self.aave_oracle,
            asset,
            debt_to_cover,
            self.client.clone(),
            &self.config
        ).await?;

        // --- Profitability Check ---

        let estimated_collateral_amt = get_estimated_collateral_amt(
            debt_to_cover,
            collateral_asset,
            asset,
            U256::from(lb),
            &self.aave_oracle,
            self.client.clone()
        ).await?;

        let min_amount_out = simulate_swap_on_dex(
            estimated_collateral_amt,
            asset,
            collateral_asset,
            &self.dex,
            U256::from(30)
        ).await?;

        if is_liquidation_profitable(debt_to_cover, min_amount_out) {
            candidates.push(LiquidationCandidate {
                borrower: account,
                debt_asset: asset,
                collateral_asset,
                debt_to_cover,
                min_amount_out
            });
        }
    }

    Ok(candidates)
}


}

#[async_trait::async_trait]
impl Liquidator for AaveLiquidator<SignerMiddleware<Provider<Ws>, LocalWallet>> {

    async fn run(&self) -> Result<()> {
        let liquidations = self.generate_liquidations().await?;

        if liquidations.is_empty() {
            log::info!("No liquidation candidates found");
            return Ok(());
        }

        use tokio::sync::Semaphore;
        
        // Limit concurrency to 5 simultaneous liquidations
        let concurrency_limit = 5;
        let sem = Arc::new(Semaphore::new(concurrency_limit));

        let mut handles = vec![];

        for liq in liquidations {
            let permit = sem.clone().acquire_owned().await?;
            let liquidator = self.flash_liquidator.clone();
            let liq = liq.clone(); // ensure LiquidationCandidate: Clone

            let handle = tokio::spawn(async move {
                let _permit = permit; // Hold the permit until task finishes

                // Attempt liquidation with optional retry
                for attempt in 1..=2 {
                    match liquidator
                        .execute_flash_liquidation(
                            liq.collateral_asset,
                            liq.debt_asset,
                            liq.borrower,
                            liq.debt_to_cover,
                            liq.min_amount_out,
                        )
                        .send()
                        .await
                    {
                        Ok(_) => {
                            log::info!(
                                "Successfully liquidated borrower {} (attempt {})",
                                liq.borrower,
                                attempt
                            );
                            break;
                        }
                        Err(e) => {
                            log::error!(
                                "Failed to liquidate borrower {} on attempt {}: {:?}",
                                liq.borrower,
                                attempt,
                                e
                            );
                            if attempt == 2 {
                                log::error!("Giving up on borrower {}", liq.borrower);
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
}

