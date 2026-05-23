
# Operational Behavior

This document describes how AI Cost Firewall behaves during startup, runtime operation, graceful shutdown, configuration reloads, semantic cache activity, and operational troubleshooting.

AI Cost Firewall is designed to behave predictably in production environments and pilot deployments.

---

# Runtime Overview

AI Cost Firewall provides:

- exact cache and semantic cache orchestration
- explicit readiness and liveness behavior
- graceful shutdown with request draining
- nginx-style hot reload using `SIGHUP`
- startup dependency validation
- OpenAI-compatible provider diagnostics
- Prometheus metrics and Grafana dashboards
- semantic cache lifecycle control
- upstream and embedding timeout visibility
- structured runtime error classification

---

# Runtime Architecture

```text
Client Applications
        │
        ▼
AI Cost Firewall
        │
        ├── Redis (exact cache)
        ├── Qdrant (semantic cache)
        │
        ▼
OpenAI-compatible provider
```

---

# Startup Behavior

Runtime dependencies are initialized during normal startup.

## Required Dependencies

Redis is always required.

Example:

```conf
redis_url redis://redis:6379;
```

When semantic cache is enabled:

```conf
semantic_cache_enabled true;
```

Qdrant must also be reachable.

Example:

```conf
qdrant_url http://qdrant:6334;
```

---

# Startup Validation

During startup, AI Cost Firewall validates:

- Redis connectivity
- Qdrant connectivity
- semantic cache configuration completeness
- embedding configuration
- Qdrant collection vector size compatibility
- provider configuration
- request-size configuration
- model validation settings

Startup intentionally fails fast on invalid configuration to avoid silent runtime errors.

---

# Vector Size Validation

If the configured Qdrant collection already exists, AI Cost Firewall validates:

```conf
qdrant_vector_size 1536;
```

against the actual Qdrant collection vector size.

Example startup error:

```text
Qdrant collection 'aif_semantic_cache' has vector size 768, but config requires 1536
```

---

# semantic_cache_fail_open Behavior

`semantic_cache_fail_open` affects runtime semantic lookup behavior only.

Example:

```conf
semantic_cache_fail_open true;
```

When enabled:

- runtime semantic lookup failures are treated as cache skips
- requests continue upstream normally

It does not bypass startup dependency validation.

---

# Startup Behavior Summary

| Condition | Behavior |
|---|---|
| Redis unavailable | startup fails |
| Semantic cache enabled and Qdrant unavailable | startup fails |
| Vector size mismatch | startup fails |
| Runtime semantic lookup failure + fail-open enabled | continue upstream |
| Runtime semantic lookup failure + fail-open disabled | request fails |

---

# Health & Readiness Endpoints

AI Cost Firewall exposes two operational endpoints.

---

## `/healthz` — Liveness

Indicates whether the process itself is alive.

Example:

```bash
curl http://localhost:8080/healthz
```

Behavior:

- returns `200 OK` while the process is running
- does not depend on downstream providers
- suitable for container liveness probes

---

## `/readyz` — Readiness

Indicates whether the firewall is currently ready to serve traffic.

Example:

```bash
curl http://localhost:8080/readyz
```

Behavior:

- returns `200 OK` during normal operation
- returns `503 Service Unavailable` during graceful shutdown
- suitable for load balancers and orchestration systems

---

# Readiness Summary

| State | `/healthz` | `/readyz` |
|---|---:|---:|
| Normal operation | 200 | 200 |
| Graceful shutdown | 200 | 503 |
| Process stopped | unavailable | unavailable |

---

# Graceful Shutdown

AI Cost Firewall supports graceful shutdown using:

- `SIGTERM`
- `SIGINT`

Useful for:

- Docker
- Docker Compose
- Kubernetes
- systemd
- cloud orchestrators

---

# Shutdown Sequence

1. Shutdown signal received
2. Readiness disabled
3. `/readyz` begins returning `503`
4. New requests rejected
5. In-flight requests continue
6. Process exits after timeout

Example:

```conf
graceful_shutdown_timeout_seconds 10;
```

---

# Shutdown Response

Requests received during shutdown return:

```json
{
  "error": {
    "code": 503,
    "message": "server is shutting down",
    "type": "service_unavailable"
  }
}
```

---

# Shutdown Metrics

```text
aif_inflight_requests
aif_shutdown_in_progress
aif_shutdown_rejections_total
```

---

# Configuration Reload

AI Cost Firewall supports nginx-style configuration reload using `SIGHUP`.

