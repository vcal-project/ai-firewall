
# Troubleshooting

AI Cost Firewall is designed to fail fast with explicit startup validation and actionable runtime errors.

This document covers the most common deployment and operational issues.

---

# Validate Configuration First

Before debugging runtime behavior, validate the configuration:

```bash
docker compose run --rm firewall \
  --config /configs/ai-firewall.conf \
  --test-config
```

Expected output:

```text
configuration OK
```

This performs static validation only and does not contact external services.

---

# Firewall Does Not Start

## Symptoms

```text
container exits immediately
```

or:

```text
configuration error: ...
```

## Common Causes

- missing required config fields
- invalid upstream URL
- missing embedding configuration
- invalid vector size
- malformed nginx-style config syntax

## Example Errors

### Missing embedding configuration

```text
configuration error: semantic_cache_enabled=true requires: embedding_model, qdrant_url
```

### Invalid request size

```text
configuration error: invalid AIF_MAX_REQUEST_BODY_BYTES value 'abc'
```

### No allowed models configured

```text
configuration error: no allowed models configured
```

## Recommended Checks

```bash
docker compose logs firewall
```

and:

```bash
docker compose run --rm firewall \
  --config /configs/ai-firewall.conf \
  --test-config
```

---

# Redis Connection Failures

## Symptoms

```text
failed to connect to redis
```

or:

```text
connection refused
```

## Common Causes

- Redis container not running
- wrong Redis hostname
- wrong Redis port
- firewall running outside Docker network

## Recommended Checks

Verify Redis:

```bash
docker compose ps
```

Test connectivity:

```bash
docker compose exec firewall ping redis
```

Check Redis logs:

```bash
docker compose logs redis
```

## Typical Configuration

```text
redis_url redis://redis:6379;
```

---

# Qdrant Connection Failures

## Symptoms

```text
failed to initialize qdrant
```

or:

```text
connection refused
```

## Common Causes

- Qdrant container not running
- wrong Qdrant port
- using HTTP instead of gRPC
- incorrect hostname

## Recommended Checks

Check Qdrant:

```bash
docker compose ps
```

Verify port:

```text
6334 = gRPC
6333 = HTTP
```

Recommended configuration:

```text
qdrant_url http://qdrant:6334;
```

Check logs:

```bash
docker compose logs qdrant
```

---

# Qdrant Vector Size Mismatch

## Symptoms

```text
existing collection vector size does not match qdrant_vector_size
```

## Cause

The configured embedding model dimension differs from the existing Qdrant collection dimension.

Example:

- OpenAI `text-embedding-3-small` → 1536
- Ollama `nomic-embed-text` → 768

## Solutions

### Option 1 — Use matching vector size

```text
qdrant_vector_size 1536;
```

### Option 2 — Remove old collection

```bash
curl -X DELETE http://localhost:6333/collections/aif_semantic_cache
```

Then restart the firewall.

---

# TLS / Certificate Errors

## Symptoms

```text
502 Bad Gateway
```

or:

```text
upstream_tls_error
```

## Common Causes

- self-signed certificates
- hostname mismatch
- expired certificate
- missing SAN entries
- local HTTPS provider without trusted CA

## Example

```text
certificate verify failed
```

## Recommended Checks

Test upstream directly:

```bash
curl https://your-provider/v1/models
```

Inspect certificate:

```bash
openssl s_client -connect host:443
```

## Notes

Local providers such as Ollama usually work more reliably over:

```text
http://
```

inside trusted local networks.

---

# Wrong Upstream Base URL

## Symptoms

```text
404
```

or:

```text
upstream_not_found
```

## Cause

Using a full endpoint path instead of a provider base URL.

## Wrong

```text
upstream_base_url http://ollama:11434/v1/chat/completions;
```

## Correct

```text
upstream_base_url http://ollama:11434/v1;
```

AI Cost Firewall automatically appends OpenAI-compatible endpoint paths.

---

# Embedding Provider Failures

## Symptoms

```text
semantic lookup failed
```

or:

```text
embedding request failed
```

## Common Causes

- embedding model not available
- wrong embedding endpoint
- incompatible embedding API
- local embedding model not pulled

## Ollama Example

Pull embedding model:

```bash
docker compose exec ollama ollama pull nomic-embed-text
```

Restart firewall:

```bash
docker compose restart firewall
```

---

# Semantic Cache Not Producing Hits

## Symptoms

- exact cache works
- semantic cache rarely hits

