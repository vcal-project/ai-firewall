# AI Cost Firewall

![Rust](https://img.shields.io/badge/Rust-1.75+-orange)
![License](https://img.shields.io/github/license/vcal-project/ai-firewall)
![Docker](https://img.shields.io/badge/docker-ready-blue)
![Status](https://img.shields.io/badge/status-MVP-green)

**OpenAI-compatible gateway for caching and cost control.**

AI Cost Firewall is a lightweight OpenAI-compatible API gateway that reduces LLM API costs and latency by caching responses using exact
matching and semantic similarity.

It sits between applications and LLM providers and forwards only necessary requests to the upstream API.

The project is developed and supported by the creators of VCAL Server.

https://vcal-project.com

---

# Why AI Cost Firewall?

LLM APIs are expensive and often receive repeated or semantically similar prompts.

Without caching, every request results in:

-   unnecessary API calls
-   increased token usage
-   higher costs
-   additional latency

AI Cost Firewall solves this by introducing a two-layer cache:

1.  Exact cache (Redis) -- instant responses for identical prompts\
2.  Semantic cache (Qdrant) -- reuse answers for similar prompts

Only cache misses are forwarded to the upstream LLM provider.

The firewall behaves similarly to "nginx for LLM APIs".

---

## Example: Cost Savings with Exact + Semantic Caching

**cache hit rate • net savings after embedding overhead • real-time cost reduction**

[![AI Cost Firewall Grafana Dashboard](assets/grafana/dashboard2.png)](assets/grafana/dashboard2.png)

*Local synthetic workload simulating enterprise support queries (VPN, onboarding, access requests).  
Demonstrates real-time cost reduction using exact and semantic caching, with full cost breakdown (gross savings, embedding cost, and net savings).*

---

# Key Features

- OpenAI-compatible `/v1/chat/completions` endpoint
- Exact request caching (Redis)
- Semantic cache (Qdrant)
- Token and cost savings metrics
- Prometheus observability (cost, cache, errors, runtime behavior)
- Error classification (validation / upstream / timeout / internal)
- Upstream latency and timeout tracking
- Semantic cache diagnostics (threshold, candidates, expiration behavior)
- Docker deployment
- nginx-style configuration
- Strict startup validation with clear error messages
- Hot configuration reload (`SIGHUP`)
- Graceful shutdown with request draining (SIGTERM / SIGINT)
- Readiness and liveness endpoints (`/readyz`, `/healthz`)
- Request size protection (`max_request_body_bytes`)
- Lightweight Rust + Axum implementation

AI Cost Firewall is designed to be safe by default preventing accidental misconfiguration and unintended upstream costs.

---

# What’s new in v0.1.4

v0.1.4 focuses on **operational predictability and observability** in real deployments.

### Key improvements

- Clear error classification (validation / upstream / timeout / internal)
- Upstream timeout visibility and latency tracking
- Graceful shutdown with request draining and rejection tracking
- Readiness vs liveness separation (`/readyz`, `/healthz`)
- Semantic cache diagnostics:
  - candidates checked
  - threshold pass/fail
  - expired entries skipped
  - lookup latency
- Improved logging for cache decisions (hit / miss / semantic reuse)
- Safer configuration with better validation and warnings

The system now behaves predictably under load and is easier to debug in production.

---

# Architecture Overview

Client applications send requests to the firewall instead of directly to the LLM provider.

[![AI Cost Firewall Architecture Diagram](assets/architecture/ai-cost-firewall-diagram.png)](assets/architecture/ai-cost-firewall-diagram.png)

Full architecture documentation:

[docs/architecture.md](docs/architecture.md)

---

# Quick Start (Docker)

The fastest way to try AI Cost Firewall is using Docker Compose.

## Prerequisites

Install:

- Docker
- Docker Compose (included with Docker Desktop)

Verify installation:

```bash
docker --version
docker compose version
```

## Clone the repository

Clone the repository and prepare the configuration:

```bash
git clone https://github.com/vcal-project/ai-firewall.git
cd ai-firewall
cp configs/ai-firewall.conf.example configs/ai-firewall.conf
```

Edit the configuration file and add your API keys:

```bash
nano configs/ai-firewall.conf
```

You should also specify the exact model names returned by your LLM provider (used for cost calculation), for example:

```text
gpt-4o-mini-2024-07-18
```

> The repository already includes all required Prometheus and Grafana configuration 

## Start the stack

This will start the full stack (Firewall, Redis, Qdrant, Prometheus, Grafana):

```bash
docker compose pull
docker compose up -d
```

## View logs

```bash
docker compose logs -f firewall
```

## Services

| Service | URL |
|-------|------|
| Firewall API | http://localhost:8080 |
| Prometheus | http://localhost:9090 |
| Grafana | http://localhost:3000 |

The stack includes:

-   AI Cost Firewall
-   Redis
-   Qdrant
-   Prometheus
-   Grafana

---

# Example Request

``` bash
curl http://localhost:8080/v1/chat/completions \
  -H "Authorization: Bearer <your-key>" \
  -H "Content-Type: application/json" \
  -d '{
    "model": "gpt-4o-mini-2024-07-18",
    "messages": [
      {"role": "user", "content": "Explain Redis briefly."}
    ]
  }'
```

---

# Configuration

AI Cost Firewall uses a simple nginx-style configuration format.

- Signal-driven operations (SIGHUP reload, SIGTERM graceful shutdown)

Example configuration:

``` text
listen_addr 0.0.0.0:8080;

redis_url redis://redis:6379;

upstream_base_url https://api.openai.com;
upstream_api_key sk-your-api-key;

embedding_base_url https://api.openai.com;
embedding_api_key sk-your-api-key;
embedding_model text-embedding-3-small;

qdrant_url http://qdrant:6334;
qdrant_collection aif_semantic_cache;
qdrant_vector_size 1536;

cache_ttl_seconds 2592000;
request_timeout_seconds 120;
graceful_shutdown_timeout_seconds 10;  # default
max_request_body_bytes 1M;

semantic_cache_enabled true;
semantic_similarity_threshold 0.92;

# Model validation behavior
# By default, only models defined via `model_price` are allowed.
# Unknown models will be rejected with 400.
allow_unknown_models_pass_through false;

# Chat-completion pricing (USD per 1M tokens)
# model_price <model> <input_usd_per_1m_tokens> <output_usd_per_1m_tokens>;

model_price gpt-4o-mini-2024-07-18 0.15 0.60;
model_price gpt-4.1-mini-2025-04-14 0.30 1.20;

# Embedding pricing (optional, used for net cost estimation only)
embedding_price 0.020;
```

> If the API returns `gpt-4o-mini-2024-07-18`, the same name must appear in the configuration.

Misconfiguration is one of the most common causes of unexpected LLM costs. AI Cost Firewall prevents this at startup.

## Startup Validation & Error Handling

AI Cost Firewall performs strict validation at startup.

### Example errors

```text
configuration error: semantic_cache_enabled=true requires: embedding_api_key, embedding_model, qdrant_url
```

```text
configuration error: no allowed models configured: add at least one model_price or set allow_unknown_models_pass_through=true
```

```text
configuration error: invalid AIF_MAX_REQUEST_BODY_BYTES value 'abc'. Use formats like 1024, 512K, 1M, 2M
```

### Behavior

- Multiple issues reported in a single error
- Invalid configs fail fast
- Prevents unintended upstream usage

## Model validation

AI Cost Firewall validates the `model` field before forwarding requests upstream.

- Only models defined via `model_price` are considered supported
- Requests with unknown models are rejected with 400 Bad Request
- This prevents accidental or unauthorized upstream usage

Example:

```bash
{
  "error": {
    "code": 400,
    "message": "Unsupported model: gpt-unknown",
    "type": "validation_error"
  }
}
```

## Optional: allow pass-through

If you want the gateway to behave like a transparent proxy:

```bash
allow_unknown_models_pass_through true;
```

In this mode:

- Unknown models are forwarded upstream
- Cost tracking will not be applied for unknown models
- Validation is relaxed

### Cache behavior and TTL

`cache_ttl_seconds` defines how long cached responses remain valid.

- Exact cache (Redis): TTL is enforced automatically by Redis
- Semantic cache (Qdrant): entries are not physically deleted, but filtered at query time based on expiration

This ensures consistent behavior across both caching layers.

v0.1.4 adds visibility into semantic cache lifecycle:

- how many candidates are evaluated
- how many fail similarity threshold
- how many are skipped due to expiration

This helps diagnose low semantic hit rates and tune thresholds effectively.

> Semantic cache entries are not automatically deleted from Qdrant. Expired entries are ignored during lookup, but remain stored in the collection. To reclaim disk space, old entries can be removed manually (for example, with a periodic cleanup script or scheduled job). Automatic cleanup support may be added in future versions.

---

## Request size limits

`max_request_body_bytes` defines the maximum request size.

Supported formats:

```text
1024
512K
1M
2M
```

Requests exceeding the limit are rejected early:

```json
{
  "error": {
    "code": 413,
    "type": "validation_error",
    "message": "request body exceeds max_request_body_bytes limit"
  }
}
```

Very small values (<1K) trigger a startup warning.

## Semantic cache requirements

When enabled:

```conf
semantic_cache_enabled true;
```

Required fields:

- embedding_base_url
- embedding_api_key
- embedding_model
- qdrant_url
- qdrant_collection
- qdrant_vector_size

---

## Environment Variables

If no configuration file is provided, AI Cost Firewall falls back to environment variables.

For convenience, you can use a `.env` file in development:

```conf
AIF_REDIS_URL=redis://127.0.0.1:6379
AIF_UPSTREAM_API_KEY=sk-xxxx
AIF_EMBEDDING_MODEL=text-embedding-3-small
AIF_EMBEDDING_PRICE_USD_PER_1M_TOKENS=0.020
```

- Variables follow the AIF_ prefix convention
- `.env` is loaded automatically if present
- Intended for development and simple deployments

If neither a config file nor required environment variables are provided, the application will fail to start with a clear configuration error.

Example errors:

```text
configuration error: AIF_REDIS_URL is required when no config file is used
```

```text
configuration error: invalid AIF_QDRANT_VECTOR_SIZE value 'abc'
```

Full configuration reference:

[docs/config-reference.md](docs/config-reference.md)

---

## Operational Behavior

AI Cost Firewall is designed to behave predictably in production environments.

### Graceful shutdown

- Stops accepting new requests
- Allows in-flight requests to complete
- Rejects new requests with 503 during shutdown
- Tracks shutdown state and rejection count

### Readiness vs liveness

- `/healthz` — process is alive
- `/readyz` — ready to serve traffic

During shutdown:

- `/healthz` → OK
- `/readyz` → 503

### Timeout handling

- Upstream requests are bounded by `request_timeout_seconds`
- Timeouts are explicitly tracked and classified

---

## Metrics

Prometheus metrics are available at:

http://localhost:8080/metrics

Example metrics:

```text
aif_requests_total
aif_cache_exact_hits
aif_cache_semantic_hits
aif_cache_misses
aif_tokens_saved
aif_cost_saved_micro_usd
aif_inflight_requests
aif_shutdown_in_progress
aif_shutdown_rejections_total
```

### Note

Token and cost savings are calculated for:

```text
/v1/chat/completions
```

For semantic cache hits:

- Gross savings are based on avoided chat-completion tokens
- Embedding lookup costs are included and deducted
- Reported savings represent net savings

Metrics:

- `aif_chat_cost_saved_micro_usd` – gross chat-completion savings
- `aif_embedding_cost_micro_usd` – embedding lookup cost
- `aif_cost_saved_micro_usd` – net savings (gross − embedding cost)
- `aif_errors_total{class=...}` – classified errors
- `aif_upstream_timeouts_total` – upstream timeout count
- `aif_upstream_request_duration_seconds` – upstream latency
- `aif_readiness_state` – readiness (1/0)
- `aif_shutdown_in_progress` – shutdown state
- `aif_semantic_candidates_checked_total`
- `aif_semantic_threshold_results_total{result="pass|fail"}`
- `aif_semantic_expired_entries_skipped_total`
- `aif_semantic_lookup_duration_seconds`

Exact cache hits have no embedding cost.

If embedding_price is not configured, embedding cost is treated as 0 and savings may be overestimated.

---

# Build from Source

Clone the repository if you want to:

- explore the code
- modify configuration templates
- build the firewall locally
- contribute to the project

```bash
git clone https://github.com/vcal-project/ai-firewall.git
cd ai-firewall
```

Build the project:

```bash
cargo build --release
```

Run the firewall:

```bash
cargo run --release
```

---

## Testing

AI Cost Firewall includes unit tests for configuration parsing, validation, and core request handling paths.

Key areas covered:
- Config validation (required fields, limits, semantic cache requirements)
- Byte-size parsing (`1M`, `2M`, etc.) for request limits
- Negative configuration tests (invalid values, missing fields, invalid sizes)
- Aggregated validation error tests (multiple misconfigurations reported together)
- Environment variable validation (invalid formats, missing required variables)
- Cost accounting correctness (chat vs embedding vs net)

Run tests locally:

```bash
cargo test
```

---

## Troubleshooting & Debugging

If cache performance is lower than expected:

1. Check semantic threshold:
   - High threshold → fewer semantic hits

2. Inspect diagnostics dashboard:
   - High `threshold_fail` → threshold too strict
   - High `expired_entries_skipped` → TTL too short

3. Check upstream latency:
   - Increasing latency may indicate provider issues

4. Check error classification:
   - `validation_error` → request issues
   - `upstream_timeout` → provider slow
   - `internal_error` → system issue

---

# Documentation

| Document | Description |
|---------|-------------|
| `docs/architecture.md` | System architecture |
| `docs/config-reference.md` | Configuration directives |
| `docs/faq.md` | Frequently asked questions |
| `docs/how-it-works.md` | Request flow and caching logic |
| `docs/quickstart.md` | Full setup guide |
| `docs/operation.md` | Runtime behavior (health checks, shutdown, reload) |

---

# Contributing

Contributions are welcome.

If you would like to contribute to AI Cost Firewall — whether through bug reports, feature suggestions, documentation improvements, or code —
please see:

[CONTRIBUTING.md](CONTRIBUTING.md)

Before submitting a pull request, please open an issue to discuss the change.

We welcome improvements in:

- performance
- documentation
- testing
- integrations with LLM providers
- observability and metrics

---

# Integration with VCAL Server

AI Cost Firewall can optionally integrate with **VCAL Server** for
advanced semantic caching and distributed vector storage.

VCAL Server project:

https://vcal-project.com

---

# License

Apache License 2.0
