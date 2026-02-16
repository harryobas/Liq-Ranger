
use super::{abi_bindings::{
    AaveOracle, 
    IAaveV3Pool,  
    UiPoolDataProvider
}, helpers::{
    self,
    select_best_collateral, 
    compute_debt
}, types::LiquidationCandidate, 
aave_config::AaveConfig, 
aave_watchlist::AaveWatchList};

use anyhow::ensure;
use ethers::{
    providers::Middleware, types::{Address, U256}
};

use std::sync::Arc;

use crate::common::{
    Liquidator,
    SwapQueryParams, 
    get_token_decimals, simulate_liq_tx, execute_liq_tx, 
    paraswap::ParaSwapClient,
    abi_bindings::IFlashLiquidator,
    liq_data::LiqData
};
use futures_util::{self, StreamExt, stream}; 

pub struct AaveLiquidator<M: Middleware + 'static> {
    pub lending_pool: IAaveV3Pool<M>,
    pub flash_liquidator: IFlashLiquidator<M>,
    pub aave_oracle: AaveOracle<M>,
    pub user_data: UiPoolDataProvider<M>,
    pub client: Arc<M>,
    pub watch_list: Arc<AaveWatchList>,
    pub config: Arc<AaveConfig>
}

impl<M: Middleware> AaveLiquidator<M> {
    // ... [new() remains similar, just remove self.dex] ...
     pub fn new(
        config: Arc<AaveConfig>, 
        client: Arc<M>, 
        watch_list: Arc<AaveWatchList>
    ) -> Self {
        
        let lending_pool = IAaveV3Pool::new(config.lending_pool, client.clone());

        let flash_liquidator = IFlashLiquidator::new(
            config.flash_liquidator,  
            client.clone()
        );

        let aave_oracle = AaveOracle::new(
            config.aave_oracle,
            client.clone()
        );
        
        let user_data = UiPoolDataProvider::new(
            config.ui_pool_data, 
            client.clone()
        );

        Self { 
            lending_pool, 
            flash_liquidator, 
            aave_oracle, 
            user_data,
            client,
            watch_list,
            config

         }
    }
    async fn generate_liquidations(&self) -> anyhow::Result<Vec<LiquidationCandidate>> {
        let snapshot = self.watch_list.snapshot();
        if snapshot.is_empty() {
            return Ok(vec![]);
        }
        let mut out = vec![];

        for (borrower, reserve) in snapshot {
            if let Ok(Some(candidate)) = self.analyze_borrower(borrower, reserve).await {
                out.push(candidate);
            }
        }

        Ok(out)
    }

    async fn analyze_borrower(
        &self,
        borrower: Address, 
        reserve: Address
    ) -> anyhow::Result<Option<LiquidationCandidate>>{
            
        // 1. Health factor check
        let (_, _, _, _, _, hf) = self.lending_pool.get_user_account_data(borrower).call().await?;

        if hf >= U256::exp10(18) {
            return Ok(None);
        }

         let v_debt = *self.config
            .vdebt_tokens
            .get(&reserve)
            .ok_or_else(|| anyhow::anyhow!("Missing vDebt"))?;

         let debt_to_cover = compute_debt(
            v_debt,
            borrower,
            hf,
            self.client.clone()
        ).await?;

        // 3. Pick collateral
        let collateral = select_best_collateral(
            borrower,
            &self.lending_pool,
            &self.user_data,
            &self.aave_oracle,
            reserve,
            debt_to_cover,
            self.client.clone(),
            &self.config
        ).await?;

         // 4. ParaSwap routing
        let (src_decimals, dest_decimals) = tokio::try_join!(
            get_token_decimals(collateral.asset, self.client.clone()),
            get_token_decimals(reserve, self.client.clone())
        )?;

        let swap_params = SwapQueryParams {
            src_token: collateral.asset.to_string(),
            dest_token: reserve.to_string(),
            src_decimals,
            dest_decimals,
            amount: collateral.seize_amount.to_string(),
            side: String::from("SELL"),
            chain_id: self.config.chain_id,
            slippage_bps: 30, // 0.3%
            user_address: self.flash_liquidator.address().to_string(),
            receiver: self.flash_liquidator.address().to_string()
        };

        let paraswap_client = ParaSwapClient::new();
        let route = paraswap_client.compose_swap_data(swap_params).await?;

        ensure!(
            collateral.seize_amount >= route.src_amount, 
            "swap src exceeds seized collateral"
        );

        ensure!(
            route.dest_amount >= debt_to_cover,
            "swap output insufficient to repay debt"
        );

        Ok(Some(LiquidationCandidate { 
            debt_to_cover, 
            debt_asset: reserve, 
            collateral_asset: collateral.asset, 
            borrower, 
            swap_target: route.swap_target, 
            swap_proxy: route.token_transfer_proxy, 
            swap_data: route.swap_data
        }))
        
    }
}

   
#[async_trait::async_trait]
impl<M> Liquidator for AaveLiquidator<M>
where
    M: Middleware + 'static,
    {
    
        async fn run(&self) -> anyhow::Result<()> {
             let candidates = self.generate_liquidations().await?;
        if candidates.is_empty() {
            return Ok(());
        }

        let jobs = candidates
            .into_iter()
            .map(|c| {
                let debt = c.debt_to_cover;
                let debt_asset = c.debt_asset;
                let data = LiqData::from(c);
                (debt, debt_asset, data)
            })
            .collect::<Vec<_>>()
            .into_iter()
            .map(|(debt, debt_asset, data)| {
                (debt, debt_asset, helpers::encode_liq_data(&data))
            })
            .collect::<Vec<_>>();
         stream::iter(jobs)
            .for_each_concurrent(2, |(loan_amt, debt_asset, data)| async move {
                if simulate_liq_tx(
                    &self.flash_liquidator,
                    self.config.clone(),
                    self.client.clone(),
                    loan_amt,
                    debt_asset,
                    data.clone(),
                )
                .await
                .is_ok()
                {
                    if let Err(e) = execute_liq_tx(
                        loan_amt,
                        debt_asset,
                        data.clone(),
                        &self.flash_liquidator,
                    )
                    .await
                    {
                        tracing::error!("liquidation failed: {:?}", e);
                    }
                }
            })
            .await;

        Ok(())

        }
    }