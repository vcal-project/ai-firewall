# AI Cost Firewall Architecture

AI Cost Firewall is designed as a lightweight LLM infrastructure component. It acts as a smart gateway between client applications and OpenAI-compatible APIs, providing caching, cost accounting, and observability. Instead of applications calling LLM providers directly, they send requests through the firewall.

The firewall reduces **API costs, latency, and token usage** by caching responses using both **exact matching** and **semantic similarity**.

The firewall combines **Redis / Valkey exact caching** and **Qdrant semantic caching** to maximize cache hit rates while maintaining response quality.

---

# High-Level Architecture

```mermaid
flowchart LR
    Client[Client Application / SDK]

    Firewall[AI Cost Firewall<br/>Rust + Axum Gateway]

    Redis[Redis / Valkey<br/>Exact Cache]
    Qdrant[Qdrant<br/>Semantic Cache]

    Upstream[Chat Upstream<br/>OpenAI-compatible]
    Embedding[Embedding Provider<br/>OpenAI-compatible]

    Prom[Prometheus]
    Graf[Grafana]

    Client --> Firewall

    Firewall --> Redis
    Firewall --> Qdrant
    Firewall --> Upstream
    Firewall --> Embedding

    Firewall --> Prom
    Prom --> Graf
```

AI Cost Firewall sits between client applications and LLM providers, reducing latency and cost through exact and semantic caching while exporting Prometheus metrics for observability.

The chat-completion upstream and embedding provider can use the same base URL or different OpenAI-compatible base URLs.

---

# Core Design Principles

AI Cost Firewall is built around three goals:

### Cost Reduction

Repeated prompts are served from cache, avoiding expensive API calls.

### Latency Reduction

Cached responses are returned instantly without contacting upstream providers.

### Observability

Prometheus metrics provide insight into:

- cache hit rates
- token savings
- API usage patterns

### Provider Compatibility

AI Cost Firewall uses a flat `openai_compatible` provider model so deployments can use OpenAI, local gateways, or self-hosted model servers without provider-specific config blocks.

---

# Components

## AI Cost Firewall

The firewall is a **Rust-based HTTP gateway** built with **Axum**.

Responsibilities include:

- request normalization
- cache lookup
- semantic similarity checks
- upstream API forwarding
- cache storage
- metrics generation

The firewall exposes an **OpenAI-compatible API**, allowing existing
applications to use it without modification.

Example endpoint:

```text
POST /v1/chat/completions
```

---

## Redis / Valkey (Exact Cache)

Redis stores cached responses using an **exact hash of the normalized
request**.

Example cache key:

```bash
aif:exact:<sha256>
```


Benefits:

- constant-time lookup
- extremely fast responses
- zero embedding cost

---

## Qdrant (Semantic Cache)

Qdrant stores embeddings of normalized prompt text in a **vector index**.

Embeddings are requested from the configured OpenAI-compatible embedding provider. The embedding provider may be the same service as the chat-completion upstream, or a separate endpoint configured through `embedding_base_url`.

Embeddings are generated using the configured embedding model (e.g. `text-embedding-3-small`) and stored in the Qdrant vector index.

When an exact match is not found, the firewall performs a **semantic similarity search**.

If a similar prompt is found above the configured similarity threshold, the cached response is returned.

Typical thresholds:

```text
0.85 aggressive caching
0.92 balanced
0.97 strict matching
```

---

## OpenAI-Compatible Chat Upstream

If no cached response is found, the firewall forwards the request to the
configured upstream provider.

Supported upstreams include OpenAI and practical OpenAI-compatible providers such as Ollama, LM Studio, vLLM, LiteLLM, and local or self-hosted gateways.

The configured `upstream_base_url` may be either the provider root URL or its `/v1` base path. AI Cost Firewall builds the final `/v1/chat/completions` endpoint internally.

For local providers without authentication, placeholder API keys such as `dummy`, `none`, `null`, or `-` can be used.

---

## OpenAI-Compatible Embedding Provider

Semantic caching requires embeddings for normalized prompt text.

The embedding provider is configured separately from the chat upstream:

```text
embedding_provider openai_compatible;
embedding_base_url <base-url>;
embedding_api_key <key-or-placeholder>;
embedding_model <embedding-model>;
```

This allows deployments where chat completions are served by one provider and embeddings are served by another.

Examples:

```text
upstream_base_url http://ollama:11434/v1;
embedding_base_url https://api.openai.com;
```



---

## Prometheus Metrics

The firewall exports metrics for monitoring and observability.

Example metrics include:

```bash
aif_requests_total
aif_upstream_calls_total
aif_upstream_request_duration_seconds
aif_upstream_timeouts_total
aif_embedding_request_duration_seconds
aif_embedding_timeouts_total
aif_cache_exact_hits
aif_cache_semantic_hits
aif_cache_misses
aif_tokens_saved
aif_chat_cost_saved_micro_usd
aif_embedding_cost_micro_usd
aif_cost_saved_micro_usd
aif_semantic_store_total
aif_semantic_store_errors_total
```

### Token and Cost Accounting

AI Cost Firewall calculates savings for cached chat-completion responses.

The following metrics track gross and net savings:

- `aif_tokens_saved`
- `aif_chat_cost_saved_micro_usd`
- `aif_embedding_cost_micro_usd`
- `aif_cost_saved_micro_usd`

For semantic cache hits, gross savings are based on avoided chat-completion tokens. Embedding lookup cost is deducted when `embedding_price` is configured, so `aif_cost_saved_micro_usd` represents net savings.

If `embedding_price` is not configured, embedding cost is treated as zero and savings may be overestimated.

---

## Grafana

Grafana can visualize Prometheus metrics using dashboards.

Typical dashboards include:

- request throughput
- cache hit ratios
- token savings
- estimated cost savings

---

# Two-Layer Cache Strategy

AI Cost Firewall uses a **two-stage caching strategy**.

### Stage 1 — Exact Cache (Redis / Valkey)

The firewall first checks Redis for an exact match.

```bash
hash(normalized request) -> cached response
```

If a hit is found, the response is returned immediately.

---

### Stage 2 — Semantic Cache (Qdrant)

If no exact match is found, the firewall searches the Qdrant vector database for **similar prompts**.

If similarity exceeds the configured threshold:

```bash
similar_prompt → cached_response
```

the cached response is returned.

---

### Stage 3 — Upstream Request

If neither cache contains a match, the firewall forwards the request to the upstream LLM provider.

The result is then stored in both caches.

---

# Request Flow

## Exact Cache Hit (Redis / Valkey)

```mermaid
flowchart LR

    Client[Client Application]
    Firewall[AI Cost Firewall]
    Redis[Redis / Valkey<br/>Exact Cache]

    Client --> Firewall
    Firewall --> Redis
    Redis -->|Exact hit| Firewall
    Firewall --> Client
```

## Semantic Cache Hit (Qdrant)

```mermaid
flowchart LR

    Client[Client Application]
    Firewall[AI Cost Firewall]
    Redis[Redis / Valkey<br/>Exact Cache]
    Qdrant[Qdrant<br/>Semantic Cache]

    Client --> Firewall
    Firewall --> Redis
    Redis -->|Miss| Firewall

    Firewall --> Qdrant
    Qdrant -->|Semantic hit| Firewall

    Firewall --> Client
```
## Cache Miss (Upstream LLM)

```mermaid
flowchart LR

    Client[Client Application]
    Firewall[AI Cost Firewall]
    Redis[Redis / Valkey<br/>Exact Cache]
    Qdrant[Qdrant<br/>Semantic Cache]
    Upstream[Chat Upstream<br/>OpenAI-compatible]

    Client --> Firewall

    Firewall --> Redis
    Redis -->|Miss| Firewall

    Firewall --> Qdrant
    Qdrant -->|Miss| Firewall

    Firewall --> Upstream
    Upstream -->|Response| Firewall

    Firewall -->|Store exact response| Redis
    Firewall -->|Store embedding + response| Qdrant

    Firewall --> Client
```

---

# Expected Performance

AI Cost Firewall is designed to introduce minimal overhead to LLM API requests.

Typical performance characteristics on a small cloud instance (4 vCPU):

| Scenario | Approx throughput | Typical latency |
|--------|--------|--------|
| Exact cache hit | 5k–20k req/sec | 1–3 ms |
| Semantic cache hit | 50–300 req/sec | 50–150 ms |
| Upstream request | depends on LLM | 0.5–5 s |

In most deployments the firewall is not the bottleneck; latency and throughput are dominated by embedding providers, vector search, and upstream chat model latency.

Actual performance depends on infrastructure, network latency, and model latency.

---

## Important Disclaimer

These values are architectural estimates and may vary by deployment.

---

# Summary

AI Cost Firewall provides a lightweight **LLM caching gateway** that:

- reduces LLM API costs
- improves latency
- provides observability
- integrates with existing OpenAI-compatible clients

By combining exact caching (Redis) and semantic caching (Qdrant), the firewall maximizes cache hit rates while maintaining response quality.
