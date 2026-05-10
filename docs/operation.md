# Operational Behavior

This document describes how AI Cost Firewall behaves at runtime: health checks, readiness, graceful shutdown, timeouts, live configuration reload, semantic cache lifecycle, and operational troubleshooting.

---

## Runtime Behavior Overview

AI Cost Firewall is designed to behave predictably in production environments.

Key capabilities:

- explicit error classification: validation, upstream, timeout, authentication, DNS/connect, TLS, and internal errors
- upstream latency and timeout visibility
- readiness and liveness separation
- graceful shutdown with request draining
- hot configuration reload via `SIGHUP`
- semantic cache diagnostics: candidates, threshold decisions, lookup latency, expiration behavior
- semantic cache lifecycle control
- strict startup dependency initialization
- runtime semantic cache fail-open behavior
- OpenAI-compatible provider diagnostics for upstream and embedding endpoints

---

## Startup Dependency Behavior

Runtime dependencies are initialized when the firewall starts normally.

### Required dependencies

Redis is required for exact caching.

If semantic caching is enabled:

```conf
semantic_cache_enabled true;
```

then Qdrant must also be reachable during startup.

If the configured Qdrant collection already exists, AI Cost Firewall validates that its vector size matches:

```conf
qdrant_vector_size 1536;
```

A mismatch fails startup clearly instead of producing confusing runtime errors.

### `semantic_cache_fail_open` does not change startup behavior

`semantic_cache_fail_open` controls runtime lookup behavior only.

Example:

```conf
semantic_cache_fail_open true;
```

When enabled, runtime semantic lookup failures are treated as cache skips and requests continue upstream.

It does **not** allow startup to continue if Qdrant cannot be initialized while semantic cache is enabled.

Summary:

| Setting / condition | Behavior |
|---|---|
| Redis unavailable | startup fails |
| `semantic_cache_enabled true` and Qdrant unavailable | startup fails |
| Qdrant collection vector size mismatch | startup fails |
| Runtime semantic lookup error and `semantic_cache_fail_open true` | request continues upstream |
| Runtime semantic lookup error and `semantic_cache_fail_open false` | request fails |

---

## Health & Readiness Endpoints

AI Cost Firewall exposes two standard operational endpoints.

### `/healthz` — Liveness

Indicates that the process is running.

```bash
curl -i http://localhost:8080/healthz
```

Behavior:

- returns `200 OK` when the process is alive
- does not depend on downstream services
- can be used by orchestrators to detect crashed processes

---

### `/readyz` — Readiness

Indicates whether the service is ready to accept traffic.

```bash
curl -i http://localhost:8080/readyz
```

Behavior:

- returns `200 OK` during normal operation
- returns `503 Service Unavailable` when the firewall is shutting down or not accepting new requests
- can be used by load balancers and orchestrators before routing traffic

---

### Behavior Summary

| State | `/healthz` | `/readyz` |
|---|---:|---:|
| Normal operation | 200 | 200 |
| During graceful shutdown | 200 | 503 |
| Process crashed | unavailable | unavailable |

---

## Graceful Shutdown

AI Cost Firewall supports graceful shutdown on:

- `SIGTERM`
- `SIGINT`

This is useful for systemd, Docker, Kubernetes, and other deployment environments.

### Shutdown Sequence

1. Shutdown signal is received.
2. Readiness changes to unavailable.
3. `/readyz` returns `503`.
4. New requests are rejected.
5. In-flight requests are allowed to complete.
6. The process exits after `graceful_shutdown_timeout_seconds`.

Example configuration:

```conf
graceful_shutdown_timeout_seconds 10;
```

### Runtime Behavior

During shutdown:

- `/healthz` still returns `200 OK`
- `/readyz` returns `503 Service Unavailable`
- new requests are rejected
- in-flight requests continue until completed or timeout expires
- shutdown state is exposed through metrics

### Shutdown Rejection Response

New requests received during shutdown return:

```json
{
  "error": {
    "code": 503,
    "message": "server is shutting down",
    "type": "service_unavailable"
  }
}
```

### Related Metrics

```text
aif_inflight_requests
aif_shutdown_in_progress
aif_shutdown_rejections_total
```

---

## Upstream Timeout Behavior

