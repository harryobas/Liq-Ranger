use ethers::prelude::abigen;


abigen!(
    AaveV3Pool,
    r#"[
  {
    "inputs": [
      { "internalType": "address", "name": "user", "type": "address" }
    ],
    "name": "getUserAccountData",
    "outputs": [
      { "internalType": "uint256", "name": "totalCollateralBase", "type": "uint256" },
      { "internalType": "uint256", "name": "totalDebtBase", "type": "uint256" },
      { "internalType": "uint256", "name": "availableBorrowsBase", "type": "uint256" },
      { "internalType": "uint256", "name": "currentLiquidationThreshold", "type": "uint256" },
      { "internalType": "uint256", "name": "ltv", "type": "uint256" },
      { "internalType": "uint256", "name": "healthFactor", "type": "uint256" }
    ],
    "stateMutability": "view",
    "type": "function"
  },
  {
    "inputs": [
      { "internalType": "address", "name": "asset", "type": "address" }
    ],
    "name": "getConfiguration",
    "outputs": [
      { "internalType": "uint256", "name": "", "type": "uint256" }
    ],
    "stateMutability": "view",
    "type": "function"
  },
  {
    "inputs": [
      { "internalType": "address", "name": "asset", "type": "address" }
    ],
    "name": "getReserveData",
    "outputs": [
      {
        "components": [
          { "internalType": "uint256", "name": "configuration", "type": "uint256" },
          { "internalType": "uint128", "name": "liquidityIndex", "type": "uint128" },
          { "internalType": "uint128", "name": "currentLiquidityRate", "type": "uint128" },
          { "internalType": "uint128", "name": "variableBorrowIndex", "type": "uint128" },
          { "internalType": "uint128", "name": "currentVariableBorrowRate", "type": "uint128" },
          { "internalType": "uint128", "name": "currentStableBorrowRate", "type": "uint128" },
          { "internalType": "uint40", "name": "lastUpdateTimestamp", "type": "uint40" },
          { "internalType": "address", "name": "aTokenAddress", "type": "address" },
          { "internalType": "address", "name": "stableDebtTokenAddress", "type": "address" },
          { "internalType": "address", "name": "variableDebtTokenAddress", "type": "address" },
          { "internalType": "address", "name": "interestRateStrategyAddress", "type": "address" },
          { "internalType": "uint8", "name": "id", "type": "uint8" }
        ],
        "internalType": "struct DataTypes.ReserveData",
        "name": "",
        "type": "tuple"
      }
    ],
    "stateMutability": "view",
    "type": "function"
  },
  {
    "inputs": [],
    "name": "getReservesList",
    "outputs": [
      { "internalType": "address[]", "name": "", "type": "address[]" }
    ],
    "stateMutability": "view",
    "type": "function"
  },
  {
    "anonymous": false,
    "inputs": [
      { "indexed": true, "internalType": "address", "name": "reserve", "type": "address" },
      { "indexed": false, "internalType": "address", "name": "user", "type": "address" },
      { "indexed": true, "internalType": "address", "name": "onBehalfOf", "type": "address" },
      { "indexed": false, "internalType": "uint256", "name": "amount", "type": "uint256" },
      { "indexed": false, "internalType": "uint8", "name": "interestRateMode", "type": "uint8" },
      { "indexed": false, "internalType": "uint256", "name": "borrowRate", "type": "uint256" },
      { "indexed": true, "internalType": "uint16", "name": "referralCode", "type": "uint16" }
    ],
    "name": "Borrow",
    "type": "event"
  },
  {
    "anonymous": false,
    "inputs": [
      { "indexed": true, "internalType": "address", "name": "reserve", "type": "address" },
      { "indexed": true, "internalType": "address", "name": "user", "type": "address" },
      { "indexed": true, "internalType": "address", "name": "repayer", "type": "address" },
      { "indexed": false, "internalType": "uint256", "name": "amount", "type": "uint256" },
      { "indexed": false, "internalType": "bool", "name": "useATokens", "type": "bool" }
    ],
    "name": "Repay",
    "type": "event"
  }
]"#;
);


abigen!(
    FlashLiquidator,
    r#"[{
        "inputs": [
            { "internalType": "address", "name": "collateralAsset", "type": "address" },
            { "internalType": "address", "name": "debtAsset", "type": "address" },
            { "internalType": "address", "name": "user", "type": "address" },
            { "internalType": "uint256", "name": "debtToCover", "type": "uint256" },
            { "internalType": "uint256", "name": "MinimumAmountOut", "type": "uint256" }
        ],
        "name": "executeFlashLiquidation",
        "outputs": [],
        "stateMutability": "nonpayable",
        "type": "function"
    }]"#
);

abigen!(
    AaveOracle,
    r#"[
        {
            "inputs": [{"internalType": "address", "name": "asset", "type": "address"}],
            "name": "getAssetPrice",
            "outputs": [{"internalType": "uint256", "name": "", "type": "uint256"}],
            "stateMutability": "view",
            "type": "function"
        }
    ]"#

);

abigen!(
    Dex,
    r#"
[
  {
    "name": "getAmountsOut",
    "type": "function",
    "stateMutability": "view",
    "inputs": [
      {
        "name": "amountIn",
        "type": "uint256"
      },
      {
        "name": "path",
        "type": "address[]"
      }
    ],
    "outputs": [
      {
        "name": "amounts",
        "type": "uint256[]"
      }
    ]
  }
]
"#;
);



abigen!(
    IERC20,
    r#"[
        function balanceOf(address account) external view returns (uint256)
        function decimals() external view returns (uint8)
        function symbol() external view returns (string)
    ]"#
);

abigen!(
    UiPoolDataProvider,
    r#"[
      {
    "inputs": [
      {
        "internalType": "contract IPoolAddressesProvider",
        "name": "provider",
        "type": "address"
      },
      {
        "internalType": "address",
        "name": "user",
        "type": "address"
      }
    ],
    "name": "getUserReservesData",
    "outputs": [
      {
        "components": [
          {
            "internalType": "address",
            "name": "underlyingAsset",
            "type": "address"
          },
          {
            "internalType": "uint256",
            "name": "scaledATokenBalance",
            "type": "uint256"
          },
          {
            "internalType": "bool",
            "name": "usageAsCollateralEnabledOnUser",
            "type": "bool"
          },
          {
            "internalType": "uint256",
            "name": "scaledVariableDebt",
            "type": "uint256"
          }
        ],
        "internalType": "struct UserReserveData[]",
        "name": "",
        "type": "tuple[]"
      },
      {
        "internalType": "uint8",
        "name": "",
        "type": "uint8"
      }
    ],
    "stateMutability": "view",
    "type": "function"
  }
    ]"#
);






