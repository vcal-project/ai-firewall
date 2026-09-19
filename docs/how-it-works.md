# How AI Cost Firewall Works

This document explains how AI Cost Firewall processes requests, optionally orchestrates enterprise guard modules, evaluates cache reuse, communicates with OpenAI-compatible providers, and reduces LLM API cost and latency.

AI Cost Firewall sits between client applications and LLM providers and applies a two-layer cache strategy:

1. exact cache (Redis)
2. semantic cache (Qdrant)

Cache misses and explicit cache-bypass requests are forwarded upstream.

---

# High-Level Architecture

```text
Client Applications
        │
        ▼
AI Cost Firewall
        │
        ├── VCAL Security Guard (optional)
        ├── VCAL Privacy Guard (optional)
        ├── VCAL Usage Guard (optional)
        ├── VCAL Audit (optional evidence delivery)
        ├── Redis (exact cache)
        ├── Qdrant (semantic cache)
        │
        ▼
OpenAI-compatible provider
```

Supported OpenAI-compatible providers include:

- OpenAI
- Ollama
- LM Studio
- vLLM
- LiteLLM
- OpenRouter

---

# Request Lifecycle

AI Cost Firewall evaluates requests in stages:

1. parse and validate request limits
2. normalize request and derive transport-independent cache identity
3. call Security Guard request scan, if enabled
4. call Privacy Guard scan/anonymize/redact, if enabled
5. call Usage Guard policy evaluation, if enabled
6. evaluate per-request cache bypass
7. exact cache lookup, if enabled
8. semantic cache lookup, if enabled and not bypassed
9. on a miss or bypass, call the upstream provider; for `stream=true`, consume provider SSE internally
10. normalize cache hits and upstream results into the canonical chat-completion response
11. call Security Guard response scan, if enabled
12. store eligible cache-miss responses before Privacy restoration, if store controls allow it
13. call Privacy Guard restore, if enabled and mapping exists
14. record usage/cost and emit terminal evidence; enqueue Audit delivery, if enabled
15. return JSON or replay the approved response as OpenAI-compatible SSE

---

# Request Flow Diagram

```mermaid
flowchart TD

    A[Client Request]

    A0{Request limits OK?}

    B[Normalize Request]

    B0{Cache bypass header?}

    C{Exact Cache Enabled?}

    C1{Exact Cache Lookup<br/>Redis}

    D[Return Cached Response]

    E{Semantic Cache Enabled?}

    E1[Generate Embedding]

    F{Semantic Search<br/>Qdrant}

    G{Candidate Valid?<br/>similarity + freshness}

    H[Forward to OpenAI-compatible Upstream]

    I[Receive Upstream Response]

    J{Exact Store Enabled?}

    J1[Store Exact Cache]

    K{Semantic Store Enabled?}

    K1[Store Semantic Cache]

    L[Return Response]

    A --> A0
    A0 -->|NO| L
    A0 -->|YES| B

    B --> B0

    B0 -->|YES| H
    B0 -->|NO| C

    C -->|NO| E
    C -->|YES| C1

    C1 -->|HIT| D
    D --> L

    C1 -->|MISS| E

    E -->|NO| H
    E -->|YES| E1

    E1 --> F
    F --> G

    G -->|YES| D
    G -->|NO| H

    H --> I

    I --> J
    J -->|YES| J1
    J -->|NO| K
    J1 --> K

    K -->|YES| K1
    K -->|NO| L
    K1 --> L
```

---

# Guard Orchestration Flow

AI Firewall can run as a guard orchestrator in addition to a cache gateway.

When VCAL Security Guard, VCAL Privacy Guard, and VCAL Usage Guard are enabled, the flow is:

```text
Client request
→ AI Firewall validates request limits
→ Security Guard scans request text
→ block with HTTP 403 if Security Guard rejects the request
→ Privacy Guard anonymizes or redacts sensitive text
→ Usage Guard evaluates organizational usage policy
→ continue on allow/warn; stop on block/escalate
→ exact/semantic cache lookup or upstream request
→ Security Guard scans assistant response text
→ block response if Security Guard rejects the output
→ Privacy Guard restores placeholders, if a mapping exists
→ AI Firewall returns final response
```

