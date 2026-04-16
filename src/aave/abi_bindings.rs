use ethers::prelude::abigen;

abigen!(
    IAaveV3Pool,
    "src/abis/aave/aave.json",
    event_derives(serde::Deserialize, serde::Serialize)
);


abigen!(
    UiPoolDataProvider,
    "src/abis/aave/pool_data.json",
    event_derives(serde::Deserialize, serde::Serialize)
);

abigen!(
    AaveOracle,
    r#"[
        function getAssetPrice(address asset) external view returns (uint256)
    ]"#
);