Requests to upstream LLM providers are bounded by:

```conf
request_timeout_seconds 120;
```

### Behavior

If an upstream request exceeds the configured timeout:

- the request is aborted
- the error is classified as `upstream_timeout`
- the timeout is counted in Prometheus metrics
- the firewall does not wait indefinitely for a slow provider

### Related Metrics

```text
aif_upstream_timeouts_total
aif_upstream_request_duration_seconds
```

This helps distinguish provider slowness from local firewall errors.

---

## OpenAI-Compatible Provider Diagnostics

AI Cost Firewall classifies common OpenAI-compatible provider failures to make configuration and runtime issues easier to diagnose.

Common upstream error classes include:

```text
upstream_authentication_error
upstream_not_found
upstream_rate_limited
upstream_timeout
upstream_tls_error
upstream_dns_error
upstream_connect_error
```

Typical causes:

| Error class | Common cause |
|---|---|
| `upstream_authentication_error` | Provider rejected the configured API key |
| `upstream_not_found` | Wrong `upstream_base_url` or full endpoint path configured |
| `upstream_rate_limited` | Provider quota or rate limit reached |
| `upstream_timeout` | Provider did not respond before `request_timeout_seconds` |
| `upstream_tls_error` | Certificate validation or hostname mismatch |
| `upstream_dns_error` | Provider hostname cannot be resolved |
| `upstream_connect_error` | Provider host or port is unreachable |

