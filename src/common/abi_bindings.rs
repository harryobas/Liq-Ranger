use ethers::prelude::abigen;

abigen!(
    IERC20,
    r#"[
        function balanceOf(address account) external view returns (uint256)
        function decimals() external view returns (uint8)
        function symbol() external view returns (string)
    ]"#
);

abigen!(
    IFlashLiquidator,
    "src/abis/liquidator/flash_liquidator.json",
    event_derives(serde::Deserialize, serde::Serialize)
);

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

abigen!(
    IMorphoBlue,
    "src/abis/morpho/morpho_blue.json",
    event_derives(serde::Deserialize, serde::Serialize)
);

abigen!(
    IOracle,
    r#"[
        function price() external view returns (uint256)
    ]"#
);
