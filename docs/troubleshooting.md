# Troubleshooting

AI Cost Firewall is designed to fail fast during startup, expose clear runtime errors, and make cache, provider, and cost behavior observable.

This document covers common deployment and operational issues.

AI Cost Firewall supports OpenAI-compatible chat and embedding APIs through a simple configuration model. It does not provide native provider-specific API integrations or provider-specific configuration blocks.

---

AI Firewall can also orchestrate optional VCAL Security Guard, VCAL Privacy Guard, and VCAL Usage Guard modules. Guard modules are disabled by default and are not required for standalone caching deployments.

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

Runtime dependencies are initialized during normal startup. In `enforce`, Redis/Qdrant availability follows the configured cache fail-open and readiness policy. In `observe`, evaluation-only Redis/Qdrant/embedding failures do not block live traffic, while static configuration validation still applies.

The configured Qdrant vector size must still match the embedding model dimension when semantic caching is configured.

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

The `/version` endpoint returns release metadata, including the AI Cost Firewall version, release title, OpenAI-compatible compatibility model, AIF enforcement mode, and effective cache scope.

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

# Evaluation Mode Diagnostics

First confirm the active mode:

```bash
curl -s http://localhost:8080/version | jq
```

In `observe`, repeated requests are expected to continue reaching the upstream provider. Check evaluation metrics instead of production cache-hit counters:

```bash
curl -s http://localhost:8080/metrics | grep 'aif_enforcement_mode_info\|aif_evaluation_'
```

Important signals:

```text
aif_evaluation_cache_outcomes_total
aif_evaluation_upstream_calls_avoided_total
aif_evaluation_tokens_avoided_total
aif_evaluation_gross_saved_micro_usd_total
aif_evaluation_net_saved_micro_usd_total
aif_evaluation_shadow_store_total
aif_evaluation_errors_total
```

If Redis or Qdrant is restarted during observe mode, AIF should continue serving live traffic and remain ready with respect to those evaluation-only dependencies. After the dependency is healthy again, evaluation lookup/store should resume automatically. If it does not, inspect `aif_evaluation_errors_total` and the firewall logs for reconnect/recovery messages.

Do not use production cache-hit or production savings counters as proof of Evaluation Mode success; shadow outcomes are intentionally isolated.

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

# Security Guard Blocks or Errors

## Symptoms

Requests return `HTTP 403` or an error type similar to:

```text
security_request_blocked
security_response_blocked
security_guard_unavailable
security_guard_timeout
```

## Common Causes

- Security Guard detected prompt injection, jailbreak, or system-prompt extraction text
- Security Guard is running in `enforce` mode
- Security Guard is unavailable and `guard_fail_open false`
- API key mismatch between AI Firewall and Security Guard
- wrong `security_guard_url`
- Security Guard default mode is still `detect_only` when blocking was expected

## Recommended Checks

```conf
security_guard_enabled true;
security_guard_url http://vcal-security-guard:8091;
security_guard_api_key dev-security-key;
security_guard_timeout_seconds 3;
guard_fail_open false;
```

```text
VCAL_SECURITY_GUARD_DEFAULT_MODE=enforce
```

```bash
curl http://localhost:8091/healthz
curl http://localhost:8091/readyz
curl -s http://localhost:8080/metrics | grep -E 'aif_guard_requests_total|aif_security_blocks_total'
```

---

# Usage Guard Blocks or Errors

## Symptoms

Requests return `HTTP 403`, `HTTP 502`, or `HTTP 504`, with error types such as:

```text
usage_request_blocked
usage_guard_unavailable
usage_guard_timeout
```

## Common Causes

- Usage Guard intentionally returned `block` or `escalate`
- request category is disallowed by the selected organizational policy
- Usage Guard is unavailable and `guard_fail_open false`
- API key mismatch between AI Firewall and Usage Guard
- wrong `usage_guard_url`
- wrong or missing `usage_guard_policy_id`
- Usage Guard returned an invalid response contract

