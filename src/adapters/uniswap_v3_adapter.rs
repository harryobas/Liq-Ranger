use ethers::{
    providers::Middleware,
    types::{Address, Bytes, U256},
};
use futures_util::stream::{self, StreamExt};
use moka::future::Cache;
use std::time::Duration;
use tracing::{debug, error, trace, warn};

use crate::{
    common::abi_bindings::{
        ExactInputParams, ExactInputSingleParams, IQuoterV2, ISwapRouter,
        QuoteExactInputSingleParams,
    },
    constants::{self, USDC, USDT, WETH},
    core::{ports::DexRouteFinder, types::MarketQuote},
};

/// Amount-aware cache key to separate routes by trade size magnitude
#[derive(Hash, Eq, PartialEq, Clone, Debug)]
pub struct CacheKey {
    pub src_token: Address,
    pub dest_token: Address,
    pub amount_bucket: u32,
}

impl CacheKey {
    pub fn new(src_token: Address, dest_token: Address, amount: U256) -> Self {
        Self {
            src_token,
            dest_token,
            amount_bucket: amount.bits() as u32,
        }
    }
}

/// Represents the structure/topology of a Uniswap V3 path
#[derive(Clone, Debug)]
pub enum RouteTopology {
    SingleHop { fee: u32 },
    MultiHop { path: Bytes },
}

const CONCURRENCY_LIMIT: usize = 4;

pub struct UniswapV3Adapter<M: Middleware> {
    pub quoter: IQuoterV2<M>,
    pub router: ISwapRouter<M>,
    route_cache: Cache<CacheKey, RouteTopology>,
}

impl<M: Middleware> UniswapV3Adapter<M> {
    pub fn new(quoter: IQuoterV2<M>, router: ISwapRouter<M>) -> Self {
        let route_cache = Cache::builder()
            .time_to_live(Duration::from_secs(3))
            .max_capacity(1_000)
            .build();

        Self {
            quoter,
            router,
            route_cache,
        }
    }

    async fn evaluate_route(
        &self,
        src_token: Address,
        dest_token: Address,
        amount: U256,
        hubs: &[Address],
    ) -> (U256, RouteTopology) {
        let cache_key = CacheKey::new(src_token, dest_token, amount);

        // 1. CACHE HIT
        if let Some(topology) = self.route_cache.get(&cache_key).await {
            debug!("Cache hit for pair {:?} -> {:?}", src_token, dest_token);
            match &topology {
                RouteTopology::SingleHop { fee } => {
                    let out = self
                        .quote_single_hop(src_token, dest_token, *fee, amount)
                        .await;
                    if !out.is_zero() {
                        return (out, topology);
                    }
                }
                RouteTopology::MultiHop { path } => {
                    if let Ok((out, _, _, _)) = self
                        .quoter
                        .quote_exact_input(path.clone(), amount)
                        .call()
                        .await
                    {
                        if !out.is_zero() {
                            return (out, topology);
                        }
                    }
                }
            }
        }

        self.route_cache.invalidate(&cache_key).await;

        // 2. CACHE MISS: Sequentially query single vs multi-hop to prevent RPC connection starvation
        debug!("Fetching fresh quotes for {:?} -> {:?}, amount: {}", src_token, dest_token, amount);

        let (single_out, single_fee) = self.find_best_single_hop(src_token, dest_token, amount).await;
        let (multi_out, packed_path) = self.find_best_multi_hop(src_token, dest_token, amount, hubs).await;

        let (best_out, best_topology) = if multi_out > single_out {
            (multi_out, RouteTopology::MultiHop { path: packed_path })
        } else {
            (single_out, RouteTopology::SingleHop { fee: single_fee })
        };

        if !best_out.is_zero() {
            debug!(target: "liq_ranger", "Discovered optimal route out: {} for {:?} -> {:?}", best_out, src_token, dest_token);
            self.route_cache.insert(cache_key, best_topology.clone()).await;
        } else {
            warn!(target: "liq_ranger", "No valid Uniswap V3 path found for {:?} -> {:?}", src_token, dest_token);
        }

        (best_out, best_topology)
    }

    async fn quote_single_hop(
        &self,
        token_in: Address,
        token_out: Address,
        fee: u32,
        amount_in: U256,
    ) -> U256 {
        let params = QuoteExactInputSingleParams {
            token_in,
            token_out,
            amount_in,
            fee,
            sqrt_price_limit_x96: U256::zero(),
        };

        match self.quoter.quote_exact_input_single(params).call().await {
            Ok((amount_out, _, _, _)) => amount_out,
            Err(e) => {
                trace!(target: "liq_ranger", "Single hop failed for fee {}: {:?}", fee, e);
                U256::zero()
            }
        }
    }

