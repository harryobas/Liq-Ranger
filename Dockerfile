# --- Stage 1: Chef Base ---
FROM lukemathwalker/cargo-chef:latest-rust-1.85-bookworm AS chef
WORKDIR /app

# Install C/C++ compilation tools required by revm, secp256k1, and OpenSSL bindings
RUN apt-get update && apt-get install -y --no-install-recommends \
    build-essential \
    clang \
    cmake \
    pkg-config \
    libssl-dev \
    && rm -rf /var/lib/apt/lists/*

# Print full backtraces if rustc panics
ENV RUST_BACKTRACE=1

# --- Stage 2: Planner ---
FROM chef AS planner
COPY . .
RUN cargo chef prepare --recipe-path recipe.json

# --- Stage 3: Builder ---
FROM chef AS builder
COPY --from=planner /app/recipe.json recipe.json

# Prevent OOM kills by capping parallel compiler threads (adjust based on machine memory)
ENV CARGO_BUILD_JOBS=2

# Build dependencies with --locked for reproducible builds
RUN cargo chef cook --release --locked --recipe-path recipe.json

COPY . .
# Build the specific binary
RUN cargo build --release --locked --bin liq-ranger

# --- Stage 4: Minimal Runtime ---
FROM debian:bookworm-slim AS runtime

# Install runtime dependencies
RUN apt-get update && apt-get install -y --no-install-recommends \
    ca-certificates \
    libssl3 \
    tini \
    && rm -rf /var/lib/apt/lists/*

# Create a non-root user
RUN useradd -m -s /bin/bash scavenger

WORKDIR /home/scavenger/app
COPY --from=builder /app/target/release/liq-ranger /usr/local/bin/liq-ranger
RUN chown scavenger:scavenger /home/scavenger/app

USER scavenger

ENTRYPOINT ["/usr/bin/tini", "--", "/usr/local/bin/liq-ranger"]