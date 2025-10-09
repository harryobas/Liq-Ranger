use super::Liquidator;

use anyhow:: Result;
use ethers::providers::Middleware;
use ethers::types::{Address, Bytes, U256};
use ethers::prelude::{SignerMiddleware, Provider, Ws, LocalWallet};
use log;

use crate::models::liquidation::LiquidationCandidate;

use crate::{
    watch_list::{aave_watch_list::AaveWatchList, WatchList},
    config::aave_config::AaveConfig
};

use std::sync::Arc;
use crate::{abi_bindings::{
    AaveOracle, 
    AaveV3Pool, 
    Dex, 
    FlashLiquidator,
    UiPoolDataProvider
}, helpers::aave_helpers::*};

use crate::constants;

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


async fn generate_liquidations(&self) -> Result<Vec<Bytes>> {
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
        
        let debt_token_addr: Address =  *self.config
            .vdebt_tokens.get(&asset)
            .ok_or_else(|| anyhow::anyhow!("Missing vDebt token for asset {:?}", asset))?;
    
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
            U256::from(constants::SLIPPAGE_BPS)
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

    let candidates = candidates
        .into_iter()
        .map(|c|{ 
            match create_aave_liquidation_calldata(&c){
                Ok(calldata) => calldata,
                Err(_) => Bytes::from([]),
            }
        })
        .filter(|calldata| !calldata.is_empty())
        .collect::<Vec<Bytes>>();

    Ok(candidates)
}


}

#[async_trait::async_trait]
impl Liquidator for AaveLiquidator<SignerMiddleware<Provider<Ws>, LocalWallet>>{

    async fn run(&self) -> Result<()> {
        let liquidations = self.generate_liquidations().await?;

        if liquidations.is_empty() {
            log::info!("No liquidation candidates found");
            return Ok(());
        }

        super::execute_flash_liquidation(liquidations, true, &self.flash_liquidator).await

    }
}