## Recommended Checks

```conf
usage_guard_enabled true;
usage_guard_url http://vcal-usage-guard:8095;
usage_guard_api_key dev-usage-key;
usage_guard_mode enforce;
usage_guard_tenant_id example-tenant;
usage_guard_policy_id business-use-only;
usage_guard_timeout_seconds 3;
guard_fail_open false;
```

```bash
curl http://localhost:8095/healthz
curl http://localhost:8095/readyz
curl -s http://localhost:8095/metrics | grep vcal_usage
curl -s http://localhost:8080/metrics | grep -E 'aif_guard_requests_total|aif_usage_blocks_total'
```

A Usage Guard policy block is an intentional enforcement outcome and does not become an allow simply because `guard_fail_open` is enabled. Fail-open applies to operational guard failures.

---

# Privacy Guard Restore or Anonymization Errors

## Symptoms

Requests return errors similar to:

```text
privacy_guard_unavailable
privacy_guard_timeout
privacy_restore_failed
guard_contract_violation
```

or final responses contain placeholders such as `[EMAIL_1]` or `[IP_1]`.

## Common Causes

- Privacy Guard unavailable and `guard_fail_open false`
- API key mismatch between AI Firewall and Privacy Guard
- wrong `privacy_guard_url`
- expired or missing mapping ID
- restore disabled
- response was blocked by Security Guard before restore
- non-text content was expected to be scanned or restored

## Recommended Checks

```conf
privacy_guard_enabled true;
privacy_guard_url http://vcal-privacy-guard:8090;
privacy_guard_api_key dev-privacy-key;
privacy_guard_mode anonymize;
privacy_guard_restore_enabled true;
privacy_guard_timeout_seconds 3;
guard_fail_open false;
```

```bash
curl http://localhost:8090/healthz
curl http://localhost:8090/readyz
curl -s http://localhost:8090/metrics | grep vcal_privacy
curl -s http://localhost:8080/metrics | grep -E 'aif_guard_requests_total|aif_privacy_restore_skipped_total'
```

---

# Controlled Streaming Problems

## `stream=true` returns HTTP 422

Check whether controlled streaming is disabled:

```conf
streaming_enabled false;
```

Enable it and reload/restart with:

```conf
streaming_enabled true;
```

A deliberately disabled deployment rejects `stream=true`; ordinary JSON requests remain available.

## `stream=true` returns an upstream/502-style error before SSE begins

Controlled streaming does not expose partial provider output. AI Cost Firewall first consumes and assembles the provider stream. A malformed, truncated, failed, oversized, idle, or over-duration stream therefore returns a normal HTTP error before downstream `text/event-stream` is committed.

Check the stream metrics:

```bash
curl -s http://localhost:8080/metrics | grep -E \
  'aif_stream_(errors|upstream_errors|upstream_chunks|upstream_bytes|upstream_response_bytes)'
```

Also check AIF logs and Audit evidence for `upstream.stream.failed`.

## Provider stream exceeds the cumulative byte limit

Review:

```conf
max_stream_upstream_bytes 8M;
```

Increase it only when expected provider responses justify accepting a larger total SSE response. The limit must be greater than zero. It counts cumulative provider SSE bytes received during the request; it is not a peak-memory-buffer limit.

## Controlled stream times out after provider headers

If a provider returns SSE headers and then stops producing body chunks, AI Cost Firewall fails the request after the configured idle interval:

```conf
upstream_timeout_seconds 120;
```

The same timeout resets whenever a provider body chunk is received. Check `aif_upstream_timeouts_total`, `aif_stream_upstream_errors_total`, `aif_stream_errors_total`, logs, and `upstream.stream.failed` evidence. No partial generated content is committed to the client.

## Provider keeps sending data but never finishes

