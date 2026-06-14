# Troubleshooting

AI Cost Firewall is designed to fail fast during startup, expose clear runtime errors, and make cache, provider, and cost behavior observable.

This document covers common deployment and operational issues for v0.2.x.

AI Cost Firewall v0.2.x supports OpenAI-compatible chat and embedding APIs through a simple configuration model. It does not provide native provider-specific API integrations or provider-specific configuration blocks.

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

- Redis is required when exact cache is enabled and startup validation requires Redis
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

The `/version` endpoint returns release metadata, including the AI Cost Firewall version, release title, and OpenAI-compatible compatibility model.

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

- which services are running
- which AI Cost Firewall release is active
- whether the process is alive
- whether it is ready to serve traffic
- whether metrics are exposed
- recent firewall, Redis, and Qdrant errors

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
- unsupported request-size or prompt-size value
- unsupported timeout value

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
- exact cache enabled while Redis is unavailable

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

```conf
redis_url redis://redis:6379;
exact_cache_enabled true;
exact_cache_fail_open true;
```

When `exact_cache_fail_open` is enabled, runtime Redis lookup/store failures behave like cache misses and requests continue upstream. This does not mean a broken Redis configuration should be ignored for production; it only controls runtime failure handling.

---

# Exact Cache Disabled or Not Producing Hits

## Symptoms

- repeated identical requests still go upstream
- exact hit panels stay flat
- only semantic hits or misses increase

## Common Causes

- exact cache disabled
- exact cache store disabled
- Redis unavailable
- requests differ after normalization
- cache TTL too short

## Recommended Checks

Inspect configuration:

```conf
exact_cache_enabled true;
exact_cache_store_enabled true;
exact_cache_fail_open true;
exact_cache_ttl_seconds 86400;
```

Inspect metrics:

```bash
curl -s http://localhost:8080/metrics | grep 'aif_cache_hits_total\|aif_cache_misses_total'
```

Important metrics:

```text
aif_cache_hits_total{cache_type="exact"}
aif_cache_misses_total
```

---

# Exact Cache Fail-Open Behavior

## Symptoms

Requests continue successfully even though Redis lookup or store errors appear in logs.

Or, when fail-open is disabled, Redis-related errors may fail the request.

## Cause

AI Cost Firewall can be configured to fail open for runtime exact-cache failures.

```conf
exact_cache_fail_open true;
```

When enabled:

- Redis lookup failures behave like cache misses
- Redis store failures do not fail the request
- requests continue upstream

When disabled:

- Redis lookup/store failures may return an error

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
- using HTTP REST port instead of gRPC port in AI Firewall config
- incorrect hostname

## Recommended Checks

Check Qdrant:

```bash
docker compose ps
```

Verify port:

```text
6334 = gRPC
6333 = HTTP REST
```

Recommended configuration:

```conf
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

```conf
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

```conf
upstream_base_url http://ollama:11434/v1/chat/completions;
```

## Correct

```conf
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
- embedding provider timeout too low

## Ollama Example

Pull embedding model:

```bash
docker compose exec ollama ollama pull nomic-embed-text
```

Restart firewall:

```bash
docker compose restart firewall
```

## Timeout Configuration

```conf
embedding_timeout_seconds 30;
```

If omitted, `request_timeout_seconds` is used as the fallback.

---

# Semantic Cache Fail-Open Behavior

## Symptoms

Requests continue successfully even though semantic cache lookup, embedding, or semantic store errors appear in logs.

Or, when fail-open is disabled, requests fail with semantic cache or embedding-related errors.

## Cause

AI Cost Firewall can be configured to fail open for runtime semantic cache failures.

When enabled:

```conf
semantic_cache_fail_open true;
```

runtime semantic cache failures do not block the request. AI Cost Firewall skips the semantic cache path and continues to the upstream LLM endpoint.

When disabled:

```conf
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
- semantic cache store disabled
- semantic cache disabled
- entries expired too quickly

## Recommended Checks

Inspect configuration:

```conf
semantic_cache_enabled true;
semantic_cache_store_enabled true;
semantic_similarity_threshold 0.92;
semantic_cache_retention_seconds 604800;
```

Inspect metrics:

```bash
curl -s http://localhost:8080/metrics | grep semantic
```

Important metrics:

```text
aif_semantic_candidates_checked_total
aif_semantic_threshold_results_total
aif_cache_hits_total{cache_type="semantic"}
aif_semantic_store_total
aif_semantic_store_errors_total
```

## Recommended Thresholds

Typical starting point:

```conf
semantic_similarity_threshold 0.92;
```

Lower threshold:

- more semantic reuse
- higher risk of incorrect matches

Higher threshold:

- stricter matching
- fewer semantic hits

---

# Cache Bypass Requests/sec Shows 0

## Symptoms

The Grafana Overview panel for cache bypass remains at zero.

## Common Causes

- no requests were sent with the bypass header
- the bypass metric is not present in the running binary
- Prometheus has not scraped the new metric yet
- the dashboard is connected to an old container/image

## Recommended Checks

Check whether the metric exists:

```bash
curl -s http://localhost:8080/metrics | grep aif_cache_bypass_requests_total
```

Send a bypass request:

```bash
curl -s http://localhost:8080/v1/chat/completions \
  -H "Content-Type: application/json" \
  -H "Authorization: Bearer demo-key" \
  -H "X-AIF-Cache-Bypass: true" \
  -d '{"model":"gpt-4o-mini-2024-07-18","messages":[{"role":"user","content":"Manual bypass metric test"}],"temperature":0}'
```

Check the metric again:

```bash
curl -s http://localhost:8080/metrics | grep aif_cache_bypass_requests_total
```

## Configuration

Default:

```conf
cache_bypass_header X-AIF-Cache-Bypass;
```

Accepted truthy values include:

```text
true
1
yes
on
```

When bypass is enabled for a request:

- exact cache lookup is skipped
- semantic cache lookup is skipped
- exact cache storage is skipped
- semantic cache storage is skipped

---

# Grafana Dashboards Are Empty

## Symptoms

- dashboards load
- graphs remain empty

## Common Causes

- no traffic generated
- Prometheus cannot scrape firewall
- metrics endpoint unavailable or protected
- wrong provisioning paths
- Grafana is still using a persistent old dashboard definition

## Recommended Checks

Verify metrics:

```bash
curl http://localhost:8080/metrics
```

If metrics auth is enabled:

```bash
curl -H "Authorization: Bearer your-prometheus-token" \
  http://localhost:8080/metrics
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

# Metrics Endpoint Returns 401 or 403

## Symptoms

```text
/metrics returns unauthorized
```

or Prometheus target is down after enabling metrics authentication.

## Cause

Metrics endpoint access control is enabled.

```conf
metrics_auth_required true;
metrics_auth_token your-prometheus-token;
```

## Recommended Checks

Query with bearer token:

```bash
curl -H "Authorization: Bearer your-prometheus-token" \
  http://localhost:8080/metrics
```

Update Prometheus scrape configuration to send the same bearer token, or disable metrics auth on a private Docker network:

```conf
metrics_auth_required false;
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

- repeated identical prompts should mostly exercise exact cache
- similar but non-identical prompts are needed to exercise semantic cache
- short test runs may not produce representative savings ratios
- local dummy providers may not reflect real provider pricing behavior

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
- Redis unavailable and readiness requires Redis
- Qdrant unavailable and readiness requires Qdrant
- upstream unavailable and readiness requires upstream
- graceful shutdown in progress

Readiness dependency behavior can be configured:

```conf
readiness_requires_redis true;
readiness_requires_qdrant false;
readiness_requires_upstream false;
```

This allows deployments to decide which dependency failures should remove the firewall from service.

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

Configure split timeouts:

```conf
request_timeout_seconds 120;
upstream_timeout_seconds 120;
embedding_timeout_seconds 30;
```

`request_timeout_seconds` remains a backward-compatible fallback. In v0.2.x, prefer setting `upstream_timeout_seconds` and `embedding_timeout_seconds` explicitly.

Inspect latency metrics:

```text
aif_upstream_request_duration_seconds
aif_embedding_request_duration_seconds
```

---

# Large Requests or Prompts Rejected

## Symptoms

```json
{
  "error": {
    "code": 413
  }
}
```

or a validation error for prompt size.

## Cause

The request exceeds configured limits.

## Request Body Limit

```conf
max_request_body_bytes 1M;
```

Supported formats:

```text
512K
1M
2M
```

## Prompt Character Limit

```conf
max_prompt_chars 200000;
```

`max_request_body_bytes` limits the full HTTP request body. `max_prompt_chars` limits parsed chat message content.

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
docker compose logs firewall > firewall.log
```

---

# Useful Metrics for Debugging

## Cache Behavior

```text
aif_cache_hits_total{cache_type="exact"}
aif_cache_hits_total{cache_type="semantic"}
aif_cache_misses_total
aif_cache_bypass_requests_total
```

## Semantic Diagnostics

```text
aif_semantic_candidates_checked_total
aif_semantic_threshold_results_total
aif_semantic_lookup_duration_seconds
aif_semantic_expired_entries_skipped_total
aif_semantic_store_total
aif_semantic_store_errors_total
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

AI Cost Firewall v0.2.x supports OpenAI-compatible provider patterns.

The expected configuration model is:

```conf
upstream_provider openai_compatible;
embedding_provider openai_compatible;
```

This means AI Cost Firewall expects OpenAI-style chat and embedding APIs.

It does not claim universal compatibility with every OpenAI-like API implementation. Some runtimes and gateways may differ in request format, response format, streaming behavior, model naming, authentication, or embedding support.

Native Anthropic, Gemini, Mistral, Cohere, and other provider-specific APIs are not directly supported in v0.2.x. They may be used only through an OpenAI-compatible compatibility layer such as LiteLLM, OpenRouter, or another gateway.

Provider-specific configuration blocks, provider-specific request transformations, fallback chains, and native provider pricing catalogs remain outside the v0.2.x scope.

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

```conf
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
