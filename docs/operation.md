# Operational Behavior

This section describes how AI Cost Firewall behaves at runtime: health checks, graceful shutdown, and live configuration reload.

---

## Runtime behavior overview

AI Cost Firewall provides predictable runtime behavior with explicit lifecycle control, observability, and safe shutdown semantics.

Key capabilities:

- explicit error classification (validation / upstream / timeout / internal)
- upstream latency and timeout visibility
- semantic cache diagnostics (threshold decisions, expiration behavior)
- graceful shutdown with request draining
- readiness and liveness separation
- semantic cache lifecycle control (v0.1.5)

---

## Health & Readiness Endpoints

AI Cost Firewall exposes two standard endpoints:

### `/healthz` — Liveness

Indicates that the process is running.

- Returns: `200 OK`
- Does not depend on downstream services
- Used by orchestrators to detect crashes

```
curl -i http://localhost:8080/healthz
```

---

### `/readyz` — Readiness

Indicates whether the service is ready to accept new requests.

- Returns:
  - `200 OK` → ready
  - `503 Service Unavailable` → not accepting traffic
- Used by load balancers and orchestrators

```
curl -i http://localhost:8080/readyz
```

---

### Behavior summary

| State                 | `/healthz` | `/readyz` |
|----------------------|-----------|----------|
| Normal operation     | 200       | 200      |
| During shutdown      | 200       | 503      |
| Process crashed      | ❌        | ❌       |

---

## Graceful Shutdown

AI Cost Firewall supports graceful shutdown on:

- `SIGTERM`
- `SIGINT`

### Shutdown sequence

1. Receive signal
2. `/readyz` returns `503`
3. New requests are rejected
4. In-flight requests complete
5. Process exits after `graceful_shutdown_timeout_seconds`

### Behavior

- `/readyz` returns `503` immediately
- New requests are rejected
- In-flight requests continue
- Rejections are tracked (`aif_shutdown_rejections_total`)
- Shutdown state is exposed (`aif_shutdown_in_progress`)

---

## Upstream Timeout Behavior

Requests to upstream providers are bounded by `request_timeout_seconds`.

### Behavior

- Requests exceeding the timeout are aborted
- Returned as `upstream_timeout` errors
- Counted in metrics

### Observability

- `aif_upstream_timeouts_total`
- `aif_upstream_request_duration_seconds`

This prevents the system from hanging on slow or unresponsive providers.

---

## Request Rejection During Shutdown

```
{
  "error": {
    "code": 503,
    "message": "server is shutting down",
    "type": "service_unavailable"
  }
}
```

---

## Configuration Reload (SIGHUP)

Reload without restart:

```
kill -HUP <pid>
```

### Behavior

- Config reloaded and validated
- Runtime rebuilt (Redis, Qdrant, embeddings, upstream)
- Atomic swap
- No downtime

---

## Metrics

AI Cost Firewall exposes Prometheus metrics for runtime visibility.

### Core runtime metrics

- `aif_inflight_requests` — active request count
- `aif_shutdown_in_progress` — shutdown state (1/0)
- `aif_shutdown_rejections_total` — requests rejected during shutdown

### Error classification

Errors are categorized as:

- `validation_error`
- `upstream_error`
- `upstream_timeout`
- `internal_error`

Metric:

`aif_errors_total{class=...}`


### Upstream behavior

- `aif_upstream_request_duration_seconds`
- `aif_upstream_timeouts_total`

### Semantic cache diagnostics

- `aif_semantic_candidates_checked_total`
- `aif_semantic_threshold_results_total{result="pass|fail"}`
- `aif_semantic_expired_entries_skipped_total`
- `aif_semantic_lookup_duration_seconds`
- `aif_semantic_store_total`
- `aif_semantic_store_errors_total`

---

## Semantic Cache Runtime Behavior

Semantic caching evaluates similar requests before forwarding to upstream.

### Lookup flow

1. Generate embedding for request
2. Query Qdrant for candidates
3. For each candidate:
   - check similarity threshold
   - check expiration (`expires_at`)
4. Return first valid match

### Rejection reasons

Candidates may be skipped due to:

- similarity below threshold
- expired TTL
- missing metadata

### Observability

- candidates evaluated
- threshold pass / fail counts
- expired entries skipped

These metrics help tune:

- `semantic_similarity_threshold`
- `semantic_cache_retention_seconds`

---

## Semantic Cache Lifecycle (v0.1.5)

Semantic cache entries include lifecycle metadata:

- `inserted_at`
- `expires_at`

### Runtime behavior

- expired entries are skipped during lookup
- expired entries are never returned
- expired entries remain stored in Qdrant

This ensures:

- no reuse of stale responses
- predictable semantic cache behavior
- safe operation without background cleanup

### Cleanup

Expired entries can be removed manually:

```bash
ai-firewall --prune-expired-semantic-cache
```

Recommended usage:

```bash
systemctl stop ai-firewall
ai-firewall --config /path/to/ai-firewall.conf --prune-expired-semantic-cache
systemctl start ai-firewall
```

Notes:

- pruning deletes only expired entries (`expires_at < now`)
- valid entries are not affected
- pruning is optional and does not affect correctness

---

## Maintenance

Semantic cache storage may grow over time depending on retention settings.

To reclaim space:

```bash
ai-firewall --prune-expired-semantic-cache
```

This operation:

- removes expired semantic entries
- does not affect active cache entries
- can be run during maintenance windows

---

## Troubleshooting

### Low cache hit rate

Check:

- semantic threshold too high
- TTL too short
- many expired entries

### High upstream latency

Check:

- `aif_upstream_request_duration_seconds`
- upstream provider health

### Frequent timeouts

- increase `request_timeout_seconds`
- inspect upstream performance

### High error rate

Inspect:

`aif_errors_total{class=...}`

- `validation_error` → bad requests
- `upstream_error` → provider issues
- `upstream_timeout` → slow upstream
- `internal_error` → system issue

---

## Notes

- No automatic file watching
- Reload via SIGHUP only
- Requests exceeding `request_timeout_seconds` are aborted and classified as upstream timeouts
- `aif_semantic_store_total`
- `aif_semantic_store_errors_total`