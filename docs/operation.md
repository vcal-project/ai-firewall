
# Operational Behavior

This document describes how AI Cost Firewall behaves during startup, runtime operation, graceful shutdown, configuration reloads, semantic cache activity, and operational troubleshooting.

AI Cost Firewall is designed to behave predictably in production environments and pilot deployments.

---

# Runtime Overview

AI Cost Firewall provides:

- configurable exact cache and semantic cache orchestration
- explicit readiness, liveness, and dependency-aware readiness behavior
- graceful shutdown with request draining
- nginx-style hot reload using `SIGHUP`
- startup dependency validation
- OpenAI-compatible provider diagnostics
- Prometheus metrics and Grafana dashboards
- semantic cache lifecycle control
- separate upstream and embedding timeout controls
- structured runtime error classification
- per-request cache bypass support
- request body and prompt-size protection
- optional VCAL Security Guard and VCAL Privacy Guard orchestration
- configurable guard fail-open/fail-closed behavior
- structured evidence lifecycle events
- optional buffered evidence delivery to VCAL Audit

---

# Runtime Architecture

```text
Client Applications
        │
        ▼
AI Cost Firewall
        │
        ├── VCAL Security Guard (optional)
        ├── VCAL Privacy Guard (optional)
        ├── VCAL Audit (optional evidence delivery)
        ├── Redis (exact cache)
        ├── Qdrant (semantic cache)
        │
        ▼
OpenAI-compatible provider
```

---

# Startup Behavior

Runtime dependencies are initialized during normal startup.

## Runtime Dependencies

Redis is used for the exact cache.

Example:

```conf
redis_url redis://redis:6379;
```

Exact cache behavior is configurable:

```conf
exact_cache_enabled true;
exact_cache_fail_open true;
```

When `exact_cache_enabled` is `false`, Redis exact-cache lookup and storage are skipped.

When `exact_cache_fail_open` is `true`, Redis initialization or runtime Redis errors can fail open depending on readiness policy. Requests can continue upstream, but `/readyz` may still report unavailable if `readiness_requires_redis` is enabled.

When semantic cache is enabled:

```conf
semantic_cache_enabled true;
```

Qdrant is used for semantic cache storage.

Example:

```conf
qdrant_url http://qdrant:6334;
```

---

# Startup Validation

During startup, AI Cost Firewall validates:

- Redis connectivity when exact cache is enabled and required
- Qdrant connectivity when semantic cache is enabled and required
- semantic cache configuration completeness
- embedding configuration
- Qdrant collection vector size compatibility
- provider configuration
- request body and prompt-size configuration
- model validation settings

Startup fails fast on invalid configuration to avoid silent runtime errors. Dependency behavior may also be affected by fail-open and readiness settings.

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

# exact_cache_fail_open Behavior

`exact_cache_fail_open` controls runtime Redis/exact-cache failure behavior.

Example:

```conf
exact_cache_fail_open true;
```

When enabled:

- Redis lookup failures behave like cache misses
- Redis store failures do not fail the request
- requests continue upstream

Readiness can still report Redis as unavailable when:

```conf
readiness_requires_redis true;
```

---

# semantic_cache_fail_open Behavior

`semantic_cache_fail_open` affects runtime semantic lookup behavior.

Example:

```conf
semantic_cache_fail_open true;
```

When enabled:

- runtime semantic lookup failures are treated as cache skips
- requests continue upstream normally

It does not bypass invalid configuration validation. Dependency startup behavior depends on the configured fail-open and readiness settings.

---

# Startup Behavior Summary

| Condition | Behavior |
|---|---|
| Redis unavailable + exact cache enabled + fail-open disabled | startup fails |
| Redis unavailable + exact cache enabled + fail-open enabled | startup may continue; readiness depends on `readiness_requires_redis` |
| Exact cache disabled | Redis exact-cache path skipped |
| Semantic cache enabled and Qdrant unavailable | startup fails unless semantic fail-open/runtime behavior is configured to tolerate it |
| Vector size mismatch | startup fails |
| Runtime exact-cache failure + fail-open enabled | continue upstream |
| Runtime exact-cache failure + fail-open disabled | request may fail |
| Runtime semantic lookup failure + fail-open enabled | continue upstream |
| Runtime semantic lookup failure + fail-open disabled | request fails |

