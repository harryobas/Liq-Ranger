use anyhow::{Context, Result};
use ethers::{
    types::{Address, Bytes, U256},
    utils::hex,
};
use reqwest::Client;
use serde::Deserialize;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::common::SwapQueryParams;

/// Price route response from Paraswap
#[derive(Debug, Deserialize)]
struct PriceRouteResponse {
    #[serde(rename = "priceRoute", alias = "price_route")]
    pub price_route: serde_json::Value, // Keep it generic for now
}

/// Transaction response from Paraswap
#[derive(Debug, Deserialize)]
struct TransactionResponse {
    pub to: String,
    pub from: String,
    pub data: String,
    pub value: String,
    pub gas_price: Option<String>,
}

pub struct ParaSwapClient {
    pub http_client: Client,
    base_url: String,
}

#[derive(Debug)]
pub struct SwapData {
    pub swap_target: Address,
    pub swap_data: Bytes,
    pub token_transfer_proxy: Address,
    pub dest_amount: U256,
    pub src_amount: U256,
    pub min_amt_out: U256,
}

impl ParaSwapClient {
    pub fn new() -> Self {
        ParaSwapClient {
            http_client: Client::new(),
            base_url: "https://api.paraswap.io".to_string(),
        }
    }

    #[cfg(test)]
    fn new_with_base_url(base_url: String) -> Self {
        ParaSwapClient {
            http_client: Client::new(),
            base_url,
        }
    }

    /// Step 1: Call /prices to get optimal route
    async fn get_price_route(&self, params: &SwapQueryParams) -> Result<PriceRouteResponse> {
        let url = format!("{}/prices", self.base_url);

        let query = [
            ("srcToken", params.src_token.clone()),
            ("destToken", params.dest_token.clone()),
            ("srcDecimals", params.src_decimals.to_string()),
            ("destDecimals", params.dest_decimals.to_string()),
            ("amount", params.amount.clone()),
            ("side", params.side.clone()),
            ("network", params.chain_id.to_string()),
            ("userAddress", params.user_address.clone()),
            ("version", "6.2".to_string()),
        ];

        let resp = self
            .http_client
            .get(url)
            .query(&query)
            .send()
            .await?
            .error_for_status()? // check HTTP 200
            .json::<PriceRouteResponse>()
            .await?;

        Ok(resp)
    }

    /// Step 2: Call /transactions/:network to build transaction calldata
    async fn build_transaction(
        &self,
        params: &SwapQueryParams,
        price_route: &serde_json::Value,
    ) -> Result<TransactionResponse> {
        let url = format!("{}/transactions/{}", self.base_url, params.chain_id);

        let deadline = SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs() + 300;

        let body = serde_json::json!({
            "srcToken": params.src_token,
            "destToken": params.dest_token,
            "srcDecimals": params.src_decimals,
            "destDecimals": params.dest_decimals,
            "srcAmount": params.amount,
            "priceRoute": price_route,
            "userAddress": params.user_address,
            "receiver": params.receiver,
            "slippage": params.slippage_bps,
            "deadline": deadline
        });

        let resp = self
            .http_client
            .post(&url)
            .json(&body)
            .send()
            .await?
            .error_for_status()?
            .json::<TransactionResponse>()
            .await?;

        Ok(resp)
    }

