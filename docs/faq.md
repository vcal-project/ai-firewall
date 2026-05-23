
# AI Cost Firewall — FAQ

## What is AI Cost Firewall?

AI Cost Firewall is an OpenAI-compatible gateway for reducing LLM API cost and latency.

It sits between applications and LLM providers and uses:

- exact cache (Redis / Valkey)
- semantic cache (Qdrant)

to avoid unnecessary upstream requests.

Only cache misses are forwarded upstream.

The firewall behaves similarly to:

```text
nginx for LLM APIs
```

---

## How does caching work?

AI Cost Firewall uses a two-layer cache strategy.

### Exact Cache

Redis stores responses for identical normalized requests.

Example flow:

```text
request hash → Redis → cached response
```

---

### Semantic Cache

Qdrant stores embeddings of normalized prompt text.

If prompts are semantically similar, cached responses may be reused even when prompts are not identical.

Example flow:

```text
prompt embedding → Qdrant similarity search → cached response
```

---

## What is the request flow?

Typical request flow:

```text
Client
→ AI Cost Firewall
→ Redis exact cache
→ Qdrant semantic cache
→ OpenAI-compatible upstream
```

Only cache misses reach upstream providers.

---

## Which endpoints are currently supported?

Currently supported:

```text
/v1/chat/completions
```

The API is OpenAI-compatible, allowing existing SDKs and applications to work without modification.

---

## Which providers are supported?

AI Cost Firewall supports practical OpenAI-compatible providers including:

- OpenAI
- Ollama
- LM Studio
- vLLM
- LiteLLM
- OpenRouter

The firewall uses a flat provider model without provider-specific configuration blocks.

---

## Can the chat provider and embedding provider differ?

Yes.

Examples:

```text
upstream_base_url https://api.openai.com;
embedding_base_url http://ollama:11434/v1;
```

or:

```text
upstream_base_url http://ollama:11434/v1;
embedding_base_url https://api.openai.com;
```

This is useful for:

- reducing embedding cost
- local semantic caching
- hybrid deployments

---

## What metrics are exposed?

Prometheus metrics are available at:

```text
/metrics
```

Example metrics:

```text
aif_requests_total
aif_cache_exact_hits
aif_cache_semantic_hits
aif_cache_misses
aif_upstream_calls_total
aif_model_cost_micro_usd_total
aif_gross_saved_micro_usd_total
aif_embedding_overhead_micro_usd_total
aif_net_saved_micro_usd_total
```

AI Cost Firewall also exports:

- semantic diagnostics
- runtime health metrics
- timeout metrics
- embedding metrics

---

## How are cost savings calculated?

Cost savings are calculated for cached chat-completion responses.

Inputs include:

- prompt tokens
- completion tokens
- configured `model_price`
- optional `embedding_price`

---

## What is gross vs net savings?

### Gross Savings

Avoided chat-completion cost.

Metric:

```text
aif_gross_saved_micro_usd_total
```

---

### Embedding Overhead

Embedding generation cost for semantic lookup.

Metric:

```text
aif_embedding_overhead_micro_usd_total
```

---

### Net Savings

Gross savings minus embedding overhead.

Metric:

```text
aif_net_saved_micro_usd_total
```

---

## Why do cost metrics show zero?

Most common reason:

```text
model_price
```

does not exactly match the upstream model name.

Example upstream model:

```text
gpt-4o-mini-2024-07-18
```

Matching configuration required:

```text
model_price gpt-4o-mini-2024-07-18 0.15 0.60;
```

---

## Do I need both Redis and Qdrant?

No.

Minimum deployment:

- AI Cost Firewall
- Redis / Valkey

Qdrant is required only when semantic caching is enabled.

---

## Can semantic cache be disabled?

Yes.

Example:

```conf
semantic_cache_enabled false;
```

When disabled:

- embeddings skipped
- Qdrant not required
- exact cache still active

---

## Which Qdrant port should be used?

AI Cost Firewall uses Qdrant gRPC on:

```text
6334
```

REST API typically runs on:

```text
6333
```

Example:

```conf
qdrant_url http://qdrant:6334;
```

---

## Why does startup fail with vector-size mismatch?

The embedding dimension must match:

```conf
qdrant_vector_size
```

Example:

| Embedding Model | Vector Size |
|---|---|
| text-embedding-3-small | 1536 |
| nomic-embed-text | 768 |

Mismatch example:

```text
existing collection vector size does not match qdrant_vector_size
```

---

## What API key should be used for local providers?

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

When placeholders are used, Authorization headers are not forwarded upstream.

---

## Why do I get upstream_not_found?

Usually means the provider returned:

```text
404
```

Most common cause:

wrong base URL.

---

## Correct

```text
http://ollama:11434/v1
```

---

## Wrong

```text
http://ollama:11434/v1/chat/completions
```

AI Cost Firewall automatically appends OpenAI-compatible endpoint paths internally.

---

## Why do I get upstream_tls_error?

TLS verification failed.

Typical causes:

- self-signed certificate
- hostname mismatch
- invalid SAN
- corporate TLS interception

For trusted local networks, local providers often work more reliably using:

```text
http://
```

---

## Is streaming supported?

Yes.

Streaming requests are forwarded upstream normally.

Current behavior:

- streaming responses are not stored in semantic cache
- semantic cache may be bypassed for streaming requests

---

## Are tool-calling and structured outputs cached?

Semantic cache may be skipped for:

- tool-calling requests
- function-calling requests
- structured outputs

because these request types often contain non-deterministic structures.

---

## Can configuration be validated before startup?

Yes.

AI Cost Firewall supports validation similar to:

```text
nginx -t
```

Example:

```bash
cargo run -- --config configs/ai-firewall.conf --test-config
```

Expected:

```text
configuration OK
```

Static validation checks:

- syntax
- required directives
- semantic cache configuration
- request-size parsing
- model validation rules

It does not contact runtime dependencies.

---

## Can the loaded configuration be inspected?

Yes.

Example:

```bash
cargo run -- --config configs/ai-firewall.conf --print-config
```

Sensitive values are masked automatically.

---

## Can configuration be reloaded without restart?

Yes.

AI Cost Firewall supports nginx-style reloads using:

```text
SIGHUP
```

Docker Compose:

```bash
docker compose kill -s HUP firewall
```

Binary deployment:

```bash
kill -HUP $(pgrep ai-firewall)
```

---

## What happens during graceful shutdown?

Shutdown sequence:

1. readiness disabled
2. new requests rejected
3. in-flight requests continue
4. process exits after timeout

Configured by:

```conf
graceful_shutdown_timeout_seconds 10;
```

---

## Why is semantic cache not producing hits?

Common causes:

- prompts insufficiently similar
- threshold too strict
- embeddings unavailable
- semantic cache disabled
- entries expired

Inspect metrics:

```text
aif_cache_semantic_hits
aif_semantic_candidates_checked_total
aif_semantic_threshold_results_total
```

Typical starting threshold:

```conf
semantic_similarity_threshold 0.92;
```

---

## Why are requests still reaching upstream providers?

Common causes:

- first request not cached yet
- prompt changed
- parameters changed
- semantic similarity below threshold
- streaming enabled

Important request fields include:

- model
- temperature
- top_p
- max_tokens

---

## Why can’t the firewall connect to Redis or Qdrant?

Inside Docker Compose:

```text
localhost
```

refers to the container itself.

Use Docker service names instead.

---

## Correct

```conf
redis_url redis://redis:6379;
qdrant_url http://qdrant:6334;
```

---

## Wrong

```conf
redis_url redis://127.0.0.1:6379;
qdrant_url http://127.0.0.1:6334;
```

---

## Is AI Cost Firewall production-ready?

AI Cost Firewall is in an early production-ready stage.

The project includes:

- Rust async runtime
- Redis exact cache
- Qdrant semantic cache
- Prometheus metrics
- Grafana dashboards
- graceful shutdown
- readiness handling
- configuration reload
- runtime diagnostics

v0.1.x releases focus on:

- operational polish
- provider compatibility
- observability
- deployment simplicity

---

## What deployment examples are included?

Ready-to-run deployment examples are available under:

```text
deploy/examples/
```

Included patterns:

```text
openai-cloud/
local-ollama/
hybrid-openai-local-embeddings/
openrouter/
local-full-stack/
```

Each example includes:

- docker-compose deployment
- minimal configuration
- example requests
- expected behavior
- expected metrics

---

## Where can I learn more?

Documentation:

```text
docs/
```

Important documents:

- `docs/quickstart.md`
- `docs/config-reference.md`
- `docs/provider-compatibility.md`
- `docs/operation.md`
- `docs/troubleshooting.md`

Source code:

```text
https://github.com/vcal-project/ai-firewall
```
