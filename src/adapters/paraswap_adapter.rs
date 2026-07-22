// dex_adapters/src/paraswap.rs

use anyhow::{Context, Result};
use async_trait::async_trait;
use ethers::{
    types::{Address, Bytes, U256},
    utils::hex,
};
use reqwest::{Client, StatusCode};
use serde::Deserialize;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::constants::SLIPPAGE_BPS;
use crate::core::ports::DexRouteFinder;
use crate::core::types::MarketQuote;
use backoff::{future::retry, ExponentialBackoff};

#[derive(Debug, Deserialize)]
struct PriceRouteResponse {
    #[serde(rename = "priceRoute", alias = "price_route")]
    pub price_route: serde_json::Value,
}

#[derive(Debug, Deserialize)]
struct TransactionResponse {
    pub to: String,
    pub data: String,
}

pub struct ParaSwapAdapter {
    pub http_client: Client,
    base_url: String,
    flash_liq_address: Address,
    chain_id: u64,
}

impl ParaSwapAdapter {
    pub fn new(flash_liq_address: Address, chain_id: u64) -> Self {
        Self {
            http_client: Client::new(),
            base_url: "https://api.paraswap.io".to_string(),
            flash_liq_address,
            chain_id,
        }
    }

    async fn execute_get_with_retry(
        &self,
        url: &str,
        query: &[(&str, String)],
    ) -> anyhow::Result<reqwest::Response> {
        let backoff_strategy = ExponentialBackoff {
            max_elapsed_time: Some(std::time::Duration::from_secs(10)), // Abandon loop after 10 seconds max
            ..Default::default()
        };

        let operation = || async move {
            let resp = self
                .http_client
                .get(url)
                .query(query)
                .send()
                .await
                .map_err(|e| anyhow::anyhow!(e))
                .map_err(backoff::Error::transient)?;

            match resp.status() {
                StatusCode::OK => Ok(resp),
                StatusCode::TOO_MANY_REQUESTS => {
                    tracing::warn!(
                        "ParaSwap hit 429 Rate Limit. Retrying with exponential backoff..."
                    );
                    Err(backoff::Error::transient(anyhow::anyhow!(
                        "Rate limited (429)"
                    )))
                }
                status if status.is_server_error() => Err(backoff::Error::transient(
                    anyhow::anyhow!("Server error: {}", status),
                )),
                status => Err(backoff::Error::permanent(anyhow::anyhow!(
                    "Fatal HTTP error: {}",
                    status
                ))),
            }
        };

        retry(backoff_strategy, operation).await
    }

    async fn get_price_route(
        &self,
        src_token: Address,
        dest_token: Address,
        amount: U256,
        src_decimals: u8,
        dest_decimals: u8,
    ) -> Result<PriceRouteResponse> {
        let url = format!("{}/prices", self.base_url);

        // Fix: Force strict lower-case string conversions without EIP-55 Checksum formatting modifications
        let query = [
            ("srcToken", format!("{:#x}", src_token)),
            ("destToken", format!("{:#x}", dest_token)),
            ("srcDecimals", src_decimals.to_string()),
            ("destDecimals", dest_decimals.to_string()),
            ("amount", amount.to_string()),
            ("side", "SELL".to_string()),
            ("network", self.chain_id.to_string()),
            ("userAddress", format!("{:#x}", self.flash_liq_address)),
            ("version", "6.2".to_string()),
        ];

        let resp = self.execute_get_with_retry(&url, &query).await?;
        let price_route = resp.json::<PriceRouteResponse>().await?;

        Ok(price_route)
    }