    pub async fn compose_swap_data(&self, params: SwapQueryParams) -> anyhow::Result<SwapData> {
        // Step 1: Get price route
        let price_route_response = self.get_price_route(&params).await?;

        // Step 2: Extract dest_amount from price_route JSON
        // The structure is: price_route.price_route.destAmount
        let dest_amount_str = price_route_response.price_route["destAmount"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("Failed to extract destAmount from price route"))?;

        let src_amount_str = price_route_response.price_route["srcAmount"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("Failed to extract srcAmount from price route"))?;

        // Parse dest_amount as U256
        let dest_amount = U256::from_dec_str(dest_amount_str)
            .with_context(|| format!("Failed to parse destAmount: {}", dest_amount_str))?;

        // Parse amount_in from params
        let src_amount = U256::from_dec_str(src_amount_str)
            .with_context(|| format!("Failed to parse amount_in: {}", params.amount))?;

        let slippage_bps = U256::from(params.slippage_bps);

        let min_amt_out = dest_amount
            .checked_mul(U256::from(10000) - slippage_bps)
            .ok_or_else(|| anyhow::anyhow!("Multiplication overflow"))?
            / U256::from(10000);

        // Step 3: Build transaction
        let tx_response = self
            .build_transaction(&params, &price_route_response.price_route)
            .await?;

        // Step 4: Extract tokenTransferProxy from price_route
        let token_transfer_proxy_str = price_route_response.price_route["tokenTransferProxy"]
            .as_str()
            .ok_or_else(|| {
                anyhow::anyhow!("Failed to extract tokenTransferProxy from price route")
            })?;

        // Step 5: Convert string addresses to Address type
        let swap_target = tx_response
            .to
            .parse::<Address>()
            .with_context(|| format!("Invalid swap_target address: {}", tx_response.to))?;

        let token_transfer_proxy =
            token_transfer_proxy_str
                .parse::<Address>()
                .with_context(|| {
                    format!(
                        "Invalid token_transfer_proxy address: {}",
                        token_transfer_proxy_str
                    )
                })?;

        // Step 6: Convert hex data to Bytes
        let swap_data = if tx_response.data.starts_with("0x") {
            Bytes::from(hex::decode(&tx_response.data[2..])?)
        } else {
            Bytes::from(hex::decode(&tx_response.data)?)
        };

        Ok(SwapData {
            swap_target,
            swap_data,
            token_transfer_proxy,
            dest_amount,
            src_amount,
            min_amt_out,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    fn addr(n: u64) -> Address {
        Address::from_low_u64_be(n)
    }

    fn addr_string(n: u64) -> String {
        format!("{:?}", addr(n))
    }

    fn swap_params() -> SwapQueryParams {
        SwapQueryParams {
            src_token: addr_string(1),
            dest_token: addr_string(2),
            src_decimals: 18,
            dest_decimals: 6,
            amount: "1000".to_string(),
            side: "SELL".to_string(),
            chain_id: 137,
            user_address: addr_string(3),
            receiver: addr_string(3),
            slippage_bps: 50,
        }
    }

    async fn spawn_mock_server(price_body: &'static str, tx_body: &'static str) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let addr = listener.local_addr().expect("local addr");

        tokio::spawn(async move {
            for _ in 0..2 {
                let (mut socket, _) = listener.accept().await.expect("accept");
                let mut buf = vec![0u8; 4096];
                let n = socket.read(&mut buf).await.expect("read");
                let request = String::from_utf8_lossy(&buf[..n]);
                let body = if request.starts_with("GET /prices") {
                    price_body
                } else {
                    tx_body
                };

                let response = format!(
                    "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
                    body.len(),
                    body
                );
                socket.write_all(response.as_bytes()).await.expect("write");
            }
        });

        format!("http://{}", addr)
    }

    #[tokio::test]
    async fn compose_swap_data_builds_price_then_transaction() {
        let token_transfer_proxy = addr(4);
        let swap_target = addr(5);
        let price_body = Box::leak(
            format!(
                r#"{{
                    "price_route": {{
                        "destAmount": "2000",
                        "srcAmount": "1000",
                        "tokenTransferProxy": "{}"
                    }}
                }}"#,
                addr_string(4)
            )
            .into_boxed_str(),
        );
        let tx_body = Box::leak(
            format!(
                r#"{{
                    "to": "{}",
                    "from": "{}",
                    "data": "0xabcdef",
                    "value": "0"
                }}"#,
                addr_string(5),
                addr_string(3)
            )
            .into_boxed_str(),
        );
        let base_url = spawn_mock_server(price_body, tx_body).await;
        let client = ParaSwapClient::new_with_base_url(base_url);

        let data = client
            .compose_swap_data(swap_params())
            .await
            .expect("swap data");

        assert_eq!(data.swap_target, swap_target);
        assert_eq!(data.token_transfer_proxy, token_transfer_proxy);
        assert_eq!(data.src_amount, U256::from(1_000u64));
        assert_eq!(data.dest_amount, U256::from(2_000u64));
        assert_eq!(data.min_amt_out, U256::from(1_990u64));
        assert_eq!(data.swap_data, Bytes::from(vec![0xab, 0xcd, 0xef]));
    }

    #[tokio::test]
    async fn compose_swap_data_errors_when_dest_amount_missing() {
        let price_body = r#"{"price_route":{"srcAmount":"1000","tokenTransferProxy":"0x0000000000000000000000000000000000000004"}}"#;
        let tx_body = r#"{"to":"0x0000000000000000000000000000000000000005","from":"0x0000000000000000000000000000000000000003","data":"0xabcdef","value":"0"}"#;
        let base_url = spawn_mock_server(price_body, tx_body).await;
        let client = ParaSwapClient::new_with_base_url(base_url);

        let err = client
            .compose_swap_data(swap_params())
            .await
            .expect_err("missing dest amount");

        assert!(err.to_string().contains("destAmount"));
    }

    #[tokio::test]
    async fn compose_swap_data_errors_on_invalid_swap_target() {
        let price_body = r#"{"price_route":{"destAmount":"2000","srcAmount":"1000","tokenTransferProxy":"0x0000000000000000000000000000000000000004"}}"#;
        let tx_body = r#"{"to":"not-an-address","from":"0x0000000000000000000000000000000000000003","data":"abcdef","value":"0"}"#;
        let base_url = spawn_mock_server(price_body, tx_body).await;
        let client = ParaSwapClient::new_with_base_url(base_url);

        let err = client
            .compose_swap_data(swap_params())
            .await
            .expect_err("invalid address");

        assert!(err.to_string().contains("Invalid swap_target address"));
    }
}