For local OpenAI-compatible providers without authentication, use a placeholder API key such as `dummy`, `none`, `null`, or `-``.

---

## Configuration Reload

AI Cost Firewall supports nginx-style hot reload using `SIGHUP`.

Reloading allows configuration changes without fully restarting the process.

### Binary / systemd deployment

```bash
kill -HUP <firewall_pid>
```

Example:

```bash
kill -HUP $(pgrep ai-firewall)
```

### Docker Compose deployment

```bash
docker compose kill -s HUP firewall
```

If your Compose service has a different name, check it with:

```bash
docker compose ps --services
```

### Reload Behavior

On `SIGHUP`, the firewall:

1. reloads the configuration file
2. validates the new configuration
3. rebuilds runtime dependencies
4. atomically swaps the active runtime state
5. continues serving traffic

Expected log messages:

```text
received SIGHUP, reloading config
config and runtime successfully reloaded
```

### Important Notes

Reload is not simple file watching. The firewall reloads only when it receives `SIGHUP`.

If the new configuration is invalid, reload fails and the existing runtime configuration remains active.

---

## Static Configuration Validation

For static validation, use:

```bash
cargo run -- --config configs/ai-firewall.conf --test-config
```

Expected output:

```text
configuration OK
```

`--test-config` validates the configuration file only.

It checks:

- syntax
- required directives
- value formats
- semantic cache configuration completeness
- model validation configuration

It does **not** connect to Redis, Qdrant, embedding providers, or upstream LLM providers.

Runtime dependencies are checked only during normal startup or runtime reload.

---

## Print Loaded Configuration

To inspect the resolved configuration:

```bash
cargo run -- --config configs/ai-firewall.conf --print-config
```

Sensitive fields are masked.

Example:

```text
upstream_api_key = sk-y...-key
embedding_api_key = sk-y...-key
qdrant_api_key = <not set>
```

This is useful for troubleshooting without exposing credentials in logs or terminals.

---

## Metrics

AI Cost Firewall exposes Prometheus metrics at:

```text
/metrics
```

Example:

```bash
curl http://localhost:8080/metrics
```

---

### Core Runtime Metrics

```text
aif_inflight_requests
aif_shutdown_in_progress
aif_shutdown_rejections_total
aif_readiness_state
```

Meaning:

- `aif_inflight_requests` — active request count
- `aif_shutdown_in_progress` — shutdown state, `1` or `0`
- `aif_shutdown_rejections_total` — requests rejected during shutdown
- `aif_readiness_state` — readiness state, `1` or `0`

---

### Error Classification Metrics

Errors are categorized as:

- `validation_error`
- `upstream_error`
- `upstream_timeout`
- `upstream_authentication_error`
- `upstream_not_found`
- `upstream_rate_limited`
- `upstream_tls_error`
- `upstream_dns_error`
- `upstream_connect_error`
- `internal_error`

Metric:

```text
aif_errors_total{class="validation_error"}
aif_errors_total{class="upstream_authentication_error"}
aif_errors_total{class="upstream_timeout"}
aif_errors_total{class="upstream_tls_error"}
```

Examples:

```text
aif_errors_total{class="validation_error"}
aif_errors_total{class="upstream_timeout"}
```

---

### Upstream Metrics

```text
aif_upstream_request_duration_seconds
aif_upstream_timeouts_total
aif_upstream_calls_total
```

These help diagnose provider latency, timeout behavior, and upstream usage.

---

### Embedding Provider Metrics

```text
aif_embedding_request_duration_seconds
aif_embedding_timeouts_total
```

These help diagnose slow or unavailable embedding providers used by semantic cache lookup and storage.

---

### Cache Metrics

```text
aif_cache_exact_hits
aif_cache_semantic_hits
aif_cache_misses
```

These metrics show whether requests are served from exact cache, semantic cache, or forwarded upstream.

---

### Cost and Token Metrics

```text
aif_tokens_saved
aif_chat_cost_saved_micro_usd
aif_embedding_cost_micro_usd
aif_cost_saved_micro_usd
```

Meaning:

- `aif_chat_cost_saved_micro_usd` — gross avoided chat-completion cost
- `aif_embedding_cost_micro_usd` — embedding lookup cost
- `aif_cost_saved_micro_usd` — net savings
- `aif_tokens_saved` — estimated avoided chat-completion tokens

If `embedding_price` is not configured, embedding cost is treated as zero and savings may be overestimated.

---

### Semantic Cache Diagnostics

```text
aif_semantic_candidates_checked_total
aif_semantic_threshold_results_total{result="pass"}
aif_semantic_threshold_results_total{result="fail"}
aif_semantic_expired_entries_skipped_total
aif_semantic_lookup_duration_seconds
aif_semantic_store_total
aif_semantic_store_errors_total
```

These metrics help tune:

- `semantic_similarity_threshold`
- `semantic_cache_retention_seconds`
- embedding provider performance, together with `aif_embedding_request_duration_seconds` and `aif_embedding_timeouts_total`
- Qdrant behavior

---

## Semantic Cache Runtime Behavior

Semantic caching evaluates similar requests before forwarding to upstream.

### Lookup Flow

1. Normalize the request.
2. Generate an embedding.
3. Query Qdrant for candidates.
4. Filter expired entries before similarity ranking.
5. Evaluate similarity threshold.
6. Return the first valid cached response.
7. If no valid match exists, continue upstream.

### Candidate Requirements

A semantic candidate is reusable only if:

```text
similarity_score >= semantic_similarity_threshold
AND
expires_at > now
AND
cached response payload is valid
```

### Rejection Reasons

Candidates may be skipped because:

- similarity is below threshold
- entry is expired
- required metadata is missing
- cached response payload is invalid
- semantic lookup failed and fail-open behavior is enabled

### Runtime Fail-Open Behavior

When enabled:

```conf
semantic_cache_fail_open true;
```

runtime semantic cache failures are treated as cache skips.

The request then continues to the upstream LLM provider.

This is useful when semantic caching should improve cost and latency, but should not block normal LLM traffic if Qdrant or the embedding provider fails during runtime.

---

## Semantic Cache Lifecycle

Semantic cache entries include lifecycle metadata:

- `inserted_at`
- `expires_at`

The expiration timestamp is calculated from:

```conf
semantic_cache_retention_seconds 604800;
```

### Runtime Behavior

Expired entries:

- are not reused
- are filtered during lookup before similarity ranking
- may remain stored in Qdrant until manually pruned

This prevents expired entries from blocking valid non-expired semantic hits.

### Correctness vs Cleanup

Semantic cache correctness does not depend on cleanup.

Even if expired entries remain in Qdrant, they are not returned as valid cache hits.

Cleanup is only needed to reduce Qdrant collection size over time.

---

## Semantic Cache Cleanup

Expired semantic cache entries can be physically removed from Qdrant with:

```bash
ai-firewall --config configs/ai-firewall.conf --prune-expired-semantic-cache
```

The command removes entries where:

```text
expires_at <= now
```

Valid entries remain untouched.

---

### Binary / systemd deployment

For conservative maintenance windows:

```bash
systemctl stop ai-firewall
ai-firewall --config /etc/ai-firewall/ai-firewall.conf --prune-expired-semantic-cache
systemctl start ai-firewall
```

---

### Docker Compose deployment

Run pruning as a one-off container using the same Compose service and config:

```bash
docker compose run --rm firewall \
  --config /configs/ai-firewall.conf \
  --prune-expired-semantic-cache
