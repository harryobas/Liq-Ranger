use ethers::types::{Address, Bytes, H256, U256};
use std::str::FromStr;

use crate::adapters::helpers::morpho_math_helpers::to_assets_up;
pub use crate::common::abi_bindings::LiquidationParams;
use crate::constants::{ORACLE_PRICE_SCALE, WAD};

pub trait HealthCheck {
    fn is_healthy(&self, market: &Market, lltv: &U256, price: &U256) -> bool;
}

#[derive(Debug, Clone)]
pub struct BorrowerProfile {
    pub address: Address,
    pub repaid_shares: Option<U256>,
    pub seized_assets: Option<U256>,
    pub market_id: Option<H256>,
    pub debt_asset: Address,
    pub debt_to_cover: U256,
    pub collateral_asset: Address,
    pub seize_amount: U256,
    pub src_decimals: u8,
    pub dest_decimals: u8,
    pub protocol: Protocol,
}

#[derive(Debug, Clone)]
pub struct MarketQuote {
    pub swap_target: Address,
    pub token_transfer_proxy: Address,
    pub swap_data: Bytes,
    pub min_amt_out: U256,
}

#[derive(Debug, Clone)]
pub struct LiquidationJob {
    pub borrower: Address,
    pub debt_asset: Address,
    pub collateral_asset: Address,
    pub debt_to_cover: U256,
    pub swap_target: Address,
    pub swap_proxy: Address,
    pub swap_data: Bytes,
    pub repaid_shares: Option<U256>,
    pub seized_assets: Option<U256>,
    pub market_id: Option<H256>,
    pub protocol: Protocol,
    pub min_amt_out: U256,
}

pub struct TxPayload {
    pub job: LiquidationJob,
    pub gas_used: u64,
}

pub struct SimulationResult {
    pub success: bool,
    pub gas_used: u64,
    pub revert_reason: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum TrackerIdentity {
    AaveV3 { borrower: Address, reserve: Address },
    MorphoBlue { market_id: H256, borrower: Address },
    Compound3 { borrower: Address },
}

impl TrackerIdentity {
    pub fn to_string_id(&self) -> String {
        // Fix: Use lower-case explicit hex serialization {:#x} to decouple from debug formatting updates
        match self {
            Self::AaveV3 { borrower, reserve } => {
                format!("aave:{:#x}:{:#x}", borrower, reserve)
            }
            Self::MorphoBlue {
                market_id,
                borrower,
            } => {
                format!("morpho:{:#x}:{:#x}", market_id, borrower)
            }
            Self::Compound3 { borrower } => {
                format!("comet:{:#x}", borrower)
            }
        }
    }
}

impl FromStr for TrackerIdentity {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let parts: Vec<&str> = s.split(':').collect();
        if parts.is_empty() {
            return Err(anyhow::anyhow!("Empty identity string"));
        }

        match parts[0] {
            "aave" => {
                if parts.len() != 3 {
                    return Err(anyhow::anyhow!("Invalid Aave string layout"));
                }
                Ok(Self::AaveV3 {
                    borrower: Address::from_str(parts[1])?,
                    reserve: Address::from_str(parts[2])?,
                })
            }
            "morpho" => {
                if parts.len() != 3 {
                    return Err(anyhow::anyhow!("Invalid Morpho string layout"));
                }
                Ok(Self::MorphoBlue {
                    market_id: H256::from_str(parts[1])?,
                    borrower: Address::from_str(parts[2])?,
                })
            }
            "comet" => {
                if parts.len() != 2 {
                    return Err(anyhow::anyhow!("Invalid Compound string layout"));
                }
                Ok(Self::Compound3 {
                    borrower: Address::from_str(parts[1])?,
                })
            }
            _ => Err(anyhow::anyhow!(
                "Unknown protocol prefix signature: {}",
                parts[0]
            )),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub enum Protocol {
    Morpho,
    Aave,
}

pub struct Market {
    pub total_borrow_assets: u128,
    pub total_borrow_shares: u128,
}

pub struct Position {
    pub borrow_shares: u128,
    pub collateral: u128,
}

impl HealthCheck for Position {
    fn is_healthy(&self, market: &Market, lltv: &U256, price: &U256) -> bool {
        let collateral_value = U256::from(self.collateral) * *price / *ORACLE_PRICE_SCALE;
        let max_borrow = collateral_value * *lltv / *WAD;

        let borrowed_assets = to_assets_up(
            U256::from(self.borrow_shares),
            U256::from(market.total_borrow_assets),
            U256::from(market.total_borrow_shares),
        );

        max_borrow >= borrowed_assets
    }
}

#[derive(Debug)]
pub enum LiquidationMode {
    RepayShares {
        repaid_shares: U256,
        expected_seized_assets: U256,
    },
    SeizeCollateral {
        seized_assets: U256,
    },
}

impl From<LiquidationJob> for LiquidationParams {
    fn from(value: LiquidationJob) -> Self {
        let market_id = {
            if let Some(id) = value.market_id {
                id.to_fixed_bytes()
            } else {
                H256::zero().to_fixed_bytes()
            }
        };

        let repaid_shares = value.repaid_shares.unwrap_or_else(U256::zero);
        let seized_assets = value.seized_assets.unwrap_or_else(U256::zero);

        match value.protocol {
            Protocol::Aave => Self {
                mode: 0,
                borrower: value.borrower,
                aave_debt_asset: value.debt_asset,
                aave_collateral: value.collateral_asset,
                aave_debt_to_cover: value.debt_to_cover,
                morpho_market_id: market_id,
                morpho_repaid_shares: repaid_shares,
                morpho_seized_assets: seized_assets,
                compound_collateral: Address::zero(),
                compound_debt_asset: Address::zero(),
                compound_debt_to_cover: U256::zero(),
                compound_min_collateral: U256::zero(),
                swap_target: value.swap_target,
                swap_allowance_target: value.swap_proxy,
                swap_data: value.swap_data,
                flash_asset: value.debt_asset,
                min_amt_out: value.min_amt_out,
            },
            Protocol::Morpho => Self {
                mode: 1,
                borrower: value.borrower,
                aave_debt_asset: Address::zero(),
                aave_collateral: Address::zero(),
                aave_debt_to_cover: U256::zero(),
                morpho_market_id: market_id,
                morpho_repaid_shares: repaid_shares,
                morpho_seized_assets: seized_assets,
                compound_collateral: Address::zero(),
                compound_debt_asset: Address::zero(),
                compound_debt_to_cover: U256::zero(),
                compound_min_collateral: U256::zero(),
                swap_target: value.swap_target,
                swap_allowance_target: value.swap_proxy,
                swap_data: value.swap_data,
                flash_asset: value.debt_asset,
                min_amt_out: value.min_amt_out,
            },
        }
    }
}
