pub mod abi_bindings;
pub mod liq_data;
pub mod paraswap;
pub mod task_manager;
pub mod simulation_sandbox;


use ethers::{
    providers::Middleware,
    signers::Signer,
    types::{Address, Bytes, H256 as TxHash, U256}
};

use std::sync::Arc;

use crate::{
    aave::{aave_watchlist::AaveWatchList, abi_bindings::{IAaveV3Pool, AaveOracle, UiPoolDataProvider}},
    bootstrap_engine::bootstrap_state::BootstrapState,
    common::{abi_bindings::{IERC20, IFlashLiquidator, LiquidationParams}, simulation_sandbox::{AnvilSandbox, SimResult}},
    compound::{abi_bindings::IComet, compound_watchlist::CompoundWatchList},
    constants::{self, TOKEN_DECIMAL_CACHE, TOKEN_SYMBOL_CACHE},
    morpho::{abi_bindings::IMorphoBlue, morpho_watchlist::MorphoWatchList},
};

use sled::Db;

#[async_trait::async_trait]
pub trait Liquidator: Send + Sync {
    async fn run(&self, block_number: u64) -> anyhow::Result<()>;
}

#[async_trait::async_trait]
pub trait WatchList<T>: Sync + Send {
    async fn remove(&self, item: T) -> anyhow::Result<()>;
    async fn add(&self, item: T) -> anyhow::Result<()>;
}

pub trait Config: Send + Sync {
    fn load() -> anyhow::Result<Self>
    where
        Self: Sized;

    fn keeper_address(&self) -> Address;
    fn chain_id(&self) -> u64;
}

#[async_trait::async_trait]
pub trait LiquidationContract<M: Middleware + 'static>: Send + Sync {
    fn address(&self) -> Address;
    async fn execute_tx(
        &self,
        flash_amt: U256,
        liq_params: LiquidationParams,
        gas_limit: U256
    ) -> anyhow::Result<TxHash>;
    fn extract_calldata(
        &self,
        flash_amt: U256,
        liq_params: LiquidationParams,
    ) -> anyhow::Result<Bytes>;
}

/// Swap query parameters
#[derive(Debug, Clone)]
pub struct SwapQueryParams {
    pub src_token: String,
    pub dest_token: String,
    pub src_decimals: u8,
    pub dest_decimals: u8,
    pub amount: String, // in wei
    pub side: String,   // "SELL" or "BUY"
    pub chain_id: u64,
    pub user_address: String, // flash_liquidator contract
    pub receiver: String,     // typically same flash_liquidator
    pub slippage_bps: u32,
}

pub enum AdminCmd {
    Prune,
    StatusCheck,
}

pub struct CoreContracts<M> {
    pub aave: IAaveV3Pool<M>,
    pub aave_oracle: AaveOracle<M>,
    pub ui_pool_data_provider: UiPoolDataProvider<M>,
    pub morpho: IMorphoBlue<M>,
    pub comet: IComet<M>,
    pub flash_liq: IFlashLiquidator<M>,
}

pub struct WatchLists {
    pub aave_watchlist: Arc<AaveWatchList>,
    pub morpho_watchlist: Arc<MorphoWatchList>,
    pub comet_watchlist: Arc<CompoundWatchList>,
    pub bootstrap_state: Arc<BootstrapState>,
}

pub async fn execute_liq_tx<M: Middleware + 'static>(
    loan_amt: U256,
    liq_params: LiquidationParams,
    flash_liq: &dyn LiquidationContract<M>,
    gas_limit: U256
) -> anyhow::Result<TxHash> {
    flash_liq.execute_tx(loan_amt, liq_params, gas_limit).await
}

pub async fn simulate_liq_tx<M: Middleware + 'static>(
    flash_liq: &dyn LiquidationContract<M>,
    sim: &AnvilSandbox,
    loan_amt: U256,
    liq_params: LiquidationParams,
    snap_shot: U256,
) -> anyhow::Result<SimResult> {

    let target_address = flash_liq.address();
    let keeper_address = constants::WALLET.address();

    let calldata = flash_liq.extract_calldata(loan_amt, liq_params)?;

    let result = match sim.simulate_tx(keeper_address, target_address, calldata, U256::zero()).await{
        Ok(res) => res,
        Err(e) => {
            sim.revert(snap_shot)
              .await
              .map_err(|e| anyhow::anyhow!("CRITICAL: failed to revert snapshot: {:?}", e))?;

            return Err(anyhow::anyhow!("Simulation failed: {:?}", e));
        }
    };

   
    sim.revert(snap_shot)
      .await
      .map_err(|e| anyhow::anyhow!("CRITICAL: failed to revert snapshot: {:?}", e))?;

    if !result.success {
        let reason = result.revert_reason.clone().unwrap_or_else(|| "Unknown Revert".to_string());
        return Err(anyhow::anyhow!("Simulation Reverted: {}", reason));
    }

    Ok(result)

    
}

pub async fn get_token_decimals<M: Middleware + 'static>(
    token: Address,
    provider: Arc<M>,
) -> anyhow::Result<u8> {
    if let Some(dec) = TOKEN_DECIMAL_CACHE.get(&token) {
        return Ok(dec.value().clone());
    }

    let contract = IERC20::new(token, provider.clone());
    let result = contract.decimals().call().await?;

    TOKEN_DECIMAL_CACHE.insert(token, result);
    Ok(result)
}