```

This starts a temporary container on the same Docker network, connects to Qdrant using the configured `qdrant_url`, prunes expired semantic entries, and exits.

The running firewall container does not need to be stopped.

---

### Verify Qdrant Collection Count

Qdrant exposes collection operations through its REST API on port `6333`.

Example count request:

```bash
curl -s http://127.0.0.1:6333/collections/aif_semantic_cache/points/count \
  -H "Content-Type: application/json" \
  -d '{"exact": true}'
```

Note:

- AI Cost Firewall uses Qdrant gRPC on port `6334`
- manual Qdrant inspection commonly uses the REST API on port `6333`

---

## Maintenance

Semantic cache storage may grow over time depending on traffic volume and retention settings.

Recommended maintenance actions:

- review `semantic_cache_retention_seconds`
- monitor Qdrant collection size
- periodically run `--prune-expired-semantic-cache`
- check semantic cache metrics in Prometheus or Grafana

Cleanup frequency depends on workload.

For small deployments, manual pruning may be enough.

For larger deployments, run pruning periodically through:

- cron
- systemd timer
- Kubernetes CronJob
- scheduled CI/CD maintenance job

---

## Logging

AI Cost Firewall writes runtime logs to stdout/stderr by default.

It does not create, store, rotate, or manage log files internally. In production, logs should be collected by the runtime environment, such as Docker, Docker Compose, systemd, Kubernetes, or a centralized logging stack.

### Default behavior

When AI Cost Firewall is started from a terminal, logs are printed to the console:

```bash
./ai-firewall --config configs/ai-firewall.conf
```

When started in Docker or Docker Compose, logs are captured by the container runtime and can be viewed using standard Docker commands.

### View logs with Docker Compose

```bash
docker compose logs -f firewall
```

### View logs from a Docker container

```bash
docker logs -f <container_name_or_id>
```

Example:

```bash
docker logs -f ai-firewall
```

### Save logs from a local binary run

To save stdout and stderr to a file:

```bash
./ai-firewall --config configs/ai-firewall.conf > logs.txt 2>&1
```

To append logs instead of overwriting the file:

```bash
./ai-firewall --config configs/ai-firewall.conf >> logs.txt 2>&1
```

To view logs in the terminal and save them at the same time:

```bash
./ai-firewall --config configs/ai-firewall.conf 2>&1 | tee logs.txt
```

### Save logs from Docker Compose

To save Docker Compose logs to a file:

```bash
docker compose logs -f firewall > logs.txt 2>&1
```

To view Docker Compose logs and save them at the same time:

```bash
docker compose logs -f firewall 2>&1 | tee logs.txt
```

### Save logs from Docker run

If AI Cost Firewall is started with `docker run`, shell redirection can also be used:

```bash
docker run --rm \
  -p 8080:8080 \
  -v "$PWD/configs:/configs:ro" \
  vcalproject/ai-cost-firewall:latest \
  --config /configs/ai-firewall.conf > logs.txt 2>&1
```

To view and save at the same time:

```bash
docker run --rm \
  -p 8080:8080 \
  -v "$PWD/configs:/configs:ro" \
  vcalproject/ai-cost-firewall:latest \
  --config /configs/ai-firewall.conf 2>&1 | tee logs.txt
