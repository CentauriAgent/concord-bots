# =============================================================================
# Dockerfile — concord-bots (Vector/Concord Protocol bot)
# =============================================================================
# Multi-stage build:
#   1. builder  — compiles `cargo build --release` (toolchain pinned below)
#   2. runtime  — debian-slim with only the release binary + ca-certificates
#
# Build:
#   docker build -t concord-bots .
#
# Run:
#   docker run -d --name my-bot \
#     -v $(pwd)/config:/app/config \
#     -v concord-data:/app/data \
#     concord-bots
#
# The bot reads its config from $BOT_CONFIG (default: /app/config/bot.toml),
# so mount your config dir — never bake bot.toml (it holds your nsec) into
# the image. See config/bot.toml.example for a template.
# =============================================================================

# ---- Stage 1: build ---------------------------------------------------------
FROM rust:1.85-slim AS builder

WORKDIR /build

# Manifests first — dependency layer caching.
COPY Cargo.toml Cargo.lock ./

# Dummy main so `cargo build` caches deps before we copy real sources.
RUN mkdir src && echo "fn main() {}" > src/main.rs \
 && cargo build --release || true

# Real sources.
COPY src/ src/
RUN touch src/main.rs && cargo build --release

# ---- Stage 2: runtime -------------------------------------------------------
FROM debian:bookworm-slim

RUN apt-get update \
 && apt-get install -y --no-install-recommends ca-certificates \
 && rm -rf /var/lib/apt/lists/* \
 && useradd -m -s /bin/bash concord

WORKDIR /app

COPY --from=builder /build/target/release/concord-bots /usr/local/bin/concord-bots

# /app/data holds identity/wallet persistence between runs (mounted volume).
RUN mkdir -p /app/data && chown -R concord:concord /app

USER concord

ENV RUST_LOG=info \
    BOT_CONFIG=/app/config/bot.toml

ENTRYPOINT ["concord-bots"]
