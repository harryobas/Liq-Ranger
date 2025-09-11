use ethers::types::{Address, U256};

#[derive(Debug, Clone)]
pub struct LiquidationCandidate {
    pub borrower: Address,
    pub debt_asset: Address,
    pub collateral_asset: Address,
    pub debt_to_cover: U256, 
    pub min_amount_out: U256
}