```

### Save logs with systemd

When AI Cost Firewall runs as a systemd service, logs are usually available through `journalctl`:

```bash
journalctl -u ai-firewall -f
```

To export logs to a file:

```bash
journalctl -u ai-firewall > logs.txt
```

To follow logs and save them at the same time:

```bash
journalctl -u ai-firewall -f | tee logs.txt
```

### Log retention and rotation

AI Cost Firewall does not rotate log files itself.

Log retention, rotation, and forwarding should be handled by the runtime environment or logging platform, for example:

- Docker logging drivers
- systemd journald configuration
- Kubernetes logging infrastructure
- logrotate
- centralized logging tools such as Loki, Elasticsearch, or OpenSearch

For containerized deployments, prefer collecting stdout/stderr logs through the container runtime instead of writing application logs directly to files inside the container.

---

## Troubleshooting

### Low cache hit rate

Check:

- `aif_cache_exact_hits`
- `aif_cache_semantic_hits`
- `aif_cache_misses`
- `aif_semantic_threshold_results_total{result="fail"}`

Common causes:

- semantic threshold is too high
- prompts are not actually similar
- request parameters differ
- semantic cache is disabled
- retention window is too short
- entries expired before reuse

---

### Upstream provider configuration errors

Check:

```text
aif_errors_total{class="upstream_authentication_error"}
aif_errors_total{class="upstream_not_found"}
aif_errors_total{class="upstream_tls_error"}
aif_errors_total{class="upstream_dns_error"}
aif_errors_total{class="upstream_connect_error"}
```

Common causes:

- wrong `upstream_api_key`
- local provider configured with a real key requirement but no valid key
- full endpoint path configured instead of base URL
- provider hostname cannot be resolved from the firewall container
- provider port is not reachable
- self-signed or hostname-mismatched TLS certificate

For local providers without authentication, use:

```text
upstream_api_key dummy;
```

For OpenAI-compatible providers, configure the base URL, not the full endpoint:

```test
# Correct
upstream_base_url http://ollama:11434/v1;

# Wrong
upstream_base_url http://ollama:11434/v1/chat/completions;
```



---

### High upstream latency

Check:

```text
aif_upstream_request_duration_seconds
```

Common causes:

- upstream provider is slow
- network latency
- large prompts
- model latency
- provider-side throttling

---

### Frequent upstream timeouts

Check:

```text
aif_upstream_timeouts_total
```

Possible actions:

- increase `request_timeout_seconds`
- inspect upstream provider health
- reduce prompt size
- check network connectivity
- use a faster model/provider

---

### Frequent embedding timeouts

Check:

```text
aif_embedding_timeouts_total
aif_embedding_request_duration_seconds
```

Common causes:

- embedding provider is slow or overloaded
- wrong embedding_base_url
- embedding provider is not reachable from the firewall container
- embedding model is too slow for the configured timeout
- semantic cache is enabled but embedding endpoint is misconfigured

Possible actions:

- increase request_timeout_seconds
- verify embedding_base_url
- use a faster embedding model/provider
- disable semantic cache for basic local testing

---

### High validation errors

Check:

```text
aif_errors_total{class="validation_error"}
```

Common causes:

- unsupported model
- malformed JSON
- missing `messages`
- request body too large
- invalid request format

---

### High semantic store errors

Check:

```text
aif_semantic_store_errors_total
```

Common causes:

- Qdrant unavailable
- collection configuration mismatch
- embedding vector size mismatch
- malformed Qdrant payload
- network issues between firewall and Qdrant

---

### Qdrant startup failure

If semantic cache is enabled, Qdrant must be reachable at startup.

Check:

```conf
qdrant_url http://qdrant:6334;
```

For Docker Compose, use service names:

```conf
redis_url redis://redis:6379;
qdrant_url http://qdrant:6334;
```

For local binary execution, use local addresses:

```conf
redis_url redis://127.0.0.1:6379;
qdrant_url http://127.0.0.1:6334;
```

---

### Qdrant vector size mismatch

If the existing Qdrant collection was created with a different vector size, startup fails.

Example:

```text
Qdrant collection 'aif_semantic_cache' has vector size 768, but config requires 1536
```

Fix options:

- recreate the Qdrant collection
- use a matching embedding model
- update `qdrant_vector_size` to match the collection and embedding model

---

## Notes

- No automatic file watching
- Reload happens via `SIGHUP`
- `--test-config` is static validation only
- Runtime dependencies are initialized during normal startup and reload
- Redis is required for exact caching
- Qdrant is required only when semantic cache is enabled
- `semantic_cache_fail_open` applies to runtime semantic lookup failures, not startup initialization
- Requests exceeding `request_timeout_seconds` are aborted and classified as upstream timeouts