Reload behavior:

1. reload configuration file
2. validate configuration
3. rebuild runtime dependencies
4. atomically swap runtime state
5. continue serving traffic

---

# Reload Commands

## Docker Compose

```bash
docker compose kill -s HUP firewall
```

---

## Binary Deployment

```bash
kill -HUP $(pgrep ai-firewall)
```

---

# Expected Reload Logs

```text
received SIGHUP, reloading config
config and runtime successfully reloaded
```

---

# Important Reload Notes

- reload is not automatic file watching
- invalid reload configuration does not replace the active runtime
- traffic continues during successful reload
- runtime dependencies are revalidated during reload

---

# Static Configuration Validation

Validate configuration without starting the firewall.

## Docker Compose

```bash
docker compose run --rm firewall \
  --config /configs/ai-firewall.conf \
  --test-config
```

---

## Binary Deployment

```bash
cargo run -- --config configs/ai-firewall.conf --test-config
```

Expected output:

```text
configuration OK
```

---

# What --test-config Validates

Static validation checks:

- syntax
- required directives
- configuration structure
- semantic cache completeness
- request-size parsing
- model validation settings

It does not contact:

- Redis
- Qdrant
- embedding providers
- upstream LLM providers

---

# Print Loaded Configuration

Inspect the resolved runtime configuration:

```bash
docker compose run --rm firewall \
  --config /configs/ai-firewall.conf \
  --print-config
```

Secrets are automatically masked.

Example:

```text
upstream_api_key = sk-y...-key
embedding_api_key = sk-y...-key
```

---

# Upstream Timeout Behavior

Requests to upstream providers are limited by:

```conf
request_timeout_seconds 120;
```

When exceeded:

- upstream request is aborted
- request classified as `upstream_timeout`
- timeout metrics incremented

---

# Timeout Metrics

```text
aif_upstream_timeouts_total
aif_upstream_request_duration_seconds
```

Embedding provider timeout metrics:

```text
aif_embedding_timeouts_total
aif_embedding_request_duration_seconds
```

---

# OpenAI-Compatible Provider Diagnostics

AI Cost Firewall classifies common provider failures explicitly.

Common runtime classes:

```text
upstream_authentication_error
upstream_not_found
upstream_rate_limited
upstream_timeout
upstream_tls_error
upstream_dns_error
upstream_connect_error
```

---

# Common Provider Causes

| Error Class | Typical Cause |
|---|---|
| `upstream_authentication_error` | Invalid API key |
| `upstream_not_found` | Wrong base URL |
| `upstream_rate_limited` | Provider quota exceeded |
| `upstream_timeout` | Slow provider |
| `upstream_tls_error` | Certificate validation issue |
| `upstream_dns_error` | Hostname resolution failure |
| `upstream_connect_error` | Provider unreachable |

---

# Local Provider Authentication

Local providers often do not require real API keys.

Accepted placeholders:

```text
dummy
none
null
-
```

Example:

```conf
upstream_api_key dummy;
embedding_api_key dummy;
```

---

# Semantic Cache Runtime Flow

Semantic cache lookup flow:

1. normalize request
2. generate embedding
3. query Qdrant
4. filter expired entries
5. evaluate similarity threshold
6. return reusable cached response
7. fallback upstream on miss

---

# Semantic Candidate Requirements

A semantic candidate is reusable only if:

```text
similarity_score >= semantic_similarity_threshold
AND
expires_at > now
AND
cached response payload is valid
```

---

# Semantic Cache Lifecycle

Semantic cache entries include:

- `inserted_at`
- `expires_at`

Expiration derives from:

```conf
semantic_cache_retention_seconds 604800;
```

Expired entries:

- are skipped during lookup
- are not reused
- may remain stored until pruned

Semantic correctness does not depend on cleanup.

---

# Semantic Cache Cleanup

Prune expired semantic entries:

## Docker Compose

```bash
docker compose run --rm firewall \
  --config /configs/ai-firewall.conf \
  --prune-expired-semantic-cache
```

---

## Binary Deployment

```bash
ai-firewall \
  --config configs/ai-firewall.conf \
  --prune-expired-semantic-cache
```

The command removes entries where:

```text
expires_at <= now
```

---

# Qdrant Verification

Inspect collection count:

```bash
curl -s http://127.0.0.1:6333/collections/aif_semantic_cache/points/count \
  -H "Content-Type: application/json" \
  -d '{"exact": true}'
```

Notes:

- AI Cost Firewall uses Qdrant gRPC on `6334`
- manual inspection typically uses REST API on `6333`

---

# Metrics

Metrics endpoint:

```text
http://localhost:8080/metrics
```

---

# Core Runtime Metrics

```text
aif_inflight_requests
aif_shutdown_in_progress
aif_shutdown_rejections_total
aif_readiness_state
```

---

# Cache Metrics

```text
aif_cache_exact_hits
aif_cache_semantic_hits
aif_cache_misses
```

---

# Upstream Metrics

```text
aif_upstream_request_duration_seconds
aif_upstream_timeouts_total
aif_upstream_calls_total
```

---

# Semantic Diagnostics Metrics

```text
aif_semantic_candidates_checked_total
aif_semantic_threshold_results_total
aif_semantic_expired_entries_skipped_total
aif_semantic_lookup_duration_seconds
aif_semantic_store_total
aif_semantic_store_errors_total
```

Useful for tuning:

- semantic thresholds
- retention windows
- embedding performance
- Qdrant behavior

---

# Cost Metrics

```text
aif_model_cost_micro_usd_total
aif_gross_saved_micro_usd_total
aif_embedding_overhead_micro_usd_total
aif_net_saved_micro_usd_total
```

These metrics help distinguish:

- gross chat savings
- embedding overhead
- net savings

---

# Logging

AI Cost Firewall logs to stdout/stderr by default.

---

# Docker Compose Logs

```bash
docker compose logs -f firewall
```

Save logs:

```bash
docker compose logs firewall > firewall.log
```

---

# Binary Logs

```bash
./ai-firewall --config configs/ai-firewall.conf > logs.txt 2>&1
```

Append:

```bash
./ai-firewall --config configs/ai-firewall.conf >> logs.txt 2>&1
```

View and save simultaneously:

```bash
./ai-firewall --config configs/ai-firewall.conf 2>&1 | tee logs.txt
```

---

# systemd Logs

```bash
journalctl -u ai-firewall -f
```

Export:

```bash
journalctl -u ai-firewall > logs.txt
```

---

# Maintenance Recommendations

Recommended operational tasks:

- review semantic retention windows
- monitor Qdrant collection growth
- periodically prune expired semantic entries
- inspect Grafana dashboards
- monitor timeout metrics
- verify provider latency
- review cache hit rates

---

# Operational Troubleshooting

---

# Low Cache Hit Rate

Inspect:

```text
aif_cache_exact_hits
aif_cache_semantic_hits
aif_cache_misses
```

Possible causes:

- threshold too high
- prompts insufficiently similar
- semantic cache disabled
- entries expired too quickly

---

# Frequent Upstream Timeouts

Inspect:

```text
aif_upstream_timeouts_total
```

Possible actions:

- increase timeout
- reduce prompt size
- use faster models
- inspect provider latency

---

# Frequent Embedding Failures

Inspect:

```text
aif_embedding_timeouts_total
aif_embedding_request_duration_seconds
```

Possible causes:

- embedding provider unavailable
- wrong embedding URL
- local embedding model missing
- embedding provider overloaded

---

# Validation Errors

Inspect:

```text
aif_errors_total{class="validation_error"}
```

Possible causes:

- unsupported model
- malformed JSON
- request too large
- invalid request format

---

# Qdrant Startup Failures

Verify:

```conf
qdrant_url http://qdrant:6334;
```

Docker Compose service names:

```conf
redis_url redis://redis:6379;
qdrant_url http://qdrant:6334;
```

Local binary example:

```conf
redis_url redis://127.0.0.1:6379;
qdrant_url http://127.0.0.1:6334;
```

---

# Wrong Upstream Base URL

Correct:

```conf
upstream_base_url http://ollama:11434/v1;
```

Wrong:

```conf
upstream_base_url http://ollama:11434/v1/chat/completions;
```

AI Cost Firewall automatically appends OpenAI-compatible endpoint paths.

---

# Additional Documentation

See also:

- `docs/quickstart.md`
- `docs/config-reference.md`
- `docs/provider-compatibility.md`
- `docs/troubleshooting.md`
- `docs/metrics-and-costs.md`

---

# Operational Notes

- reload happens only via `SIGHUP`
- `--test-config` performs static validation only
- runtime dependencies initialize during startup and reload
- Redis is always required
- Qdrant required only when semantic cache enabled
- `semantic_cache_fail_open` affects runtime lookup behavior only
- upstream requests exceeding timeout are aborted