Usage Guard is request-side only in the current AI Firewall integration and runs after Privacy Guard, so anonymized text is evaluated when Privacy Guard modifies the request.

---
# Example Request

Client applications use the standard OpenAI-compatible API.

Example:

```bash
curl http://localhost:8080/v1/chat/completions \
  -H "Content-Type: application/json" \
  -d '{
    "model": "gpt-4o-mini-2024-07-18",
    "messages": [
      {"role":"user","content":"Explain Redis briefly"}
    ]
  }'
```

The firewall returns a standard OpenAI-compatible response.

No client-side API changes are required.

---

# Client Authentication Notes

By default, AI Cost Firewall does not require incoming client authentication.

The configured:

```text
upstream_api_key
```

is used only for upstream provider communication.

For production environments, place the firewall behind:

- authenticated reverse proxy
- API gateway
- VPN
- private network boundary
- service mesh

---

# Request Limits and Cache Bypass

Before cache lookup, AI Cost Firewall validates basic gateway limits.

Relevant settings:

```conf
max_request_body_bytes 1048576;
max_prompt_chars 200000;
```

`max_request_body_bytes` limits the full HTTP request body. `max_prompt_chars` limits the total chat message content after parsing.

AI Cost Firewall also supports per-request cache bypass:

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

When bypass is enabled for a request:

- exact cache lookup is skipped
- semantic cache lookup is skipped
- exact cache storage is skipped
- semantic cache storage is skipped
- the request is forwarded upstream

Bypass requests are counted with:

```text
aif_cache_bypass_requests_total
```

---

# Step 1 — Request Normalization

Before cache lookup, the firewall normalizes requests.

Normalization removes non-deterministic request fields and produces stable semantic text.

Example normalized prompt:

```text
user: Explain Redis briefly
```

With system message:

```text
system: You are a concise assistant
user: Explain Redis briefly
```

Normalization is used for:

- exact cache hashing
- semantic embeddings
- semantic search consistency

---

# Step 2 — Exact Cache Lookup

Exact cache is controlled by:

```conf
exact_cache_enabled true;
exact_cache_fail_open true;
```

When `exact_cache_enabled` is disabled, Redis exact-cache lookup and storage are skipped and the request continues to semantic lookup or upstream forwarding.

When `exact_cache_fail_open` is enabled, runtime Redis lookup failures behave like cache misses and requests continue upstream.

The firewall creates a SHA256 hash from the normalized request JSON.

Example key:

```text
aif:exact:<sha256>
```

Redis lookup:

```text
GET aif:exact:<sha256>
```

---

# Exact Cache Hit

If the response already exists in Redis:

```text
Redis → HIT
```

The firewall immediately returns the cached response.

Benefits:

- near-zero latency
- no upstream API call
- no additional token usage
- predictable response times

---

# Exact Cache Miss

If Redis does not contain the request:

```text
Redis → MISS
```

the firewall continues to semantic lookup.

---

# Step 3 — Embedding Generation

The firewall generates an embedding from the normalized semantic text.

Embeddings are produced through the configured embedding provider.

Embedding calls use:

```conf
embedding_timeout_seconds 30;
```

If omitted, `request_timeout_seconds` is used as the fallback.

The embedding provider may differ from the chat provider.

Examples:

| Chat Provider | Embedding Provider |
|---|---|
| OpenAI | OpenAI |
| OpenAI | Ollama |
| OpenRouter | OpenAI |
| Ollama | Ollama |

---

# Embedding Models

Typical embedding models:

| Model | Vector Size |
|---|---|
| text-embedding-3-small | 1536 |
| nomic-embed-text | 768 |

Example configuration:

```conf
embedding_model text-embedding-3-small;
qdrant_vector_size 1536;
```

The vector size must match the embedding model dimension.

---

# Step 4 — Semantic Search

The generated embedding is used to query Qdrant.

Typical collection:

```text
aif_semantic_cache
```

Qdrant returns semantically similar candidates.

Each candidate contains:

- embedding vector
- cached response payload
- similarity score
- inserted_at
- expires_at
- model metadata

---

# Semantic Candidate Evaluation

AI Cost Firewall validates semantic candidates before reuse.

A candidate is reusable only if:

```text
similarity_score >= semantic_similarity_threshold
AND
expires_at > now
AND
cached response payload is valid
```

---

# Semantic Threshold Example

Example:

```text
Prompt: "Explain Redis briefly"
Cached Prompt: "What is Redis used for?"
Similarity: 0.94
Threshold: 0.92
```

Because:

```text
0.94 >= 0.92
```

the candidate may be reused.

---

# Semantic Cache Hit

When a valid semantic candidate exists:

```text
semantic HIT
```

the firewall returns the cached response.

Benefits:

- reduced upstream API cost
- reduced latency
- reuse of semantically equivalent answers
- higher effective cache utilization

---

# Semantic Cache Miss

Semantic miss occurs when:

- no candidates returned
- similarity below threshold
- entry expired
- payload invalid
- semantic lookup failure occurs

The firewall then forwards the request upstream.

---

# Semantic Cache Lifecycle

Semantic entries contain lifecycle metadata:

- inserted_at
- expires_at

Expiration derives from:

```conf
semantic_cache_retention_seconds 604800;
```

Expired entries:

- are filtered during lookup
- are never reused
- may remain stored until pruned

Semantic correctness does not depend on cleanup.

---

# Runtime Fail-Open Behavior

AI Cost Firewall supports separate runtime fail-open controls for exact and semantic cache paths:

```conf
exact_cache_fail_open true;
semantic_cache_fail_open true;
```

When enabled:

- Redis exact-cache lookup/store failures can behave like cache misses or skipped stores
- semantic lookup failures behave like cache misses
- requests continue upstream normally

This prevents cache infrastructure failures from blocking chat traffic when fail-open behavior is desired.

Fail-open settings do not bypass static configuration validation.

---

# Step 5 — Upstream Request

When both cache layers miss, or when cache bypass is requested, the firewall forwards the request upstream.

Example providers:

```text
https://api.openai.com
http://ollama:11434/v1
http://vllm:8000/v1
```

AI Cost Firewall internally appends:

```text
/v1/chat/completions
```

Chat-completion upstream calls use:

```conf
upstream_timeout_seconds 120;
```

If omitted, `request_timeout_seconds` is used as the fallback.

Do not configure the full endpoint path as:

```text
upstream_base_url
```

---

# Correct vs Wrong Base URLs

## Correct

```text
http://ollama:11434/v1
```

## Wrong

```text
http://ollama:11434/v1/chat/completions
```

---

# Local Provider Authentication

Local providers often do not require authentication.

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

When placeholders are used, AI Cost Firewall does not send:

```text
Authorization: Bearer ...
```

headers upstream.

---

# Step 6 — Cache Storage

After receiving and normalizing the upstream response, the firewall can store results in one or both cache layers. Controlled-streaming cache misses are first assembled into the same canonical response used by the non-streaming path.

Cache writes are controlled independently:

```conf
exact_cache_store_enabled true;
semantic_cache_store_enabled true;
```

When a store control is disabled, lookup may still be enabled, but new upstream responses are not written to that cache layer.

Cache storage is skipped for requests that use the cache bypass header.

When Privacy Guard restoration is enabled, eligible responses are stored using the pre-restore representation and restored only after the response-control/cache-store stage. This preserves the existing cache semantics while keeping sensitive original values out of cache storage.

---

# Exact Cache Storage

When `exact_cache_store_enabled` is enabled, Redis stores:

```text
exact request → response
```

Example:

```text
SET aif:exact:<sha256> response
```

---

# Semantic Cache Storage

When `semantic_cache_store_enabled` is enabled, Qdrant stores:

- normalized prompt text
- embedding vector
- response payload
- inserted_at
- expires_at
- model metadata

---

# Step 7 — Return Response

The firewall returns a standard OpenAI-compatible response to the client.

For `stream=false` or an omitted `stream` field, the response is normal JSON. For `stream=true`, the approved canonical response is encoded as OpenAI-compatible SSE and terminated with `[DONE]`.

Applications do not need special cache-awareness logic.

---

---

# Controlled Streaming Observability

Key metrics include:

