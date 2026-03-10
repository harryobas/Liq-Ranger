
use ethers::prelude::abigen;

abigen!(
    IComet,
    "abis/compound/comet.json",
    event_derives(serde::Deserialize, serde::Serialize)
);