## Common Causes

- threshold too high
- prompts too different
- embeddings not working
- empty Qdrant collection

## Recommended Checks

Inspect metrics:

```bash
curl -s http://localhost:8080/metrics | grep semantic
```

Important metrics:

```text
aif_semantic_candidates_checked_total
aif_semantic_threshold_results_total
aif_cache_semantic_hits
```

## Recommended Thresholds

Typical starting point:

```text
semantic_similarity_threshold 0.92;
```

Lower threshold:

- more semantic reuse
- higher risk of incorrect matches

Higher threshold:

- stricter matching
- fewer semantic hits

---

# Grafana Dashboards Are Empty

## Symptoms

- dashboards load
- graphs remain empty

## Common Causes

- no traffic generated
- Prometheus cannot scrape firewall
- metrics endpoint unavailable
- wrong provisioning paths

## Recommended Checks

Verify metrics:

```bash
curl http://localhost:8080/metrics
```

Verify Prometheus targets:

```text
http://localhost:9090/targets
```

Verify Grafana provisioning:

```bash
docker compose logs grafana
```

## Generate Demo Traffic

Run repeated requests:

```bash
for i in {1..20}; do
  curl http://localhost:8080/v1/chat/completions \
    -H "Content-Type: application/json" \
    -d '{"model":"gpt-4o-mini-2024-07-18","messages":[{"role":"user","content":"Explain Redis briefly."}]}'
done
```

---

# Health Checks Fail

## `/healthz` Fails

Indicates the process itself is unhealthy or not running.

Check:

```bash
docker compose logs firewall
```

---

## `/readyz` Fails

Indicates the process is alive but not ready to serve traffic.

Common causes:

- startup initialization incomplete
- Redis unavailable
- Qdrant unavailable
- graceful shutdown in progress

---

# Upstream Timeouts

## Symptoms

```text
upstream_timeout
```

or:

```text
aif_upstream_timeouts_total increasing
```

## Common Causes

- provider overloaded
- slow local models
- large prompts
- insufficient timeout

## Recommended Checks

Increase timeout:

```text
request_timeout_seconds 120;
```

Inspect latency metrics:

```text
aif_upstream_request_duration_seconds
```

---

# Large Requests Rejected

## Symptoms

```json
{
  "error": {
    "code": 413
  }
}
```

## Cause

Request exceeds configured limit.

## Example

```text
max_request_body_bytes 1M;
```

Supported formats:

```text
512K
1M
2M
```

---

# Docker Compose Path Problems

## Symptoms

- dashboards not found
- provisioning missing
- config files missing

## Common Cause

Running Docker Compose from the wrong directory.

## Recommended Pattern

```bash
cd deploy/examples/openai-cloud

docker compose up -d
```

For observability overlays:

```bash
docker compose \
  -f docker-compose.yml \
  -f docker-compose.observability.yml \
  up -d
```

---

# Logs

AI Cost Firewall logs to stdout/stderr.

View logs:

```bash
docker compose logs -f firewall
```

Save logs:

```bash
docker compose up -d > logs.txt 2>&1
```

or:

```bash
docker compose logs firewall > firewall.log
```

---

# Useful Metrics for Debugging

## Cache Behavior

```text
aif_cache_exact_hits
aif_cache_semantic_hits
aif_cache_misses
```

## Semantic Diagnostics

```text
aif_semantic_candidates_checked_total
aif_semantic_threshold_results_total
aif_semantic_lookup_duration_seconds
```

## Runtime Health

```text
aif_inflight_requests
aif_shutdown_in_progress
aif_readiness_state
```

## Errors

```text
aif_errors_total
aif_upstream_timeouts_total
aif_embedding_timeouts_total
```

---

# Provider Compatibility Notes

## OpenAI

Reference implementation.

---

## Ollama

Use OpenAI-compatible mode.

Recommended:

```text
http://ollama:11434/v1
```

Pull models before testing.

---

## LM Studio

Verify that embeddings are enabled.

---

## vLLM

Check timeout configuration for large models.

---

## LiteLLM

Useful aggregation layer for multiple providers.

---

## OpenRouter

Enable:

```text
allow_unknown_models_pass_through true;
```

because model naming varies by provider.

---

# Additional Documentation

See also:

- `docs/config-reference.md`
- `docs/provider-compatibility.md`
- `docs/operation.md`
- `docs/metrics-and-costs.md`
- `docs/quickstart.md`
