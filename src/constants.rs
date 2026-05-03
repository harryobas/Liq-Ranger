use ethers::{
     signers::{LocalWallet, Signer}, 
     types::{Address, Bytes, H256, U256}, 
     utils::parse_ether
};
use once_cell::sync::Lazy;
use tokio::{
    sync::Mutex,
    task::JoinHandle
};
use std::{collections::HashSet, str::FromStr};
use secrecy::{SecretString, ExposeSecret};

use std::env;
use dashmap::DashMap;


// Shared
pub const CHAIN_ID: u64 = 137;
pub const SLED_PATH: &str = "./data/sled_db";
pub const LIQ_EXECUTOR_INTERVAL: u64 = 10;
pub const PRUNE_INTERVAL: u64 = 50;

pub static DATABASE_URL: Lazy<String> = Lazy::new(|| {
    env::var("DATABASE_URL").unwrap_or_else(|_| "sqlite://./data/history.db".to_string())
});

pub const AAVE_DEPLOY_BLOCK: u64 = 75_000_000;
//pub const COMPOUND_DEPLOY_BLOCK: u64 = 42_000_000;
pub const MORPHO_DEPLOY_BLOCK: u64 = 68_000_000;

pub static FLASH_LIQUIDATOR: Lazy<Address> = Lazy::new(|| {
    Address::from_str("0x089C0634bb99593174D8273f997c9dbC5D9A4991").expect("Failed")
});


pub static LIQ_BYTECODE: Lazy<Bytes> = Lazy::new(|| {
    let bytecode_str = include_str!("./abis/liquidator/flash_liquidator.bin");
    Bytes::from_str(bytecode_str).unwrap_or(Bytes::new())
});

pub static USDC: Lazy<Address> = Lazy::new(||
    Address::from_str("0x3c499c542cEF5E3811e1192ce70d8cC03d5c3359").expect("Failed")
);
pub static USDT: Lazy<Address> = Lazy::new(||
    Address::from_str("0xc2132D05D31c914a87C6611C10748AEb04B58e8F").expect("Failed")
);
pub static WPOL: Lazy<Address> = Lazy::new(||
    Address::from_str("0x0d500B1d8E8eF31E21C99d1Db9A6444d3ADf1270").expect("Failed")
);

pub static BREET: Lazy<Address> = Lazy::new(||
    Address::from_str("0x46082c9F4ca0eF92c510984B612183211c0a27dE").expect("Failed")

);

pub static GAS_THRESHOLD: Lazy<U256> = Lazy::new(|| parse_ether(10u64).expect("Failed"));
pub static REFUEL_AMT: Lazy<U256> = Lazy::new(|| parse_ether(100u64).expect("Failed"));

pub static PROFIT_DIST_ASSETS: Lazy<Vec<Address>> = Lazy::new(|| {
    let mut profit_assets: Vec<Address> = AAVE_RESERVES.iter().cloned().collect();
    if !profit_assets.contains(&WPOL) {
        profit_assets.push(*WPOL);
    }

    profit_assets


});

pub static TOKEN_DECIMAL_CACHE: Lazy<DashMap<Address, u8>> = Lazy::new(|| DashMap::new() );
pub static TOKEN_SYMBOL_CACHE: Lazy<DashMap<Address, String>> = Lazy::new(|| DashMap::new() );

pub static PRIVATE_KEY: Lazy<SecretString> = Lazy::new(|| {
    SecretString::new(load_private_key().into())
});

pub static RPC_URL: Lazy<String> = Lazy::new(|| {
    load_rpc_url()
});

pub static RPC_URL_HTTP: Lazy<String> = Lazy::new(|| {
    match env::var("RPC_URL_HTTP") {
        Ok(url) => url,
        Err(_) => panic!("No RPC_URL_HTTP found. Please set RPC_URL_HTTP env var.")
    }
});

pub static WALLET: Lazy<LocalWallet> = Lazy::new(|| {
        PRIVATE_KEY
        .expose_secret()
        .parse::<LocalWallet>()
        .expect("Invalid private key")
        .with_chain_id(CHAIN_ID)
});

pub static GLOBAL_TASK_HANDLES: Lazy<Mutex<Vec<JoinHandle<()>>>> = Lazy::new(|| {Mutex::new(Vec::new())});

// Morpho
pub static  MORPHO_BLUE: Lazy<Address> = Lazy::new(|| {
    Address::from_str("0x1bF0c2541F820E775182832f06c0B7Fc27A25f67").expect("Failed")
});
pub const VIRTUAL_ASSETS: u128 = 1;
pub const VIRTUAL_SHARES: u128 = 1_000_000;

pub static  WAD: Lazy<U256> = Lazy::new(|| {
    pow10(18)
});

pub static  MAX_LIQUIDATION_INCENTIVE_FACTOR: Lazy<U256> = Lazy::new(||{
    max_liquidation_incentive_factor()
});

pub static  ORACLE_PRICE_SCALE: Lazy<U256> = Lazy::new(||{
    oracle_price_scale()
});

pub static  LIQUIDATION_CURSOR: Lazy<U256> = Lazy::new(||{
    liquidation_cursor()
});

