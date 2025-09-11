pub mod aave_config;

use std::{env as std_env, process};
use once_cell::sync::Lazy;

pub static SUBGRAPH_URL: Lazy<String> = Lazy::new(|| get_env_var(env::SUBGRAPH_URL_ENV_VAR));
pub static SUBGRAPH_API_KEY: Lazy<String> = Lazy::new(|| get_env_var(env::SUBGRAPH_API_KEY_ENV_VAR));
pub static PRIVATE_KEY: Lazy<String> = Lazy::new(|| get_env_var(env::PRIVATE_KEY_VAR));
pub static LENDING_POOL: Lazy<String> = Lazy::new(|| get_env_var(env::LENDING_POOL_VAR));
pub static FLASH_LIQUIDATOR: Lazy<String> = Lazy::new(|| get_env_var(env::FLASH_LIQUIDATOR_VAR));
pub static RPC_URL: Lazy<String> = Lazy::new(|| get_env_var(env::RPC_URL_VAR));
pub static AAVE_ORACLE: Lazy<String> = Lazy::new(|| get_env_var(env::AAVE_ORACLE_VAR));
pub static DEX_ROUTER: Lazy<String> = Lazy::new(|| get_env_var(env::DEX_ROUTER_VAR));
pub static UIPOOL_DATA: Lazy<String> = Lazy::new(|| get_env_var(env::UIPOOL_DATA_VAR));
pub static POOL_ADDRESS_PROVIDER: Lazy<String> = Lazy::new(|| get_env_var(env::POOL_ADDRESS_PROVIDER_VAR));


//pub const BORROWERS_QUERY: &str = include_str!("../borrowers.gql");


fn get_env_var(var_name: &str) -> String {
     match std_env::var(var_name){
        Ok(val) if !val.is_empty() => val,
        Ok(_) => {
            log::error!("Enviroment variable {var_name} is empty");
            process::exit(1)
        },
        Err(_) => {
            log::error!("Environment variable {var_name} is not set");
            process::exit(1)
        },
     }
}

mod env {
    pub const SUBGRAPH_URL_ENV_VAR: &str = "SUBGRAPH_URL";
    pub const SUBGRAPH_API_KEY_ENV_VAR: &str = "SUBGRAPH_API_KEY";
    pub const PRIVATE_KEY_VAR: &str = "PRIVATE_KEY";
    pub const LENDING_POOL_VAR: &str = "LENDING_POOL";
    pub const FLASH_LIQUIDATOR_VAR: &str = "FLASH_LIQUIDATOR";
    pub const RPC_URL_VAR: &str = "RPC_URL";
    pub const AAVE_ORACLE_VAR: &str = "AAVE_ORACLE";
    pub const DEX_ROUTER_VAR: &str = "DEX_ROUTER";
    pub const UIPOOL_DATA_VAR: &str = "UIPOOL_DATA";
    pub const POOL_ADDRESS_PROVIDER_VAR: &str = "POOL_ADDRESS_PROVIDER";

}