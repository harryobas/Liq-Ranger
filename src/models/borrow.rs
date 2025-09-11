use serde::{Serialize, Deserialize};
use super::{Asset, Account};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Borrow{
    pub asset: Asset,
    pub account: Account
}

#[derive( Debug, Clone, Deserialize)]
pub struct BorrowsData{
    pub borrows: Vec<Borrow>
}