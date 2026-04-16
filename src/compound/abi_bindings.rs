
use ethers::prelude::abigen;

abigen!(
    IComet,
    "src/abis/compound/comet.json",
    event_derives(serde::Deserialize, serde::Serialize)
);