use ethers::{
    abi::{self, Token},
    providers::Middleware,
    types::{Address, Bytes, U256},
};
use moka::future::Cache;
use std::time::Duration;
use tracing::{debug, error, trace, warn};

use crate::{
    common::abi_bindings::{
        Call3, ExactInputParams, ExactInputSingleParams, IMulticall3, IQuoterV2, ISwapRouter,
        QuoteExactInputSingleParams,
    },
    constants::{self, USDC, USDT, WETH},
    core::{ports::DexRouteFinder, types::MarketQuote},
};

/// Logarithmic scaling bucket to accurately group trade amounts (~10% resolution)
#[derive(Hash, Eq, PartialEq, Clone, Debug)]
pub struct CacheKey {
    pub src_token: Address,
    pub dest_token: Address,
    pub amount_bucket: u64,
}

impl CacheKey {
    pub fn new(src_token: Address, dest_token: Address, amount: U256) -> Self {
        if amount.is_zero() {
            return Self {
                src_token,
                dest_token,
                amount_bucket: 0,
            };
        }

        // Zero-allocation order of magnitude + most significant digit calculation
        let bits = amount.bits() as u64;
        let approx_magnitude = bits * 301 / 1000; // log10 approximation via bit-shift
        let scaling_factor = U256::from(10).pow(U256::from(approx_magnitude.saturating_sub(1)));
        let most_significant = if !scaling_factor.is_zero() {
            (amount / scaling_factor).as_u64() % 10
        } else {
            0
        };

        let amount_bucket = (approx_magnitude * 10) + most_significant;

        Self {
            src_token,
            dest_token,
            amount_bucket,
        }
    }
}

#[derive(Clone, Debug)]
pub enum RouteTopology {
    SingleHop { fee: u32 },
    MultiHop { path: Bytes },
}

pub struct UniswapV3Adapter<M: Middleware> {
    pub quoter: IQuoterV2<M>,
    pub router: ISwapRouter<M>,
    pub multicall: IMulticall3<M>,
    route_cache: Cache<CacheKey, RouteTopology>,
}