---

# Guard Runtime Behavior

AI Firewall v0.4.1 can orchestrate VCAL Security Guard and VCAL Privacy Guard.

Recommended full enterprise order:

```text
Security Guard request scan
→ Privacy Guard scan/anonymize/redact
→ exact/semantic cache lookup or upstream LLM
→ Security Guard response scan
→ Privacy Guard restore
```

## guard_fail_open Behavior

`guard_fail_open` controls what happens when an enabled guard is unavailable, times out, or returns an invalid response contract.

```conf
guard_fail_open false;
```

For enterprise security and privacy deployments, fail-closed is recommended.

## Security Guard Blocks

When Security Guard blocks a request, AI Firewall returns a structured error such as:

```json
{
  "error": {
    "code": 403,
    "guard": "security",
    "type": "security_request_blocked",
    "stage": "request",
    "rule_id": "VSG-PA-003"
  }
}
```

Request-side blocks happen before Privacy Guard, cache lookup, or upstream forwarding.

## Privacy Guard Restore

When Privacy Guard runs in `anonymize` mode, AI Firewall stores the returned `mapping_id` for the request flow and calls `/v1/restore` on assistant output when restoration is enabled and a mapping exists.

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
- can return `503 Service Unavailable` when a configured dependency is unavailable
- suitable for load balancers and orchestration systems

Readiness can be configured with:

```conf
readiness_requires_redis true;
readiness_requires_qdrant false;
readiness_requires_upstream false;
```

This allows deployments to choose whether readiness should fail when Redis, Qdrant, or the upstream provider is unavailable.

---

# Readiness Summary

| State | `/healthz` | `/readyz` |
|---|---:|---:|
| Normal operation | 200 | 200 |
| Graceful shutdown | 200 | 503 |
| Required Redis unavailable | 200 | 503 |
| Required Qdrant unavailable | 200 | 503 |
| Required upstream unavailable | 200 | 503 |
| Process stopped | unavailable | unavailable |

---

---

# Audit Delivery Runtime Behavior

When `audit_enabled` is true, AI Firewall initializes a bounded buffered HTTP evidence sink during startup.

The sender:

1. accepts evidence events from request processing
2. stores them in an in-memory queue
3. forms batches by size or flush interval
4. posts batches to VCAL Audit
5. retries failed deliveries using the configured backoff

Audit delivery is deliberately decoupled from the client request path. A temporary Audit failure does not normally fail the LLM request.

## Failure behavior

The following conditions can cause evidence loss:

- the bounded queue is full
- VCAL Audit remains unavailable after retry exhaustion
- AI Firewall terminates before queued events are flushed

Delivery failures and dropped batches are logged and exposed through delivery metrics where implemented.

## Shutdown

Graceful shutdown should be used so the sender has an opportunity to flush queued evidence. The current memory-backed design is still best effort and is not equivalent to a durable disk spool.

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
# Request Limits and Cache Bypass

AI Cost Firewall can reject oversized requests before forwarding them upstream.

```conf
max_request_body_bytes 1048576;
max_prompt_chars 200000;
```

`max_request_body_bytes` limits the full HTTP request body.

`max_prompt_chars` limits the total chat message content after parsing.

Oversized requests are rejected as validation or payload-size errors and are not forwarded upstream.

Per-request cache bypass can be enabled with:

```conf
cache_bypass_header X-AIF-Cache-Bypass;
```

Example request header:

```http
X-AIF-Cache-Bypass: true
```

Accepted truthy values:

```text
true
1
yes
on
```

When cache bypass is enabled for a request:

- exact cache lookup is skipped
- semantic cache lookup is skipped
- exact cache storage is skipped
- semantic cache storage is skipped
- the request is counted by `aif_cache_bypass_requests_total`

---


# Upstream Timeout Behavior

Requests to upstream providers are limited by:

```conf
request_timeout_seconds 120;
upstream_timeout_seconds 120;
embedding_timeout_seconds 30;
```

where `request_timeout_seconds` remains a fallback.

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

1. check per-request cache bypass header
2. normalize request
3. generate embedding
4. query Qdrant
5. filter expired entries
6. evaluate similarity threshold
7. return reusable cached response
8. fallback upstream on miss

