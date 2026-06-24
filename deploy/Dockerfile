# =============================================================================
# Dockerfile — Docker deployment for concord-bots
# =============================================================================
#
# Build:
#   docker build -t concord-bots .
#
# Run:
#   docker run -d \
#     --name my-bot \
#     -v $(pwd)/config:/app/config \
#     -e NSEC=nsec1... \
#     concord-bots
#
# The config directory is mounted so you can edit bot.toml without rebuilding.

FROM rust:1.85-slim AS builder

WORKDIR /build

# Copy manifests first for better layer caching
COPY Cargo.toml Cargo.lock ./

# Create a dummy main.rs so dependencies can be cached
RUN mkdir src && echo "fn main() {}" > src/main.rs
RUN cargo build --release || true

# Now copy the real source and build
COPY src/ src/
RUN touch src/main.rs && cargo build --release

# -----------------------------------------------------------------------------
# Runtime image
# -----------------------------------------------------------------------------
FROM debian:bookworm-slim

RUN apt-get update && apt-get install -y \
    ca-certificates \
    && rm -rf /var/lib/apt/lists/*

# Create a non-root user
RUN useradd -m -s /bin/bash concord

WORKDIR /app

# Copy the binary
COPY --from=builder /build/target/release/concord-bots /usr/local/bin/concord-bots

# Copy default config
COPY config/ /app/config/

# Create data directory for identity persistence
RUN mkdir -p /app/data && chown -R concord:concord /app

USER concord

# Environment defaults
ENV RUST_LOG=info
ENV BOT_CONFIG=/app/config/bot.toml

CMD ["concord-bots"]
