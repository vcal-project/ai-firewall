
# AI Cost Firewall — Quick Start

This guide explains how to deploy, validate, and test AI Cost Firewall using either Docker Compose or a local Rust build.

AI Cost Firewall is an OpenAI-compatible gateway that reduces LLM API cost and latency using:

- exact cache (Redis)
- semantic cache (Qdrant)

Only cache misses are forwarded upstream.

---

# Architecture Overview

```text
Client
   │
   ▼
AI Cost Firewall
   │
   ├── Redis (exact cache)
   ├── Qdrant (semantic cache)
   │
   ▼
OpenAI-compatible upstream
```

Supported OpenAI-compatible providers include:

- OpenAI
- Ollama
- LM Studio
- vLLM
- LiteLLM
- OpenRouter

---

# Quick Evaluation Paths

AI Cost Firewall includes ready-to-run deployment examples:

```text
deploy/examples/
```

Recommended starting points:

| Deployment Pattern | Best For |
|---|---|
| `openai-cloud/` | Fastest cloud evaluation |
| `local-ollama/` | Fully local testing |
| `hybrid-openai-local-embeddings/` | OpenAI chat + local embeddings |
| `openrouter/` | OpenRouter evaluation |
| `local-full-stack/` | Full local observability stack |

Each example includes:

- docker-compose deployment
- minimal configuration
- expected metrics
- dashboard support
- example requests

---

# Quickest Start (Docker)

## Prerequisites

Install:

- Docker
- Docker Compose

Verify installation:

```bash
docker --version
docker compose version
```

---

# Clone the Repository

```bash
git clone https://github.com/vcal-project/ai-firewall.git
cd ai-firewall
```

---

# Option 1 — Fastest Evaluation Path

Use OpenAI for chat completions and embeddings.

```bash
cd deploy/examples/openai-cloud
```

Edit:

```text
ai-firewall.conf
```

Replace:

```text
sk-your-openai-key
```

Start the stack:

```bash
docker compose up -d
```

Optional observability stack:

```bash
docker compose \
  -f docker-compose.yml \
  -f docker-compose.observability.yml \
  up -d
```

---

# Option 2 — Fully Local Evaluation

Use Ollama for both chat completions and embeddings.

```bash
cd deploy/examples/local-full-stack
```

Start the stack:

```bash
docker compose up -d
```

Pull local models:

```bash
docker compose exec ollama ollama pull llama3.2:3b
docker compose exec ollama ollama pull nomic-embed-text
```

Restart firewall:

```bash
docker compose restart firewall
```

---

# Services

| Service | URL |
|---|---|
| Firewall API | http://localhost:8080 |
| Prometheus | http://localhost:9090 |
| Grafana | http://localhost:3000 |

Prometheus and Grafana are included automatically in:

```text
deploy/examples/local-full-stack/
```

---

# Validate the Deployment

Check liveness:

```bash
curl http://localhost:8080/healthz
```

Check readiness:

```bash
curl http://localhost:8080/readyz
```

Expected:

```text
OK
READY
```

Check logs:

```bash
docker compose logs -f firewall
```

---

# Example Requests

## OpenAI Example

```bash
curl http://localhost:8080/v1/chat/completions \
  -H "Content-Type: application/json" \
  -d '{
    "model": "gpt-4o-mini-2024-07-18",
    "messages": [
      {"role": "user", "content": "Explain Redis briefly."}
    ]
  }'
```

---

## Ollama Example

```bash
curl http://localhost:8080/v1/chat/completions \
  -H "Content-Type: application/json" \
  -d '{
    "model": "llama3.2:3b",
    "messages": [
      {"role": "user", "content": "Explain Redis briefly."}
    ]
  }'
```

Run the same request twice.

Expected behavior:

- first request → upstream provider
- second request → exact cache hit
- similar requests → possible semantic cache hits

---

# Included Dashboards

AI Cost Firewall includes pre-configured Grafana dashboards.

## Overview Dashboard

Shows:

- exact cache savings
- semantic cache savings
- embedding overhead
- net savings
- request activity

---

## Diagnostics Dashboard

Shows:

- semantic threshold pass/fail behavior
- semantic lookup latency
- cache diagnostics
- runtime semantic behavior

Dashboards are loaded from:

```text
deploy/grafana/dashboards/
```

---

# Build from Source

## Install Rust

```bash
curl https://sh.rustup.rs -sSf | sh
source $HOME/.cargo/env
```

Verify installation:

```bash
rustc --version
cargo --version
```

---

## Redis

Docker example:

```bash
docker run -d -p 6379:6379 redis:8
```

Verify:

```bash
docker exec -it <redis-container> redis-cli ping
```

Expected:

```text
PONG
```

---

## Qdrant

Required only for semantic cache.

Run with Docker:

```bash
docker run -d --rm --name qdrant \
  -p 6333:6333 \
  -p 6334:6334 \
  qdrant/qdrant
```

Verify:

```bash
curl http://127.0.0.1:6333/healthz
```

---

## Build the Firewall

```bash
git clone https://github.com/vcal-project/ai-firewall.git
cd ai-firewall

cargo build --release
```

Binary:

```text
target/release/ai-firewall
```

---

# Configuration Basics

AI Cost Firewall uses nginx-style configuration.

Example:

```text
listen_addr 0.0.0.0:8080;

redis_url redis://redis:6379;

upstream_provider openai_compatible;
upstream_base_url https://api.openai.com;
upstream_api_key sk-your-key;

embedding_provider openai_compatible;
embedding_base_url https://api.openai.com;
embedding_api_key sk-your-key;
embedding_model text-embedding-3-small;

qdrant_url http://qdrant:6334;
qdrant_collection aif_semantic_cache;
qdrant_vector_size 1536;

semantic_cache_enabled true;
semantic_similarity_threshold 0.92;
```

---

# OpenAI-Compatible Provider URLs

Use provider base URLs only.

## Correct

```text
https://api.openai.com
http://ollama:11434/v1
```

## Wrong

```text
http://ollama:11434/v1/chat/completions
```

AI Cost Firewall automatically appends endpoint paths.

---

# Placeholder API Keys for Local Providers

For providers without authentication:

```text
upstream_api_key dummy;
embedding_api_key dummy;
```

Accepted placeholders:

```text
dummy
none
null
-
```

---

# Model Validation

By default:

- only configured models are allowed
- unknown models are rejected

Example:

```text
model_price gpt-4o-mini-2024-07-18 0.15 0.60;
```

Optional pass-through mode:

```text
allow_unknown_models_pass_through true;
```

Useful for:

- OpenRouter
- rapidly changing provider catalogs
- proxy-style deployments

---

# Semantic Cache Lifecycle

Semantic cache entries include:

- inserted_at
- expires_at

Behavior:

- expired entries are skipped during lookup
- expired entries are not automatically deleted
- semantic correctness does not depend on pruning

Prune expired entries:

```bash
docker compose run --rm firewall \
  --config /configs/ai-firewall.conf \
  --prune-expired-semantic-cache
```

---

# Configuration Validation

Validate configuration before startup.

## Docker Compose

```bash
docker compose run --rm firewall \
  --config /configs/ai-firewall.conf \
  --test-config
```

Expected:

```text
configuration OK
```

---

## Print Loaded Configuration

```bash
docker compose run --rm firewall \
  --config /configs/ai-firewall.conf \
  --print-config
```

Secrets are automatically masked.

---

# Runtime Dependency Validation

Runtime dependencies are initialized during normal startup.

Requirements:

- Redis required for exact cache
- Qdrant required when semantic cache is enabled
- vector size must match embedding model dimension

---

# Common Startup Errors

## Missing embedding configuration

```text
configuration error: semantic_cache_enabled=true requires: embedding_model, qdrant_url
```

---

## Invalid request size

```text
configuration error: invalid AIF_MAX_REQUEST_BODY_BYTES value 'abc'
```

---

## Qdrant vector size mismatch

```text
existing collection vector size does not match qdrant_vector_size
```

Typical vector sizes:

| Embedding Model | Vector Size |
|---|---|
| text-embedding-3-small | 1536 |
| nomic-embed-text | 768 |

---

## Wrong upstream URL

## Wrong

```text
upstream_base_url http://ollama:11434/v1/chat/completions;
```

## Correct

```text
upstream_base_url http://ollama:11434/v1;
```

---

# Metrics

Metrics endpoint:

```text
http://localhost:8080/metrics
```

Example metrics:

```text
aif_requests_total
aif_cache_exact_hits
aif_cache_semantic_hits
aif_model_cost_micro_usd_total
aif_net_saved_micro_usd_total
```

Useful semantic diagnostics:

```text
aif_semantic_candidates_checked_total
aif_semantic_threshold_results_total
aif_semantic_lookup_duration_seconds
```

---

# Hot Reload

Reload configuration without restarting.

## Docker Compose

```bash
docker compose kill -s HUP firewall
```

## Binary Deployment

```bash
kill -HUP $(pgrep ai-firewall)
```

Expected logs:

```text
received SIGHUP, reloading config
config and runtime successfully reloaded
```

---

# Logging

AI Cost Firewall logs to stdout/stderr.

View logs:

```bash
docker compose logs -f firewall
```

Save logs:

```bash
docker compose up -d > logs.txt 2>&1
```

---

# Additional Documentation

| Document | Description |
|---|---|
| `docs/config-reference.md` | Full configuration reference |
| `docs/provider-compatibility.md` | Provider compatibility notes |
| `docs/troubleshooting.md` | Common operational issues |
| `docs/operation.md` | Runtime behavior |
| `docs/architecture.md` | System architecture |
| `docs/metrics-and-costs.md` | Cost accounting model |

Full documentation:

```text
https://ai-firewall.docs.vcal-project.com/
```