When the configured cache bypass header is present with a truthy value, exact lookup, semantic lookup, and cache storage are skipped for that request.

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

Cache writes can be controlled independently:

```conf
exact_cache_store_enabled true;
semantic_cache_store_enabled true;
```

When store controls are disabled, existing cache entries may still be read, but new upstream responses are not written to the corresponding cache layer.

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

# Guard Metrics

Guard orchestration exposes additional metrics:

```text
aif_guard_requests_total
aif_guard_latency_seconds
aif_security_blocks_total
aif_privacy_restore_skipped_total
```

Useful checks:

```bash
curl -s http://localhost:8080/metrics | grep -E 'aif_guard_requests_total|aif_security_blocks_total|aif_privacy_restore_skipped_total'
```

---
# Metrics

Metrics endpoint:

```text
http://localhost:8080/metrics
```

Metrics access can be controlled with:

```conf
metrics_auth_required false;
# metrics_auth_token replace-with-prometheus-token;
```

When `metrics_auth_required` is enabled, `/metrics` requires:

```http
Authorization: Bearer <metrics_auth_token>
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
aif_cache_hits_total{cache_type="exact"}
aif_cache_hits_total{cache_type="semantic"}
aif_cache_misses
aif_cache_bypass_requests_total
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

# Guard Operational Checks

When guard modules are enabled, useful health checks include:

```bash
curl http://localhost:8091/healthz
curl http://localhost:8091/readyz
curl http://localhost:8090/healthz
curl http://localhost:8090/readyz
```

Typical full-stack metrics checks:

```bash
curl -s http://localhost:8080/metrics | grep -E 'aif_guard_requests_total|aif_security_blocks_total'
curl -s http://localhost:8091/metrics | grep vcal_security
curl -s http://localhost:8090/metrics | grep vcal_privacy
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
aif_cache_hits_total{cache_type="exact"}
aif_cache_hits_total{cache_type="semantic"}
aif_cache_misses
aif_cache_bypass_requests_total
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

# Cache Bypass Requests Show Zero

Inspect:

```bash
curl -s http://localhost:8080/metrics | grep aif_cache_bypass_requests_total
```

Send a bypass request:

```bash
curl -s http://localhost:8080/v1/chat/completions \
  -H "Content-Type: application/json" \
  -H "Authorization: Bearer demo-key" \
  -H "X-AIF-Cache-Bypass: true" \
  -d '{"model":"gpt-4o-mini-2024-07-18","messages":[{"role":"user","content":"Bypass test"}],"temperature":0}'
```

If the metric increments after the next Prometheus scrape, the Grafana panel should show bypass traffic.

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

# Docker Container Shows Unhealthy but Logs Show Ready

If the runtime image does not include `curl`, a Dockerfile or Compose healthcheck that calls `curl` inside the container will fail even when AI Cost Firewall is running correctly.

Check the healthcheck failure:

```bash
docker inspect ai-firewall --format '{{json .State.Health}}' | jq
```

Verify the app from the host:

```bash
curl http://127.0.0.1:8080/healthz
curl http://127.0.0.1:8080/readyz
```

Prefer host-level, Prometheus, Grafana, load-balancer, or Kubernetes probes instead of installing extra tools only for container-internal healthchecks.

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

# Non-text Content

The current guard modules inspect text content only. Non-text content such as images, audio, video, and binary payloads is preserved where possible but is not scanned, anonymized, or classified by AI Firewall guard modules.

---
# Operational Notes

- reload happens only via `SIGHUP`
- `--test-config` performs static validation only
- runtime dependencies initialize during startup and reload
- Redis is required only when exact cache is enabled and the deployment treats Redis as required
- Qdrant is required only when semantic cache is enabled and the deployment treats Qdrant as required
- `exact_cache_fail_open` affects runtime Redis/exact-cache failure behavior
- `semantic_cache_fail_open` affects runtime semantic lookup behavior
- `request_timeout_seconds` is a backward-compatible fallback
- `upstream_timeout_seconds` controls chat-completion upstream calls
- `embedding_timeout_seconds` controls embedding provider calls
- upstream, embedding, or guard requests exceeding timeout are aborted
