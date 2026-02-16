use anyhow::{Result, Context};
use reqwest::Client;
use serde::Deserialize;
use std::time::{SystemTime, UNIX_EPOCH};
 use ethers::{
    types::{Address, Bytes, U256},
    utils::hex
};

use crate::common::SwapQueryParams;

/// Price route response from Paraswap
#[derive(Debug, Deserialize)]
 struct PriceRouteResponse {
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
}

pub struct SwapData {
    pub swap_target: Address,
    pub swap_data: Bytes,
    pub token_transfer_proxy: Address,
    pub dest_amount: U256,
    pub src_amount: U256,
}

impl ParaSwapClient {
    pub fn new() -> Self {
        ParaSwapClient {
            http_client: Client::new(),
        }
    }

    /// Step 1: Call /prices to get optimal route
     async fn get_price_route(&self, params: &SwapQueryParams) -> Result<PriceRouteResponse> {
        let url = "https://api.paraswap.io/prices";

        let query = [
            ("srcToken", params.src_token.clone()),
            ("destToken", params.dest_token.clone()),
            ("srcDecimals", params.src_decimals.to_string()),
            ("destDecimals", params.dest_decimals.to_string()),
            ("amount", params.amount.clone()),
            ("side", params.side.clone()),
            ("network", params.chain_id.to_string()),
            ("userAddress", params.user_address.clone()),
            ("version", "6.2".to_string())
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
        let url = format!("https://api.paraswap.io/transactions/{}", params.chain_id);

        // 10 minute deadline
        let deadline = SystemTime::now()
            .duration_since(UNIX_EPOCH)?
            .as_secs()
            + 600;

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
        let dest_amount = dest_amount_str
            .parse::<U256>()
            .with_context(|| format!("Failed to parse destAmount: {}", dest_amount_str))?;
        
        // Parse amount_in from params
        let src_amount = src_amount_str
            .parse::<U256>()
            .with_context(|| format!("Failed to parse amount_in: {}", params.amount))?;
        
        // Step 3: Build transaction
        let tx_response = self.build_transaction(&params, &price_route_response.price_route).await?;
        
        // Step 4: Extract tokenTransferProxy from price_route
        let token_transfer_proxy_str = price_route_response.price_route["tokenTransferProxy"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("Failed to extract tokenTransferProxy from price route"))?;
        
        // Step 5: Convert string addresses to Address type
        let swap_target = tx_response.to
            .parse::<Address>()
            .with_context(|| format!("Invalid swap_target address: {}", tx_response.to))?;
        
        let token_transfer_proxy = token_transfer_proxy_str
            .parse::<Address>()
            .with_context(|| format!("Invalid token_transfer_proxy address: {}", token_transfer_proxy_str))?;
        
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
        })
    }
}