pub async fn get_token_symbol<M: Middleware + 'static>(
    token: Address,
    provider: Arc<M>,
) -> anyhow::Result<String> {
    if let Some(dec) = TOKEN_SYMBOL_CACHE.get(&token) {
        return Ok(dec.value().clone());
    }

    let contract = IERC20::new(token, provider.clone());
    let result = contract.symbol().call().await?;

    TOKEN_SYMBOL_CACHE.insert(token, result.clone());
    Ok(result)
}

pub fn fetch_contracts<M: Middleware + 'static>(
    client: Arc<M>,
) -> anyhow::Result<CoreContracts<M>> {
    let liq_addr = *constants::FLASH_LIQUIDATOR;
    let aave_addr = *constants::AAVE_V3_POOL;
    let comet_addr = *constants::COMET_USDT;
    let morpho_addr = *constants::MORPHO_BLUE;
    let oracle_addr = *constants::AAVE_ORACLE;
    let ui_pool_data_addr = *constants::UIPOOL_DATA_PROVIDER;

    let flash_liq = IFlashLiquidator::new(liq_addr, client.clone());
    let aave = IAaveV3Pool::new(aave_addr, client.clone());
    let comet = IComet::new(comet_addr, client.clone());
    let morpho = IMorphoBlue::new(morpho_addr, client.clone());
    let aave_oracle = AaveOracle::new(oracle_addr, client.clone());
    let ui_pool_data_provider = UiPoolDataProvider::new(ui_pool_data_addr, client.clone());

    Ok(CoreContracts {
        aave,
        morpho,
        comet,
        flash_liq,
        aave_oracle,
        ui_pool_data_provider,
    })
}

pub fn fetch_watchlists(db: Arc<Db>) -> anyhow::Result<WatchLists> {
    Ok(WatchLists {
        aave_watchlist: Arc::new(AaveWatchList::new(db.clone())?),
        morpho_watchlist: Arc::new(MorphoWatchList::new(db.clone())?),
        comet_watchlist: Arc::new(CompoundWatchList::new(db.clone())?),
        bootstrap_state: Arc::new(BootstrapState::new(db)?),
    })
}

pub async fn create_simulation_sandbox<M: Middleware + 'static>(block_number: u64, f_liq: &IFlashLiquidator<M>) -> anyhow::Result<AnvilSandbox> {
    let sim_sandbox = AnvilSandbox::new(&*constants::RPC_URL_HTTP, block_number)?;
    let bytecode = constants::LIQ_BYTECODE.clone();
    let target_address = f_liq.address();
    let keeper_address = constants::WALLET.address();

    sim_sandbox.set_code(target_address, bytecode).await?;
    sim_sandbox.impersonate(keeper_address).await?;
    sim_sandbox.set_balance(keeper_address, U256::exp10(18) * 50).await?;

    Ok(sim_sandbox)
}

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct LiquidationRecord {
    pub timestamp: i64,
    pub block_number: i64,
    pub protocol: String,
    pub borrower: Address,
    pub collateral_asset: Address,
    pub profit_asset: Address,
    pub profit_amount: f64,
    pub profit_symbol: String,
    pub collateral_symbol: String,
    pub tx_hash: String,
}

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct DistributionRecord {
    pub tx_hash: String,
    pub asset: String,
    pub asset_symbol: String,
    pub amount: f64,
    pub owner_share: f64,
    pub breet_share: f64,
    pub timestamp: i64,
}

impl LiquidationRecord {

    pub async fn save(&self, pool: &sqlx::SqlitePool) -> anyhow::Result<()> {
        sqlx::query(
            r#"
            INSERT OR IGNORE INTO liquidations (
                tx_hash, protocol, borrower, profit_asset, profit_symbol, 
                collateral_asset, collateral_symbol, profit_amount, block_number, timestamp
            ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
            "#
        )
        .bind(&self.tx_hash)
        .bind(&self.protocol)
        .bind( self.borrower.to_string())
        .bind(self.profit_asset.to_string())
        .bind(&self.profit_symbol)
        .bind(self.collateral_asset.to_string())
        .bind(&self.collateral_symbol)
        .bind(self.profit_amount)
        .bind(self.block_number)
        .bind(self.timestamp)
        .execute(pool)
        .await?;

        Ok(())
    }

    
}

impl DistributionRecord {
    
    pub async fn save(&self, pool: &sqlx::SqlitePool) -> anyhow::Result<()> {
        sqlx::query(
            r#"
            INSERT OR IGNORE INTO distributions (
                tx_hash, asset, asset_symbol, amount, owner_share, breet_share, timestamp
            ) VALUES (?, ?, ?, ?, ?, ?, ?)
            "#
        )
        .bind(&self.tx_hash)
        .bind(&self.asset)
        .bind(&self.asset_symbol)
        .bind(self.amount)
        .bind(self.owner_share)
        .bind(self.breet_share)
        .bind(self.timestamp)
        .execute(pool)
        .await?;

        Ok(())
    }
}