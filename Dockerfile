# --- Stage 1: Chef Base (using Debian to match Runtime) ---
FROM lukemathwalker/cargo-chef:latest-rust-1.85-bookworm AS chef
WORKDIR /app

# Install only essential build dependencies (pkg-config, libssl-dev)
RUN apt-get update && apt-get install -y --no-install-recommends \
pkg-config \
libssl-dev \
&& rm -rf /var/lib/apt/lists/*

# --- Stage 2: Planner ---
FROM chef AS planner
COPY . .
RUN cargo chef prepare --recipe-path recipe.json

# --- Stage 3: Builder ---
FROM chef AS builder
COPY --from=planner /app/recipe.json recipe.json
# Build dependencies with --locked for reproducible builds
RUN cargo chef cook --release --locked --recipe-path recipe.json

COPY . .
# Build the specific binary
RUN cargo build --release --locked --bin liq-ranger

# --- Stage 4: Minimal Runtime ---
FROM debian:bookworm-slim AS runtime

# Install runtime dependencies: CA certs, OpenSSL, and tini (init process)
RUN apt-get update && apt-get install -y --no-install-recommends \
ca-certificates \
libssl3 \
tini \
&& rm -rf /var/lib/apt/lists/*

# Create a non‑root user with a home directory
RUN useradd -m -s /bin/bash scavenger

# Use a writable workdir owned by the non‑root user
WORKDIR /home/scavenger/app
COPY --from=builder /app/target/release/liq-ranger /usr/local/bin/liq-ranger
RUN chown scavenger:scavenger /home/scavenger/app

# Switch to non‑root user
USER scavenger

# Use tini as the init process to handle signals and reap zombies
ENTRYPOINT ["/usr/bin/tini", "--", "/usr/local/bin/liq-ranger"]
