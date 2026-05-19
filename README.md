# Liq-Ranger

Cross-protocol liquidation bot for Polygon PoS.

## Overview

Liq-Ranger monitors liquidation and collateral-purchase opportunities across Aave v3, Morpho Blue, and Compound/Comet. It keeps persistent protocol watchlists, reacts to new blocks, simulates candidate transactions on an Anvil fork, and executes profitable calls through a flash liquidator contract.

The bot also records liquidation and profit-distribution history to SQLite for Grafana dashboards.

## What It Does

- Tracks Aave v3 borrowers and generates liquidation candidates from unhealthy health factors.
- Tracks Morpho Blue market positions and applies Morpho-specific liquidation math.
- Tracks Compound/Comet collateral purchase opportunities when reserves are below target.
- Uses ParaSwap quotes to build swap calldata for seized collateral.
- Simulates candidate liquidations before sending real transactions.
- Persists watchlists and bootstrap state with sled.
- Persists liquidation and distribution history with SQLite.
- Runs scheduled profit distribution and gas-refuel maintenance.

## Entry Points

- `src/main.rs` loads environment variables, initializes tracing, and starts the bot.
- `src/lib.rs` wires providers, signer middleware, storage, contracts, bootstraps, engines, and background tasks.
- `src/liquidation_executor.rs` runs protocol liquidators every configured block interval.
- `src/block_watcher.rs` subscribes to Polygon blocks over WebSocket.

## Key Modules

- `src/aave/` contains Aave config, watchlist updates, helpers, and liquidation logic.
- `src/morpho/` contains Morpho config, market math, watchlist updates, and liquidation logic.
- `src/compound/` contains Compound/Comet watchlist updates and collateral purchase logic.
- `src/bootstrap_engine/` performs initial watchlist bootstrapping and stores bootstrap progress.
- `src/common/` contains shared traits, contract factories, ParaSwap integration, simulation, task management, and database record types.
- `src/profit_distributor.rs` handles scheduled profit distribution and gas refueling.
- `src/liq_data_extractor.rs` listens for flash liquidator events and writes history records.
- `src/constants.rs` contains chain settings, contract addresses, token allowlists, intervals, and environment loading.

## Requirements

- Rust stable.
- Polygon RPC endpoints with both WebSocket and HTTP access.
- A funded keeper private key.
- Anvil available on `PATH` for transaction simulation.
- Docker and Docker Compose for containerized deployment.

## Environment

Create a local `.env` file for development:

```env
PRIVATE_KEY=your_private_key
RPC_URL=wss://your-polygon-websocket-rpc
RPC_URL_HTTP=https://your-polygon-http-rpc
RUST_LOG=info,liq_ranger=debug
DATABASE_URL=sqlite://./data/history.db
```

`DATABASE_URL` is optional. If omitted, the bot uses `sqlite://./data/history.db`.

## Build And Run

```bash
cargo build --release
cargo run --release --bin liq-ranger
```

Useful checks:

```bash
cargo fmt
cargo clippy
cargo test
```

## Docker

The production image is built by `Dockerfile` and runs the `liq-ranger` binary as a non-root user.

```bash
docker compose up --build
```

`docker-compose.yml` runs the bot and Grafana. The shared `./data` volume stores sled state and SQLite history.

## Persistence

- `./data/sled_db` stores protocol watchlists and bootstrap state.
- `./data/history.db` stores liquidation and profit-distribution history.
- `db/` contains SQLx migrations for SQLite.

## Deployment

`.github/workflows/prod.yml` builds and pushes the Docker image on `main`, copies deployment files to the VPS, and runs `deploy.sh`. Grafana is provisioned from `provisioning/`.

## Suggested Reading Order

1. `src/main.rs`
2. `src/lib.rs`
3. `src/constants.rs`
4. `src/common/mod.rs`
5. `src/liquidation_executor.rs`
6. `src/block_watcher.rs`
7. The protocol module you plan to change: `src/aave/`, `src/morpho/`, or `src/compound/`
8. `src/bootstrap_engine/` if changing watchlist population
9. `src/common/simulation_sandbox.rs` and `src/common/paraswap.rs` before changing execution economics
