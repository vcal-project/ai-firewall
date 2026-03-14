# ---------- planner ----------
FROM rust:1.92.0-bookworm AS planner

WORKDIR /app

RUN cargo install cargo-chef --version 0.1.73 --locked

COPY Cargo.toml Cargo.lock ./
COPY src ./src

RUN cargo chef prepare --recipe-path recipe.json

# ---------- builder ----------
FROM rust:1.92.0-bookworm AS builder

WORKDIR /app

COPY --from=planner /usr/local/cargo/bin/cargo-chef /usr/local/cargo/bin/cargo-chef
COPY --from=planner /app/recipe.json recipe.json

RUN cargo chef cook --release --recipe-path recipe.json

COPY Cargo.toml Cargo.lock ./
COPY src ./src

RUN cargo build --release --locked

# ---------- runtime ----------
FROM debian:12.13-slim AS runtime

WORKDIR /app

RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates curl \
    && rm -rf /var/lib/apt/lists/* \
    && groupadd -r aif \
    && useradd -r -g aif -u 10001 aif

COPY --from=builder --chown=10001:10001 /app/target/release/ai-firewall /usr/local/bin/ai-firewall

USER 10001:10001

EXPOSE 8080

HEALTHCHECK --interval=30s --timeout=3s --start-period=10s --retries=3 \
  CMD curl -fsS http://127.0.0.1:8080/healthz || exit 1

ENTRYPOINT ["/usr/local/bin/ai-firewall"]