pub static MORPHO_MARKETS: Lazy<HashSet<H256>> = Lazy::new(|| {
    [
        "0x1cfe584af3db05c7f39d60e458a87a8b2f6b5d8c6125631984ec489f1d13553b",
        "0x2476bb905e3d94acd7b402b3d70d411eeb6ace82afd3007da69a0d5904dfc998",
        "0x1947267c49c3629c5ed59c88c411e8cf28c4d2afdb5da046dc8e3846a4761794",
        "0x7506b33817b57f686e37b87b5d4c5c93fdef4cffd21bbf9291f18b2f29ab0550",
        "0x267f344f5af0d85e95f253a2f250985a9fb9fca34a3342299e20c83b6906fc80",
        "0xa5b7ae7654d5041c28cb621ee93397394c7aee6c6e16c7e0fd030128d87ee1a3",
        "0x41e537c46cc0e2f82aa69107cd72573f585602d8c33c9b440e08eaba5e8fded1",
        "0x96e62bd75493006b81dae51d5db3c5af4b3ced65133dab60e70df9dc8e38bf2c",
        "0xb8ae474af3b91c8143303723618b31683b52e9c86566aa54c06f0bc27906bcae",
        "0x28d8d92f5392c1b26e82dcbec25949ed028ea5b99d5a929ce485f0fd88e47fcc",
        "0x01550b8779f4ca978fc16591537f3852c02c3491f597db93d9bb299dcbf5ddbe",
        "0xa932e0d8a9bf52d45b8feac2584c7738c12cf63ba6dff0e8f199e289fb5ca9bb"
    ]
    .into_iter()
    .map(|s| H256::from_str(s).expect("invalid Morpho market id"))
    .collect()
});

    // Aave
pub const HF_LIQUIDATION_THRESHOLD_BPS: u128 = 9_500; // 0.95
pub static UIPOOL_DATA_PROVIDER: Lazy<Address> = Lazy::new(||{
    Address::from_str("0xFa1A7c4a8A63C9CAb150529c26f182cBB5500944").expect("Failed")
});
pub static  AAVE_V3_POOL: Lazy<Address> = Lazy::new(||{
    Address::from_str("0x794a61358D6845594F94dc1DB02A252b5b4814aD").expect("Failed")
});
pub static  AAVE_ORACLE: Lazy<Address> = Lazy::new(||{
    Address::from_str("0xb023e699F5a33916Ea823A16485e259257cA8Bd1").expect("Failed")
});
pub static  POOL_ADDRESS_PROVIDER: Lazy<Address> = Lazy::new(||{
    Address::from_str("0xa97684ead0e402dC232d5A977953DF7ECBaB3CDb").expect("Failed")
});

pub static AAVE_RESERVES: Lazy<HashSet<Address>> = Lazy::new(|| {
    [
        "0x3c499c542cEF5E3811e1192ce70d8cC03d5c3359", // USDC
        "0xc2132D05D31c914a87C6611C10748AEb04B58e8F", // USDT
        "0x7ceB23fD6bC0adD59E62ac25578270cFf1b9f619", // WETH
        "0x8f3Cf7ad23Cd3CaDbD9735AFf958023239c6A063",  // DAI
        "0x1BFD67037B42Cf73acF2047067bd4F2C47D9BfD6"  // WBTC
    ]
    .into_iter()
    .map(|s| s.parse::<Address>().expect("invalid reserve address"))
    .collect()
});

pub static ATOKENS_ADDR: Lazy<DashMap<Address, Address>> = Lazy::new(|| {
    DashMap::new()

});

//compound

pub static COMET_USDT: Lazy<Address> = Lazy::new(||
    Address::from_str("0xaeB318360f27748Acb200CE616E389A6C9409a07").expect("Failed")
);

pub static COMPOUND_RESERVES: Lazy<HashSet<Address>> = Lazy::new(|| {
    [
        "0xfa68FB4628DFF1028CFEc22b4162FCcd0d45efb6",
        "0x3A58a54C066FdC0f2D55FC9C89F0415C92eBf3C4",
        "0x1BFD67037B42Cf73acF2047067bd4F2C47D9BfD6",
        "0x7ceB23fD6bC0adD59E62ac25578270cFf1b9f619",
        "0x0d500B1d8E8eF31E21C99d1Db9A6444d3ADf1270"
    ]
    .into_iter()
    .map(|s| s.parse::<Address>().expect("invalid reserve address"))
    .collect()
});

//helpers

fn max_liquidation_incentive_factor() -> U256 {
    U256::from(115) * pow10(16)
}

/// Oracle price scale (1e36)
pub fn oracle_price_scale() -> U256 {
    pow10(36)
}

/// 30% liquidation cursor (0.3e18)
pub fn liquidation_cursor() -> U256 {
    U256::from(3) * pow10(17)
}

fn pow10(exp: u32) -> U256 {
    U256::from(10).pow(U256::from(exp))
}

fn load_private_key() -> String {

    match env::var("PRIVATE_KEY") {
        Ok(key) => {
            key
        }
        Err(_) => panic!(
            "❌ No private key found. Please set PRIVATE_KEY env var or provide /run/secrets/private_key."
        ),
    }
}

 fn load_rpc_url() -> String {
    match env::var("RPC_URL") {
        Ok(key) => key,
        Err(_) => panic!(
            "No RPC URL found. Please set RPC_URL env var."
        )
    }
}


