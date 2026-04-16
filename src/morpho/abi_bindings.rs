

use ethers::{prelude::abigen, };

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

