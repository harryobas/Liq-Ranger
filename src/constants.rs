use lazy_static::lazy_static;
use std::env as std_env;

lazy_static!{
    pub static ref SUBGRAPH_URL: String = get_env_var(env::SUBGRAPH_URL_ENV_VAR);
    pub static ref SUBGRAPH_API_KEY: String = get_env_var(env::SUBGRAPH_API_KEY_ENV_VAR);
    pub static ref PRIVATE_KEY: String = get_env_var(env::PRIVATE_KEY_VAR);
    pub static ref LENDING_POOL: String = get_env_var(env::LENDING_POOL_VAR);
    pub static ref FLASH_LIQUIDATOR: String = get_env_var(env::FLASH_LIQUIDATOR_VAR);
    pub static ref RPC_URL: String = get_env_var(env::RPC_URL_VAR);
    pub static ref AAVE_ORACLE: String = get_env_var(env::AAVE_ORACLE_VAR);
    pub static ref DEX_ROUTER: String = get_env_var(env::DEX_ROUTER_VAR);
    pub static ref UIPOOL_DATA: String = get_env_var(env::UIPOOL_DATA_VAR);
    pub static ref POOL_ADDRESS_PROVIDER: String = get_env_var(env::POOL_ADDRESS_PROVIDER_VAR);

}

pub const BORROWERS_QUERY: &str = include_str!("../borrowers.gql");


fn get_env_var(var_name: &str) -> String {
     match std_env::var(var_name){
        Ok(val) if !val.is_empty() => val,
        Ok(_) => panic!("Enviroment variable {var_name} is set but empty"),
        Err(_) => panic!("Enviroment variable {var_name} is not set"),
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