    async fn find_best_single_hop(
        &self,
        token_in: Address,
        token_out: Address,
        amount_in: U256,
    ) -> (U256, u32) {
        let fee_tiers = [100u32, 500u32, 3000u32, 10000u32];

        let results = stream::iter(fee_tiers.into_iter().map(|fee| async move {
            let out = self.quote_single_hop(token_in, token_out, fee, amount_in).await;
            (fee, out)
        }))
        .buffer_unordered(CONCURRENCY_LIMIT)
        .collect::<Vec<_>>()
        .await;

        results
            .into_iter()
            .max_by_key(|(_, amount_out)| *amount_out)
            .map(|(fee, amount_out)| (amount_out, fee))
            .unwrap_or((U256::zero(), 0))
    }

    pub async fn find_best_multi_hop(
        &self,
        token_in: Address,
        token_out: Address,
        amount_in: U256,
        intermediates: &[Address],
    ) -> (U256, Bytes) {
        let valid_intermediates: Vec<Address> = intermediates
            .iter()
            .copied()
            .filter(|&intermediate| intermediate != token_in && intermediate != token_out)
            .collect();

        let mut best_out = U256::zero();
        let mut best_path = Bytes::default();

        for intermediate in valid_intermediates {
            let (out_leg1, fee1) = self.find_best_single_hop(token_in, intermediate, amount_in).await;
            if out_leg1.is_zero() {
                continue;
            }

            let (out_leg2, fee2) = self.find_best_single_hop(intermediate, token_out, out_leg1).await;
            if out_leg2.is_zero() {
                continue;
            }

            if out_leg2 > best_out {
                best_out = out_leg2;
                best_path = encode_v3_path(&[token_in, intermediate, token_out], &[fee1, fee2]);
            }
        }

        (best_out, best_path)
    }
}

#[async_trait::async_trait]
impl<M: Middleware + 'static> DexRouteFinder for UniswapV3Adapter<M> {
    async fn get_swap_quote(
        &self,
        src_token: Address,
        dest_token: Address,
        _src_decimals: u8,
        _dest_decimals: u8,
        amount: U256,
    ) -> anyhow::Result<MarketQuote> {
        debug!(target: "liq_ranger", "Requesting swap quote for {:?} -> {:?}, amount {}", src_token, dest_token, amount);

        let hubs = [*WETH, *USDC, *USDT];

        let (expected_out, topology) = self
            .evaluate_route(src_token, dest_token, amount, &hubs)
            .await;

        if expected_out.is_zero() {
            error!(target: "liq_ranger", "Failed to find Uniswap V3 quote for {:?} -> {:?}", src_token, dest_token);
            return Err(anyhow::anyhow!(
                "No Uniswap V3 liquidity found for pair {:?} -> {:?}",
                src_token,
                dest_token
            ));
        }

        let min_amt_out = calculate_min_amount_out(expected_out, constants::SLIPPAGE_BPS);

        let swap_data = match topology {
            RouteTopology::MultiHop { path } => {
                let params = ExactInputParams {
                    path,
                    recipient: *constants::FLASH_LIQUIDATOR,
                    amount_in: amount,
                    amount_out_minimum: min_amt_out,
                };
                self.router
                    .exact_input(params)
                    .calldata()
                    .ok_or_else(|| anyhow::anyhow!("Failed to encode exactInput calldata"))?
            }
            RouteTopology::SingleHop { fee } => {
                let params = ExactInputSingleParams {
                    token_in: src_token,
                    token_out: dest_token,
                    fee,
                    recipient: *constants::FLASH_LIQUIDATOR,
                    amount_in: amount,
                    amount_out_minimum: min_amt_out,
                    sqrt_price_limit_x96: U256::zero(),
                };
                self.router
                    .exact_input_single(params)
                    .calldata()
                    .ok_or_else(|| anyhow::anyhow!("Failed to encode exactInputSingle calldata"))?
            }
        };

        debug!(target: "liq_ranger", "Quote successfully built! Target: {:?}, Min Out: {}", self.router.address(), min_amt_out);

        Ok(MarketQuote {
            swap_target: self.router.address(),
            token_transfer_proxy: self.router.address(),
            swap_data,
            min_amt_out,
        })
    }
}

fn encode_v3_path(tokens: &[Address], fees: &[u32]) -> Bytes {
    assert_eq!(tokens.len(), fees.len() + 1);
    let mut packed = Vec::with_capacity(tokens.len() * 20 + fees.len() * 3);

    for i in 0..fees.len() {
        packed.extend_from_slice(tokens[i].as_bytes());
        let fee_bytes = fees[i].to_be_bytes();
        packed.extend_from_slice(&fee_bytes[1..4]);
    }

    packed.extend_from_slice(tokens.last().unwrap().as_bytes());
    Bytes::from(packed)
}

fn calculate_min_amount_out(expected_out: U256, slippage_bps: u32) -> U256 {
    let multiplier = U256::from(10000 - slippage_bps);
    let divider = U256::from(10000);
    (expected_out * multiplier) / divider
}