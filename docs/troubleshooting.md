# Troubleshooting

AI Cost Firewall is designed to fail fast during startup, expose clear runtime errors, and make cache, provider, and cost behavior observable.

This document covers common deployment and operational issues for v0.2.0, the first pilot-ready OpenAI-compatible gateway milestone.

AI Cost Firewall v0.2.0 supports OpenAI-compatible chat and embedding APIs through a simple configuration model. It does not yet provide native provider-specific API integrations or provider-specific configuration blocks.

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

`--test-config` validates configuration syntax and internal consistency. It does not verify that Redis, Qdrant, the upstream LLM endpoint, or the embedding endpoint are reachable.

Runtime dependencies are initialized during normal startup:

- Redis is required for exact cache
- Qdrant is required when semantic cache is enabled
- embedding configuration is required when semantic cache is enabled
- the configured Qdrant vector size must match the embedding model dimension

After the service starts, confirm the running release and readiness state:

```bash
curl http://localhost:8080/version
curl http://localhost:8080/healthz
curl http://localhost:8080/readyz
```

Expected basic responses:

```text
OK
ready
```

The /version endpoint returns release metadata, including the AI Cost Firewall version, release title, and OpenAI-compatible compatibility model.

---

# Basic Diagnostic Commands

Use these commands before deeper debugging:

```bash
docker compose ps
curl http://localhost:8080/version
curl http://localhost:8080/healthz
curl http://localhost:8080/readyz
curl -s http://localhost:8080/metrics | head
docker compose logs --tail=100 firewall
docker compose logs --tail=100 redis
docker compose logs --tail=100 qdrant
```

These commands confirm:

which services are running
which AI Cost Firewall release is active
whether the process is alive
whether it is ready to serve traffic
whether metrics are exposed
recent firewall, Redis, and Qdrant errors

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

# Semantic Cache Fail-Open Behavior

## Symptoms

Requests continue successfully even though semantic cache lookup, embedding, or semantic store errors appear in logs.

Or, when fail-open is disabled, requests fail with semantic cache or embedding-related errors.

## Cause

AI Cost Firewall can be configured to fail open for runtime semantic cache failures.

When enabled:

```text
semantic_cache_fail_open true;
```

runtime semantic cache failures do not block the request. AI Cost Firewall skips the semantic cache path and continues to the upstream LLM endpoint.

When disabled:

```text
semantic_cache_fail_open false;
```

runtime semantic cache failures may return an error instead of silently falling back to the upstream path.

## Important Distinction

`semantic_cache_fail_open` applies to runtime semantic cache operations.

It does not bypass startup dependency validation. If semantic cache is enabled, Qdrant must be reachable during startup, and the configured `qdrant_vector_size` must match the existing collection or the embedding model dimension.

## Recommended Checks

Check semantic-related logs:

```bash
docker compose logs firewall | grep -i semantic
```

Check embedding-related logs:

```bash
docker compose logs firewall | grep -i embedding
```

Check semantic metrics:

```bash
curl -s http://localhost:8080/metrics | grep semantic
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
Grafana Dashboards Are Empty
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

# Cost or Savings Metrics Look Unexpected

## Symptoms

- net savings are lower than expected
- semantic cache hits occur but savings look small
- embedding overhead is visible even when chat savings are low
- dashboard savings panels appear unrealistic during short demos

## Cause

AI Cost Firewall separates:

- gross chat-completion savings
- embedding overhead
- net savings after embedding cost

Semantic cache lookup may require embedding generation. This means semantic cache can save chat-completion cost while still adding embedding overhead.

## Recommended Checks

Inspect cost metrics:

```bash
curl -s http://localhost:8080/metrics | grep '_micro_usd'
```

Important metrics:

```text
aif_model_cost_micro_usd_total
aif_gross_saved_micro_usd_total
aif_embedding_overhead_micro_usd_total
aif_net_saved_micro_usd_total
```

Check whether traffic is realistic:

repeated identical prompts should mostly exercise exact cache
similar but non-identical prompts are needed to exercise semantic cache
short test runs may not produce representative savings ratios
local dummy providers may not reflect real provider pricing behavior

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

AI Cost Firewall v0.2.0 supports OpenAI-compatible provider patterns.

The expected configuration model is:

```text
upstream_provider openai_compatible;
embedding_provider openai_compatible;
```

This means AI Cost Firewall expects OpenAI-style chat and embedding APIs.

It does not claim universal compatibility with every OpenAI-like API implementation. Some runtimes and gateways may differ in request format, response format, streaming behavior, model naming, authentication, or embedding support.

Native Anthropic, Gemini, Mistral, Cohere, and other provider-specific APIs are not directly supported in v0.2.0. They may be used only through an OpenAI-compatible compatibility layer such as LiteLLM, OpenRouter, or another gateway.

Provider-specific configuration blocks, provider-specific request transformations, fallback chains, and native provider pricing catalogs are intentionally postponed until after v0.2.0.

## OpenAI

Reference implementation.

## Ollama

Use OpenAI-compatible mode.

Recommended:

```text
http://ollama:11434/v1
```

Pull models before testing.

## LM Studio

Verify that embeddings are enabled.

## vLLM

Check timeout configuration for large models.

## LiteLLM

Useful aggregation layer for multiple providers.

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