    async fn build_transaction(
        &self,
        src_token: Address,
        dest_token: Address,
        amount: U256,
        src_decimals: u8,
        dest_decimals: u8,
        slippage_bps: u32,
        price_route: &serde_json::Value,
    ) -> Result<TransactionResponse> {
        let url = format!("{}/transactions/{}", self.base_url, self.chain_id);
        let deadline = SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs() + 300;

        let body = serde_json::json!({
            "srcToken": format!("{:#x}", src_token),
            "destToken": format!("{:#x}", dest_token),
            "srcDecimals": src_decimals,
            "destDecimals": dest_decimals,
            "srcAmount": amount.to_string(),
            "priceRoute": price_route,
            "userAddress": format!("{:#x}", self.flash_liq_address),
            "receiver": format!("{:#x}", self.flash_liq_address),
            "slippage": slippage_bps,
            "deadline": deadline
        });

        let backoff_strategy = ExponentialBackoff {
            max_elapsed_time: Some(std::time::Duration::from_secs(10)),
            ..Default::default()
        };

        let operation = || async {
            let resp = self
                .http_client
                .post(&url)
                .json(&body)
                .send()
                .await
                .map_err(|e| anyhow::anyhow!(e))
                .map_err(backoff::Error::transient)?;

            match resp.status() {
                StatusCode::OK | StatusCode::CREATED => Ok(resp),
                StatusCode::TOO_MANY_REQUESTS => {
                    tracing::warn!("ParaSwap Tx Builder hit 429 Rate Limit. Retrying...");
                    Err(backoff::Error::transient(anyhow::anyhow!(
                        "Tx rate limited"
                    )))
                }
                status if status.is_server_error() => Err(backoff::Error::transient(
                    anyhow::anyhow!("Server error: {}", status),
                )),
                status => Err(backoff::Error::permanent(anyhow::anyhow!(
                    "Fatal: {}",
                    status
                ))),
            }
        };

        let resp = retry(backoff_strategy, operation).await?;
        let tx_res = resp.json::<TransactionResponse>().await?;
        Ok(tx_res)
    }
}

#[async_trait]
impl DexRouteFinder for ParaSwapAdapter {
    async fn get_swap_quote(
        &self,
        src_token: Address,
        dest_token: Address,
        src_decimals: u8,
        dest_decimals: u8,
        amount: U256,
    ) -> anyhow::Result<MarketQuote> {
        let price_route_response = self
            .get_price_route(src_token, dest_token, amount, src_decimals, dest_decimals)
            .await?;

        let route_data = &price_route_response.price_route;

        let dest_amount = if let Some(as_str) = route_data["destAmount"].as_str() {
            U256::from_dec_str(as_str)?
        } else if let Some(as_u64) = route_data["destAmount"].as_u64() {
            U256::from(as_u64)
        } else {
            anyhow::bail!(
                "ParaSwap payload 'destAmount' is neither a valid string nor integer: {:?}",
                route_data["destAmount"]
            );
        };

        // Guard against math calculations overflow bounds during slippage configurations mutations
        let min_amt_out = dest_amount
            .checked_mul(U256::from(10000) - U256::from(SLIPPAGE_BPS))
            .ok_or_else(|| anyhow::anyhow!("Slippage computation multiplication overflow"))?
            / U256::from(10000);

        let tx_response = self
            .build_transaction(
                src_token,
                dest_token,
                amount,
                src_decimals,
                dest_decimals,
                SLIPPAGE_BPS,
                route_data,
            )
            .await?;

        let token_transfer_proxy_str =
            route_data["tokenTransferProxy"].as_str().ok_or_else(|| {
                anyhow::anyhow!("Failed to extract tokenTransferProxy from price route")
            })?;

        let swap_target = tx_response
            .to
            .parse::<Address>()
            .with_context(|| format!("Invalid swap_target address: {}", tx_response.to))?;

        let token_transfer_proxy =
            token_transfer_proxy_str
                .parse::<Address>()
                .with_context(|| {
                    format!("Invalid token_transfer_proxy: {}", token_transfer_proxy_str)
                })?;

        // Safely trim '0x' prefixes to avoid hex decoder panics
        let clean_hex_data = if tx_response.data.starts_with("0x") {
            &tx_response.data[2..]
        } else {
            &tx_response.data
        };
        let swap_data = Bytes::from(hex::decode(clean_hex_data)?);

        Ok(MarketQuote {
            swap_target,
            token_transfer_proxy,
            swap_data,
            min_amt_out,
        })
    }
}