Controlled streaming has a separate 15-minute absolute provider-generation ceiling. This closes the drip-feed case where chunks arrive frequently enough to avoid the idle timeout but the stream never terminates. The request fails before downstream SSE commit and releases the upstream concurrency permit.

## Streaming feels less immediate than raw provider SSE

This is expected. Controlled streaming prioritizes response-control guarantees over token-by-token immediacy. The client receives SSE only after upstream generation, canonical assembly, response Security Guard processing, cache-store processing, Privacy restoration, and accounting are complete.

Compare:

```text
aif_stream_upstream_time_to_first_byte_seconds
aif_stream_generation_duration_seconds
aif_stream_client_time_to_first_byte_seconds
```

---

# Non-text Content Not Scanned

The current guard modules inspect text content only. Images, audio, video, and binary payloads are not scanned, anonymized, or classified by AI Firewall guard modules.

Extract text before sending it through AI Firewall if you need text-oriented guard protection for non-text assets.

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

Check the active mode first. In `observe`, normal production savings can remain at zero even when evaluation predicts large savings; use the `aif_evaluation_*` cost counters for the hypothetical result.

Check whether traffic is realistic:

- repeated identical prompts should mostly exercise exact cache in `enforce`, or would-have exact hits in `observe`
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

In `observe`, Redis/Qdrant used only for evaluation should not make readiness fail by themselves. If `/readyz` fails during an evaluation-cache outage, inspect the active mode from `/version`, upstream readiness policy, and whether another required dependency is unhealthy.

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

`request_timeout_seconds` remains a backward-compatible fallback. Prefer setting `upstream_timeout_seconds` and `embedding_timeout_seconds` explicitly.

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

## Guard Orchestration

```text
aif_guard_requests_total
aif_guard_latency_seconds
aif_security_blocks_total
aif_privacy_restore_skipped_total
aif_usage_blocks_total
```

---

---

# VCAL Audit Delivery Problems

## Audit initialization is not logged

Confirm the configuration:

```conf
audit_enabled true;
audit_url http://vcal-audit:8092;
audit_api_key replace-with-shared-audit-token;
```

Check:

```bash
docker compose logs firewall | grep -i audit
```

## Audit returns HTTP 401

The value configured in AI Firewall must exactly match `VCAL_AUDIT_API_KEY`.

Confirm that both services use the same token and that no surrounding quotes or whitespace were copied into the value.

## Audit hostname cannot be resolved

AI Firewall and VCAL Audit must share a Docker network.

From the AI Firewall container, test service resolution:

```bash
docker exec ai-firewall-firewall-1 \
  getent hosts vcal-audit
```

Distroless images may not include diagnostic tools. In that case, use a temporary BusyBox container attached to the same network.

## Batches are retried and then dropped

This means VCAL Audit remained unavailable or rejected the request through all configured attempts.

Check:

- Audit health and readiness
- Docker DNS and network membership
- API key
- request-body and maximum-batch limits
- Audit logs
- AI Firewall timeout and retry settings

The sender queue is memory-backed. Batches dropped after retry exhaustion are not replayed automatically.

## Requests succeed while Audit is unavailable

This is expected. Audit delivery is asynchronous and does not normally fail the LLM request path.

# Provider Compatibility Notes

AI Cost Firewall supports OpenAI-compatible provider patterns.

The expected configuration model is:

```conf
upstream_provider openai_compatible;
embedding_provider openai_compatible;
```

This means AI Cost Firewall expects OpenAI-style chat and embedding APIs.

It does not claim universal compatibility with every OpenAI-like API implementation. Some runtimes and gateways may differ in request format, response format, streaming behavior, model naming, authentication, or embedding support.

Native Anthropic, Gemini, Mistral, Cohere, and other provider-specific APIs are not directly supported. They may be used only through an OpenAI-compatible compatibility layer such as LiteLLM, OpenRouter, or another gateway.

Provider-specific configuration blocks, provider-specific request transformations, fallback chains, and native provider pricing catalogs remain outside the current scope.

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