```text
aif_stream_requests_total
aif_stream_completed_total
aif_stream_errors_total
aif_stream_aborted_total
aif_stream_upstream_errors_total
aif_stream_upstream_chunks_total
aif_stream_upstream_bytes_total
aif_stream_upstream_time_to_first_byte_seconds
aif_stream_generation_duration_seconds
aif_stream_client_time_to_first_byte_seconds
aif_stream_upstream_response_bytes
aif_stream_client_buffer_bytes
aif_stream_duration_seconds
```

Upstream stream completion/failure is also represented in structured evidence as `upstream.stream.completed` or `upstream.stream.failed`. Failed controlled streams record that content was not committed to the client.

---

# Evidence and Audit Delivery

AI Cost Firewall emits structured evidence throughout the request lifecycle.

Evidence uses:

```text
schema: vcal.evidence.event
schema_version: 1.1
```

A stable `trace_id` correlates validation, guard, cache, upstream, response, and terminal events.

For each trace that emits `request.received`, AI Firewall emits exactly one terminal event:

```text
request.completed
```

or:

```text
request.failed
```

When Audit delivery is enabled, events are enqueued without synchronously blocking the request path. The sender groups events by batch size or flush interval and posts them to:

```text
{audit_url}/v1/events/batch
```

Failed deliveries are retried with configurable backoff. The queue is bounded and memory-backed, so producer-side delivery does not guarantee durable replay after process termination or retry exhaustion.

# Streaming Behavior

AI Cost Firewall supports **controlled streaming** rather than raw provider-SSE passthrough.

The normal JSON path, controlled provider-stream path, and eligible cache-hit path converge on the same canonical response processing:

```text
ordinary upstream JSON ───────────────┐
assembled upstream stream ────────────┼─► canonical ChatCompletionResponse
eligible exact / semantic cache hit ──┘
                                              │
                                              ▼
                                      response Security Guard
                                      eligible cache store
                                      (cache miss only, pre-restore)
                                      Privacy Guard restore
                                      usage / cost accounting
                                      metrics / evidence
                                              │
                                 ┌────────────┴────────────┐
                                 ▼                         ▼
                          JSON serializer             SSE encoder / replay
                          stream=false                stream=true
                                                       → [DONE]
```

Because cache identity is transport-independent, JSON and SSE delivery can reuse the same eligible cached completion.

No model-generated response content is sent to the client until the provider stream has completed, the response has been assembled, and all enabled response controls have approved it.

If the provider stream fails, is malformed, is truncated, exceeds `max_stream_upstream_bytes`, becomes idle for longer than `upstream_timeout_seconds`, or exceeds the 15-minute absolute generation ceiling, AI Cost Firewall returns a normal HTTP error before downstream SSE commit. Partial provider content is not exposed.

Configuration:

```conf
streaming_enabled true;
max_stream_upstream_bytes 8M;
upstream_timeout_seconds 120;
```

`max_stream_upstream_bytes` counts cumulative provider SSE bytes for the request; it is not an instantaneous in-memory buffer measurement.

`upstream_timeout_seconds` is also the controlled-stream idle timeout after provider headers arrive. A separate 15-minute absolute generation ceiling closes the drip-feed case where chunks continue to arrive without the generation terminating.

When `stream_options.include_usage` is requested by the client, controlled SSE replay includes the usage chunk. AI Cost Firewall requests upstream usage data for its own accounting even when the client does not request the downstream usage chunk.

Controlled streaming reconstructs fragmented content and tool-call/function arguments into the canonical response before re-encoding SSE. The downstream stream is semantically OpenAI-compatible but is not intended to preserve the provider's byte-for-byte event framing.

---

# Structured Outputs and Tools

Semantic cache may also be skipped for:

- tool-calling requests
- function-calling requests
- structured response formats

These request types often contain non-deterministic structures that reduce safe semantic reuse.

---

# When Cache Layers Are Disabled

Semantic cache may be disabled globally:

```conf
semantic_cache_enabled false;
```

In this mode:

- exact cache can still be used
- semantic embeddings are skipped
- Qdrant is not required for request processing

Exact cache may also be disabled:

```conf
exact_cache_enabled false;
```

In this mode:

- Redis exact-cache lookup and storage are skipped
- semantic cache can still be used if enabled
- all cache can be disabled when both exact and semantic cache are disabled

---

# Non-text Content

The current guard modules inspect text content only.

