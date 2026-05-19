use ethers::types::{Address, Bytes, U256};

pub struct CollateralCandidate {
    pub asset: Address,
    pub liquidation_bonus_bps: u16,
    pub seize_amount: U256,
    pub usd_value: U256,
}

#[derive(Debug, Clone)]
pub struct LiquidationCandidate {
    pub debt_to_cover: U256,
    pub debt_asset: Address,
    pub collateral_asset: Address,
    pub borrower: Address,
    pub swap_target: Address,
    pub swap_proxy: Address,
    pub swap_data: Bytes,
    pub min_amt_out: U256,
}
