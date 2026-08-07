# ---------- planner ----------
FROM rust:1.96.0-bookworm AS planner

WORKDIR /app

RUN cargo install cargo-chef --version 0.1.73 --locked

COPY Cargo.toml Cargo.lock ./
COPY src ./src

RUN cargo chef prepare --recipe-path recipe.json

# ---------- builder ----------
FROM rust:1.96.0-bookworm AS builder

WORKDIR /app

COPY --from=planner /usr/local/cargo/bin/cargo-chef /usr/local/cargo/bin/cargo-chef
COPY --from=planner /app/recipe.json recipe.json

RUN cargo chef cook --release --locked --recipe-path recipe.json

COPY Cargo.toml Cargo.lock ./
COPY src ./src

RUN cargo build --release --locked

# ---------- runtime ----------
FROM gcr.io/distroless/cc-debian12:nonroot AS runtime

ARG AIF_VERSION=0.5.0
ARG VCS_REF=unknown
ARG BUILD_DATE=unknown

LABEL org.opencontainers.image.title="AI Cost Firewall" \
      org.opencontainers.image.description="VCAL AI Cost Firewall" \
      org.opencontainers.image.vendor="VCAL" \
      org.opencontainers.image.version="${AIF_VERSION}" \
      org.opencontainers.image.revision="${VCS_REF}" \
      org.opencontainers.image.created="${BUILD_DATE}" \
      org.opencontainers.image.licenses="Apache-2.0"

WORKDIR /app

COPY --from=builder /app/target/release/ai-firewall /usr/local/bin/ai-firewall

USER nonroot:nonroot

EXPOSE 8080

ENTRYPOINT ["/usr/local/bin/ai-firewall"]