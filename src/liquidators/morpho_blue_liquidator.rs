
use ethers::{
    middleware::SignerMiddleware, 
    providers::{
    Middleware, 
    Provider, 
    Ws
    }, signers::LocalWallet, types::{Address, Bytes, H256, U256}
};

use crate::{
    abi_bindings::{
        FlashLiquidator, 
        IMorphoBlue, 
        Market, 
        MarketParams, 
        Position}, config::morpho_config::MorphoConfig, 
        helpers::morpho_helpers::*, liquidators::Liquidator, 
        watch_list::{
            morpho_watch_list::MorphoWatchList, WatchList
        }
    };
use std::sync::Arc;

pub struct MorphoLiquidationCandidate{
    pub debt_to_cover: U256,
    pub borrower: Address,
    pub seized_assets: U256,
    pub repaid_shares: U256,
    pub market_id: H256,
}

struct HealthCheckResult {
    is_healthy: bool,
    debt_to_cover: U256,
    repaid_shares: U256,
}
pub struct MorphoBlueLiquidator<M: Middleware> {
    watch_list: Arc<MorphoWatchList>,
    client: Arc<M>,
    morpho_blue: IMorphoBlue<M>,
    flash_liquidator: FlashLiquidator<M>,
    config: Arc<MorphoConfig>
}

impl <M: Middleware> MorphoBlueLiquidator<M> {
    pub fn new(
        config: Arc<MorphoConfig>,
        client: Arc<M>,
        watch_list:Arc<MorphoWatchList>
    ) -> Self {
        let morpho_blue = IMorphoBlue::new(config.morpho_blue, client.clone());
        let flash_liquidator = FlashLiquidator::new(
            config.flash_liquidator, 
            client.clone()
        );

        Self { watch_list, client, morpho_blue, flash_liquidator, config }
    }

    async fn generate_liquidations(&self) -> anyhow::Result<Vec<Bytes>> {
        let borrows = self.watch_list.snapshot().await?;
        let mut candidates = vec![];

        for (borrower, market_id) in borrows {
            let id = market_id.to_fixed_bytes();

            let position: Position = self.morpho_blue.position(id, borrower).call().await?;
            let market: Market = self.morpho_blue.market(id).call().await?;
            
        
            let market_params: MarketParams = self.morpho_blue.id_to_market_params(id)
            .call()
            .await?;
            
            let price: U256 = IOracle::new(market_params.oracle, self.client.clone())
                .call()
                .await?
                .price(); 

            let (is_healthy, debt_to_cover, repaid_shares) = position_is_healthy(
                &position,
                &market,
                market_params.lltv,
                price,
            )?;
                
            if !is_healthy {

                let candidate = MorphoLiquidationCandidate {
                    debt_to_cover,
                    borrower,
                    seized_assets: U256::zero(),
                    repaid_shares,
                    market_id, 
                };

                candidates.push(candidate);
            }

        }

        let candidates = candidates
            .into_iter()
            .map(|c|{
                match create_morpho_liquidation_calldata(&c){
                    Ok(calldata) => calldata,
                    Err(_e) => Bytes::from([]), 
                }
            })
            .filter(|calldata| !calldata.is_empty())
            .collect::<Vec<Bytes>>();

        Ok(candidates)

    } 

}


#[async_trait::async_trait]
impl Liquidator for MorphoBlueLiquidator<SignerMiddleware<Provider<Ws>, LocalWallet>>{

    async fn run(&self) -> anyhow::Result<()>{
        let  liquidations = self.generate_liquidations().await?;

        if liquidations.is_empty() {
            log::info!("No liquidation candidates found");
            return Ok(());
        }

        super::execute_flash_liquidation(liquidations, false, &self.flash_liquidator).await?;

        Ok(())
        
    }

}