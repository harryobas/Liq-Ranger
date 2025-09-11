pub mod borrow;
pub mod liquidation;


use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Asset {
    pub id: String 
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Account {
    pub id: String
}

pub enum LiquidationCommand{
    RunCycle,
    Shutdown
}



