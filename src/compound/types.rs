use ethers::types::{Address, Bytes, U256};


pub struct BuyCollateralParams{
    pub collateral_asset: Address,
    pub base_asset: Address,
    pub base_amount: U256,
    pub min_collateral: U256,
    pub swap_target: Address,
    pub swap_proxy: Address,
    pub swap_data: Bytes,
    pub min_base_out: U256

}