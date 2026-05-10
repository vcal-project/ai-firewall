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
Client → AI Cost Firewall → Redis → Qdrant → OpenAI-compatible upstream
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
- `aif_upstream_calls_total`
- `aif_tokens_saved`
- `aif_cost_saved_micro_usd`
- `aif_semantic_store_total`
- `aif_semantic_store_errors_total`
- `aif_embedding_request_duration_seconds`
- `aif_embedding_timeouts_total`


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

Embedding lookup cost can be included in net savings when `embedding_price` is configured.

Related metrics:

- `aif_chat_cost_saved_micro_usd` — gross avoided chat-completion cost
- `aif_embedding_cost_micro_usd` — embedding lookup cost
- `aif_cost_saved_micro_usd` — net savings after embedding cost

If `embedding_price` is not configured, embedding cost is treated as zero and savings may be overestimated.

---

## Why does Total Cost Saved show zero?

`model_price` matching is exact.

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

AI Cost Firewall supports practical OpenAI-compatible upstream and embedding endpoints while keeping the flat provider model.

Examples include:

- OpenAI
- Ollama OpenAI-compatible endpoint
- LM Studio
- vLLM
- LiteLLM
- other local or self-hosted OpenAI-compatible gateways

Configure the upstream provider with:

```text
upstream_provider openai_compatible;
upstream_base_url <base-url>;
upstream_api_key <key-or-placeholder>;
```

The base URL may be either the provider root URL or its /v1 base path:

```text
https://api.openai.com
http://ollama:11434
http://ollama:11434/v1
http://lmstudio:1234/v1
http://vllm:8000/v1
```

Do not configure the full endpoint path:

```text
# Wrong
upstream_base_url http://ollama:11434/v1/chat/completions;

# Correct
upstream_base_url http://ollama:11434/v1;
```

---

## What API key should I use for local providers?

For local OpenAI-compatible providers that do not require authentication, use a placeholder key:

```text
upstream_api_key dummy;
embedding_api_key dummy;
```

Accepted placeholder values are:

```text
dummy
none
null
-
```

When a placeholder key is used, AI Cost Firewall does not send the upstream `Authorization: Bearer ...` header.

---

## Can the chat model and embedding model use different providers?

Yes.

The main model upstream and embedding provider can use different base URLs:

```text
upstream_base_url http://ollama:11434/v1;
embedding_base_url https://api.openai.com;
```

This is useful when chat completions are served locally but embeddings are provided by another OpenAI-compatible service.

---

## Why do I get an upstream_not_found error?

`upstream_not_found` usually means the provider returned `404`.

Common causes:

- `upstream_base_url` points to the wrong host or port
- a full endpoint path was configured instead of a base URL
- the provider does not expose an OpenAI-compatible `/v1/chat/completions` endpoint

Correct:

```text
upstream_base_url http://ollama:11434/v1;
```

Wrong:

```text
upstream_base_url http://ollama:11434/v1/chat/completions;
```

---

## Why do I get an upstream_tls_error?

`upstream_tls_error` means TLS certificate verification failed.

Common causes:

- self-signed certificate
- certificate hostname does not match the configured host
- missing or invalid Subject Alternative Name
- corporate proxy or gateway replacing certificates

Fix options:

- use a trusted certificate
- configure the provider URL with a hostname that matches the certificate
- use HTTP only inside a trusted private network for local testing

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

- forwards compatible chat-completion requests to the configured upstream
- returns OpenAI-compatible responses to the client

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
```

This command validates the configuration file only. It checks syntax, required directives, value ranges, semantic cache settings, and model validation configuration.

It does not connect to Redis, Qdrant, embedding providers, or upstream LLM providers.

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

AI Cost Firewall is in an early production-ready stage.

It is designed with production-grade components:

- Rust async runtime
- Redis exact cache
- Qdrant semantic cache
- Prometheus + Grafana observability
- Docker deployment
- graceful shutdown and readiness checks

Recent releases improved:

- runtime stability and error handling
- observability and diagnostics
- semantic cache lifecycle control (expiration and pruning)

The system is stable to run, with further improvements planned.

---

## Why is the semantic cache not being used?

Most common reasons:

### 1. Not configured

Semantic cache requires:

```text
semantic_cache_enabled true;

embedding_base_url https://api.openai.com;
embedding_api_key sk-xxxx;
embedding_model text-embedding-3-small;

qdrant_url http://qdrant:6334;
qdrant_collection aif_semantic_cache;
qdrant_vector_size 1536;
```

### 2. No similar requests

Semantic cache only works if **similar prompts repeat**.

### 3. Threshold too strict

```text
semantic_similarity_threshold 0.92
```

Higher → fewer matches.

### 4. Entries expired

Expired entries are skipped automatically.

If retention is too short:

```text
semantic_cache_retention_seconds
```

→ no reuse happens.

### Quick check

```text
aif_cache_semantic_hits
```

If this stays `0`, semantic cache is not being used.

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
