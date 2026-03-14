# AI Cost Firewall — FAQ

## What is AI Cost Firewall?

AI Cost Firewall is an **OpenAI-compatible API gateway** that sits between client applications and LLM providers.
It reduces cost and latency by caching responses and avoiding unnecessary API calls.

The firewall behaves similarly to **nginx for LLM APIs**, forwarding requests when necessary and serving cached responses when possible.

---

## How does caching work?

AI Cost Firewall uses two caching layers:

1. Exact cache (Redis / Valkey) 
   Stores responses for identical requests using a normalized request hash.

2. Semantic cache (Qdrant)  
   Uses embeddings to detect semantically similar requests and reuse previous responses.

Request flow:

```text
Client → AI Cost Firewall → Redis → Qdrant → Upstream LLM
```

Only cache misses reach the upstream provider.

---

## Which endpoints are supported?

Currently the firewall supports:

```
/v1/chat/completions
```

The endpoint is **OpenAI-compatible**, allowing existing OpenAI SDKs to work without modification.

Future versions may add support for additional endpoints.

---

## What metrics are exposed?

Prometheus metrics are available at:

```
/metrics
```

Key metrics include:

- `aif_requests_total`
- `aif_cache_exact_hits`
- `aif_cache_semantic_hits`
- `aif_cache_misses`
- `aif_upstream_calls`
- `aif_tokens_saved`
- `aif_cost_saved_micro_usd`

These metrics can be visualized using **Grafana dashboards**.

---

## How are token and cost savings calculated?

Token and cost savings are currently calculated **only for chat-completion responses**.

The following values are used:

- `prompt_tokens`
- `completion_tokens`
- configured `model_price` values

Example configuration:

```
model_price gpt-4o-mini-2024-07-18 0.15 0.60;
```

This defines:

- input token price (USD per 1M tokens)
- output token price (USD per 1M tokens)

Embedding requests used internally for semantic caching are **not included in cost accounting** in the current version.

---

## Why does Total Cost Saved show zero?

In v0.1.0, `model_price` matching is exact.

If the upstream API returns a versioned model name such as:

```text
gpt-4o-mini-2024-07-18
```

the same name must appear in the configuration:

```text
model_price gpt-4o-mini-2024-07-18 0.15 0.60;
```

If the names do not match exactly, cost savings cannot be calculated and `aif_cost_saved_micro_usd` will remain zero.

---

## Do I need both Redis and Qdrant?

No.

Minimum setup:

- Redis (or a Redis-compatible server such as Valkey)
- AI Cost Firewall

Optional:

- Qdrant for semantic caching.

If semantic caching is disabled, the firewall still works using exact request caching.

---

## Can the firewall work with providers other than OpenAI?

Yes.

Any provider exposing an **OpenAI-compatible API** can work with the firewall.

Examples include:

- OpenAI
- Azure OpenAI
- local OpenAI-compatible gateways

You only need to configure:

```
upstream_base_url
upstream_api_key
```

---


## Which Qdrant port should be used?

AI Cost Firewall uses the **Qdrant gRPC** interface, which runs on port:

```text
6334
```

The REST API port (`6333`) is not used by the firewall.

Example configuration:

```text
qdrant_url http://qdrant:6334;
```

---

## Does the firewall modify requests or responses?

No.

The firewall:

- forwards requests unchanged
- returns responses unchanged

It only performs:

- request normalization for hashing
- caching
- metrics collection

---

## Is streaming supported?

Yes, but streaming responses are **not cached**.

Streaming requests are forwarded directly to the upstream provider.

---

## Can the configuration be validated before starting the server?

Yes.

AI Cost Firewall provides a configuration validation command similar to `nginx -t`.

Example:

```bash
cargo run -- --config configs/ai-firewall.conf --test-config
```

Expected output:

```text
configuration OK
runtime dependencies initialized successfully
```

This command verifies configuration syntax and runtime dependencies without starting the HTTP server.

---

## Can the configuration be reloaded without restarting the service?

Yes.

AI Cost Firewall supports **nginx-style hot reload**.

Reload configuration:

```
kill -HUP <firewall_pid>
```

The service will reload configuration without dropping connections.

---

## Is AI Cost Firewall production-ready?

The project is currently in **MVP stage**, but designed with production architecture:

- Rust async runtime
- Redis-compatible caching
- vector search with Qdrant
- Prometheus observability
- Grafana dashboards
- Docker deployment

Future versions will expand features and provider support.

---

## Why is the semantic cache not being used?

Semantic caching requires all of the following components to be correctly configured:
- Qdrant running and reachable
- embedding API configured
- semantic caching enabled

Minimum required configuration:

```text
embedding_base_url https://api.openai.com;
embedding_api_key sk-xxxx;
embedding_model text-embedding-3-small;

qdrant_url http://qdrant:6334;
qdrant_collection aif_semantic_cache;
qdrant_vector_size 1536;

semantic_cache_enabled true;
semantic_similarity_threshold 0.92;
```

If any of these are missing, semantic caching will be disabled and only exact request caching will be used.

You can confirm semantic cache activity using Prometheus metrics:

```text
aif_cache_semantic_hits
```

If this value remains zero, the semantic cache is not being triggered.

---

## Why does the firewall fail to connect to Redis or Qdrant?

Connection errors usually occur when the service hostname is incorrect.

When running via **Docker Compose**, services must be addressed using their **service names**, not `localhost`.

Correct configuration:

```text
redis_url redis://redis:6379;
qdrant_url http://qdrant:6334;
```

Incorrect configuration (common mistake):

```text
redis_url redis://127.0.0.1:6379;
qdrant_url http://127.0.0.1:6334;
```

Inside Docker containers, `localhost` refers to the container itself, not other services.

Using the correct service names ensures the firewall can reach Redis and Qdrant through the Docker network.

---

## Why do cached responses still call the upstream API?

A request is served from cache only if it matches an existing cached entry.

Common reasons the upstream API is still called:

- Prompt changed – even small differences in text or message history create a new cache key.
- Request parameters differ – values like `model`, `temperature`, `top_p`, or `max_tokens` are part of the cache key.
- First request – the initial request must reach the upstream provider before it can be cached.
- Semantic similarity too low – when semantic caching is enabled, prompts must exceed the configured similarity threshold (e.g. `0.92`).
- Streaming requests – responses with `stream=true` are not cached.

You can monitor cache behavior using Prometheus metrics:

```text
aif_cache_exact_hits
aif_cache_semantic_hits
aif_cache_misses
```

---

## Where can I learn more?

Source code and documentation:

https://github.com/vcal-project/ai-firewall