Non-text content such as images, audio, video, and binary payloads is preserved where possible but is not scanned, anonymized, or classified by AI Firewall guard modules.

Text extracted by the client application, such as OCR text or captions, can be scanned and anonymized normally if it is sent as text content.

---
# Metrics and Observability

AI Cost Firewall exposes Prometheus metrics:

```text
/metrics
```

Example:

```bash
curl http://localhost:8080/metrics
```

The metrics endpoint can be protected:

```conf
metrics_auth_required true;
metrics_auth_token your-prometheus-token;
```

When enabled, clients must send:

```http
Authorization: Bearer your-prometheus-token
```

---

# Core Request Metrics

```text
aif_requests_total
aif_cache_hits_total{cache_type="exact"}
aif_cache_hits_total{cache_type="semantic"}
aif_cache_misses
aif_cache_bypass_requests_total
aif_upstream_calls_total
```

These metrics show whether requests were:

- exact cache hits
- semantic cache hits
- cache misses
- explicit cache-bypass requests
- upstream requests

---

# Semantic Diagnostics Metrics

```text
aif_semantic_candidates_checked_total
aif_semantic_threshold_results_total
aif_semantic_lookup_duration_seconds
aif_semantic_expired_entries_skipped_total
aif_semantic_store_total
aif_semantic_store_errors_total
```

These metrics help tune:

- semantic similarity thresholds
- retention windows
- embedding quality
- semantic reuse behavior

---

# Timeout Metrics

```text
aif_upstream_timeouts_total
aif_embedding_timeouts_total
aif_upstream_request_duration_seconds
aif_embedding_request_duration_seconds
```

Useful for diagnosing:

- slow providers
- overloaded local models
- embedding bottlenecks
- network latency

---

# Cost Metrics

```text
aif_model_cost_micro_usd_total
aif_gross_saved_micro_usd_total
aif_embedding_overhead_micro_usd_total
aif_net_saved_micro_usd_total
```

These metrics distinguish:

- gross avoided chat-completion cost
- embedding overhead
- net savings after embeddings

---

# Guard Metrics

```text
aif_guard_requests_total
aif_guard_latency_seconds
aif_security_blocks_total
aif_privacy_restore_skipped_total
aif_usage_blocks_total
```

---
# Included Dashboards

AI Cost Firewall includes pre-configured Grafana dashboards in the Docker deployment files.

Overview dashboard:

- total request volume
- estimated chat cost
- gross and net savings
- embedding overhead
- cache hit rate
- exact and semantic cache activity
- cache bypass request rate
- per-model spend and savings
- Usage Guard policy blocks
- high-level guard orchestration health

Diagnostics dashboard:

- readiness state
- semantic threshold pass/fail behavior
- semantic candidate evaluation
- expired entry skips
- semantic lookup latency
- upstream and embedding latency
- semantic store health
- runtime and provider pressure signals
- provider error classes

---

# Why Semantic Cache Matters

Exact cache alone only reuses identical prompts.

Semantic cache enables reuse across:

- paraphrased questions
- slightly modified prompts
- repeated support requests
- repeated agent interactions
- recurring enterprise workflows

This significantly improves effective cache utilization.

---

# Typical Deployment Patterns

AI Cost Firewall supports:

| Pattern | Example |
|---|---|
| Cloud upstream | OpenAI |
| Fully local | Ollama |
| Hybrid | OpenAI + local embeddings |
| Routing layer | OpenRouter |
| Self-hosted inference | vLLM |

Deployment examples:

```text
deploy/examples/
```

---

# Operational Notes

- Redis is used for exact cache when `exact_cache_enabled` is true
- Redis startup/runtime behavior depends on `exact_cache_fail_open` and readiness settings
- Qdrant is required for semantic cache when semantic cache is enabled and fail-open behavior does not allow startup without it
- vector size must match embedding dimension
- semantic correctness does not depend on pruning
- expired entries are filtered during lookup
- exact and semantic cache writes can be controlled independently
- per-request cache bypass skips lookup and store
- OpenAI-compatible providers are supported through a flat configuration model

---

# Additional Documentation

See also:

- `docs/quickstart.md`
- `docs/config-reference.md`
- `docs/provider-compatibility.md`
- `docs/operation.md`
- `docs/troubleshooting.md`
