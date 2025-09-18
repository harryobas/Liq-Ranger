
pub const BORROWERS_QUERY_AAVE: &str = include_str!("../borrowers.gql");
pub const BORROWERS_QUERY_MORPHO: &str = include_str!("../collaterals.gql");
pub const SLIPPAGE_BPS: u64 = 30;
pub const CONCURRENCY_LIMIT: usize = 5;
pub const CHAIN_ID: u64 = 137;

pub const AAVE_RESERVES: [&str; 3] = [
    "0x3c499c542cEF5E3811e1192ce70d8cC03d5c3359", 
    "0xc2132D05D31c914a87C6611C10748AEb04B58e8F", 
    "0x8f3Cf7ad23Cd3CaDbD9735AFf958023239c6A063"
    ];
    
pub const MORPHO_MARKETS: [&str; 9] = [
    "0x1cfe584af3db05c7f39d60e458a87a8b2f6b5d8c6125631984ec489f1d13553b", 
    "0x2476bb905e3d94acd7b402b3d70d411eeb6ace82afd3007da69a0d5904dfc998", 
    "0xd1485762dd5256b99530b6b07ab9d20c8d31b605dd5f27ad0c6dec2a18179ac6", 
    "0xa8c2e5b31d1f3fb6c000bd49355d091f71e7c866fcb74a1cb2562ef67157bc2a", 
    "0x1947267c49c3629c5ed59c88c411e8cf28c4d2afdb5da046dc8e3846a4761794",
    "0x7506b33817b57f686e37b87b5d4c5c93fdef4cffd21bbf9291f18b2f29ab0550",
    "0x267f344f5af0d85e95f253a2f250985a9fb9fca34a3342299e20c83b6906fc80",
    "0xa5b7ae7654d5041c28cb621ee93397394c7aee6c6e16c7e0fd030128d87ee1a3",
    "0x41e537c46cc0e2f82aa69107cd72573f585602d8c33c9b440e08eaba5e8fded1"
    ];