impl<M: Middleware + 'static> UniswapV3Adapter<M> {
    pub fn new(quoter: IQuoterV2<M>, router: ISwapRouter<M>, multicall: IMulticall3<M>) -> Self {
        let route_cache = Cache::builder()
            .time_to_live(Duration::from_secs(3))
            .max_capacity(1_000)
            .build();

        Self {
            quoter,
            router,
            multicall,
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

        // 1. CACHE HIT: Re-evaluate known topology directly
        if let Some(topology) = self.route_cache.get(&cache_key).await {
            debug!(target: "liq_ranger", "Cache hit for pair {:?} -> {:?}", src_token, dest_token);
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

        // 2. CACHE MISS: Execute all single + multi-hop candidates in 1 Multicall3 batch
        debug!(target: "liq_ranger", "Batch querying all routes via Multicall3 for {:?} -> {:?}", src_token, dest_token);

        let (best_out, best_topology) = self
            .find_best_route_multicall(src_token, dest_token, amount, hubs)
            .await;

        if !best_out.is_zero() {
            debug!(target: "liq_ranger", "Discovered optimal route out: {} for {:?} -> {:?}", best_out, src_token, dest_token);
            self.route_cache.insert(cache_key, best_topology.clone()).await;
        } else {
            warn!(target: "liq_ranger", "No valid Uniswap V3 path found for {:?} -> {:?}", src_token, dest_token);
        }

        (best_out, best_topology)
    }

    async fn find_best_route_multicall(
        &self,
        src_token: Address,
        dest_token: Address,
        amount: U256,
        hubs: &[Address],
    ) -> (U256, RouteTopology) {
        let single_fee_tiers = [100u32, 500u32, 3000u32, 10000u32];
        let multihop_fee_tiers = [500u32, 3000u32]; // High-liquidity tiers only for 2-hop

        let valid_hubs: Vec<Address> = hubs
            .iter()
            .copied()
            .filter(|&hub| hub != src_token && hub != dest_token)
            .collect();

        let mut multicall_calls: Vec<Call3> = Vec::new();
        let mut route_metadata = Vec::new();

        // A. Build Single-Hop Calls (4 Fee Tiers)
        for &fee in &single_fee_tiers {
            let params = QuoteExactInputSingleParams {
                token_in: src_token,
                token_out: dest_token,
                amount_in: amount,
                fee,
                sqrt_price_limit_x96: U256::zero(),
            };

            if let Some(call_data) = self.quoter.quote_exact_input_single(params).calldata() {
                multicall_calls.push(Call3 {
                    target: self.quoter.address(),
                    allow_failure: true,
                    call_data,
                });
                route_metadata.push(RouteMeta::SingleHop { fee });
            }
        }

        // B. Build Multi-Hop Calls (Across Intermediate Hub Pairs)
        for &hub in &valid_hubs {
            for &fee1 in &multihop_fee_tiers {
                for &fee2 in &multihop_fee_tiers {
                    let path = encode_v3_path(&[src_token, hub, dest_token], &[fee1, fee2]);
                    if let Some(call_data) = self.quoter.quote_exact_input(path.clone(), amount).calldata() {
                        multicall_calls.push(Call3 {
                            target: self.quoter.address(),
                            allow_failure: true,
                            call_data,
                        });
                        route_metadata.push(RouteMeta::MultiHop { path });
                    }
                }
            }
        }

        if multicall_calls.is_empty() {
            return (U256::zero(), RouteTopology::SingleHop { fee: 3000 });
        }

        // C. Send Single RPC Request via Multicall3
        let aggregate_result = match self.multicall.aggregate_3(multicall_calls).call().await {
            Ok(res) => res,
            Err(e) => {
                error!(target: "liq_ranger", "Multicall3 aggregate_3 failed: {:?}", e);
                return (U256::zero(), RouteTopology::SingleHop { fee: 3000 });
            }
        };

        // D. Parse Batch Responses
        let mut best_out = U256::zero();
        let mut best_topology = RouteTopology::SingleHop { fee: 3000 };

        for (i, response) in aggregate_result.into_iter().enumerate() {
            if !response.success || response.return_data.is_empty() {
                continue;
            }

            let is_multihop = matches!(&route_metadata[i], RouteMeta::MultiHop { .. });

            // QuoterV2 return signatures differ between Single-Hop and Multi-Hop:
            // Single: (uint256, uint160, uint32, uint256)
            // Multi:  (uint256, uint160[], uint32[], uint256)
            let decoded_result = if is_multihop {
                abi::decode(
                    &[
                        abi::ParamType::Uint(256),
                        abi::ParamType::Array(Box::new(abi::ParamType::Uint(160))),
                        abi::ParamType::Array(Box::new(abi::ParamType::Uint(32))),
                        abi::ParamType::Uint(256),
                    ],
                    &response.return_data,
                )
            } else {
                abi::decode(
                    &[
                        abi::ParamType::Uint(256),
                        abi::ParamType::Uint(160),
                        abi::ParamType::Uint(32),
                        abi::ParamType::Uint(256),
                    ],
                    &response.return_data,
                )
            };

            if let Ok(decoded) = decoded_result {
                if let Some(Token::Uint(amount_out)) = decoded.get(0) {
                    if *amount_out > best_out {
                        best_out = *amount_out;
                        best_topology = match &route_metadata[i] {
                            RouteMeta::SingleHop { fee } => RouteTopology::SingleHop { fee: *fee },
                            RouteMeta::MultiHop { path } => RouteTopology::MultiHop { path: path.clone() },
                        };
                    }
                }
            }
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
}

enum RouteMeta {
    SingleHop { fee: u32 },
    MultiHop { path: Bytes },
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