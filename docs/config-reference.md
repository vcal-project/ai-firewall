
# Configuration Reference

AI Cost Firewall uses a simple nginx-style configuration format.

Each directive contains:

- directive name
- value
- terminating semicolon

Example:

```conf
listen_addr 0.0.0.0:8080;
```

Configuration directives are:

- case-sensitive
- semicolon-terminated
- validated during startup and reload

---

# Configuration Philosophy

AI Cost Firewall intentionally uses a flat OpenAI-compatible configuration model.

The same structure works across:

- OpenAI
- Ollama
- LM Studio
- vLLM
- LiteLLM
- OpenRouter

No provider-specific configuration blocks are required.

---

# Configuration Sections

The configuration is logically divided into:

| Section | Purpose |
|---|---|
| Core Settings | Runtime and server behavior |
| Upstream Provider | Chat-completion provider |
| Embedding Provider | Embedding generation |
| Qdrant | Semantic cache storage |
| Cache Settings | TTL and retention |
| Exact Cache | Redis exact-cache behavior |
| Semantic Cache | Semantic lookup behavior |
| Request Limits | Body size, prompt size, and timeout controls |
| Per-request Controls | Cache bypass behavior |
| Model Pricing | Cost tracking |
| Metrics & Observability | Prometheus metrics and metrics endpoint access |
| Guard Orchestration | Optional VCAL Security Guard and VCAL Privacy Guard integration |
| Readiness Behavior | Dependency-aware readiness checks |
| Operational Settings | Shutdown, reload, and maintenance behavior |

---

# Minimal Configuration Example

```conf
listen_addr 0.0.0.0:8080;

redis_url redis://redis:6379;

upstream_provider openai_compatible;
upstream_base_url https://api.openai.com;
upstream_api_key sk-your-key;

embedding_provider openai_compatible;
embedding_base_url https://api.openai.com;
embedding_api_key sk-your-key;
embedding_model text-embedding-3-small;

qdrant_url http://qdrant:6334;
qdrant_collection aif_semantic_cache;
qdrant_vector_size 1536;

cache_ttl_seconds 86400;

exact_cache_enabled true;
exact_cache_fail_open true;
exact_cache_store_enabled true;

semantic_cache_enabled true;
semantic_similarity_threshold 0.92;
semantic_cache_fail_open true;
semantic_cache_store_enabled true;

request_timeout_seconds 120;
upstream_timeout_seconds 120;
embedding_timeout_seconds 30;

max_request_body_bytes 1M;
max_prompt_chars 200000;

cache_bypass_header X-AIF-Cache-Bypass;

# Optional enterprise guard orchestration.
security_guard_enabled false;
# security_guard_url http://vcal-security-guard:8091;
# security_guard_api_key replace-with-security-guard-key;
security_guard_timeout_seconds 3;

privacy_guard_enabled false;
# privacy_guard_url http://vcal-privacy-guard:8090;
# privacy_guard_api_key replace-with-privacy-guard-key;
privacy_guard_mode anonymize;
privacy_guard_restore_enabled true;
privacy_guard_timeout_seconds 3;

guard_fail_open false;

metrics_auth_required false;
# metrics_auth_token replace-with-prometheus-token;

readiness_requires_redis true;
readiness_requires_qdrant false;
readiness_requires_upstream false;

graceful_shutdown_timeout_seconds 10;

model_price gpt-4o-mini-2024-07-18 0.15 0.60;
embedding_price 0.020;
```

---

# Deployment Examples

Ready-to-run examples are included under:

```text
deploy/examples/
```

Available deployment patterns:

| Example | Description |
|---|---|
| `openai-cloud/` | OpenAI cloud deployment |
| `local-ollama/` | Fully local Ollama |
| `hybrid-openai-local-embeddings/` | OpenAI chat + local embeddings |
| `openrouter/` | OpenRouter upstream |
| `local-full-stack/` | Full observability stack |

---

# OpenAI-Compatible Provider Model

AI Cost Firewall supports OpenAI-compatible APIs through a flat provider abstraction.

## Chat Provider

```conf
upstream_provider openai_compatible;
upstream_base_url <base-url>;
upstream_api_key <key>;
```

## Embedding Provider

```conf
embedding_provider openai_compatible;
embedding_base_url <base-url>;
embedding_api_key <key>;
```

The upstream provider and embedding provider may use different endpoints.

---

# Base URL Rules

The provider URL may be:

- provider root URL
- `/v1` base path

---

# Correct Examples

```text
https://api.openai.com
https://api.openai.com/v1
http://ollama:11434
http://ollama:11434/v1
http://vllm:8000/v1
http://litellm:4000/v1
```

---

# Wrong Examples

Do not configure full endpoint paths.

Wrong:

```text
http://ollama:11434/v1/chat/completions
```

Wrong:

```text
http://ollama:11434/v1/embeddings
```

AI Cost Firewall appends OpenAI-compatible endpoint paths internally.

---

# Placeholder API Keys

Local providers may not require authentication.

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

When placeholders are used:

```text
Authorization: Bearer ...
```

headers are not sent upstream.

---

# Core Settings

---

## listen_addr

HTTP server listen address.

Example:

```conf
listen_addr 0.0.0.0:8080;
```

Typical values:

```text
0.0.0.0:8080
127.0.0.1:8080
```

---

## redis_url

Redis-compatible connection string.

Used for:

- exact cache
- cache coordination

Example:

```conf
redis_url redis://redis:6379;
```

Redis-compatible systems:

- Redis
- Valkey

---

# Upstream Provider Settings

These directives configure the chat-completion provider.

---

## upstream_provider

Currently supported:

```conf
upstream_provider openai_compatible;
```

---

## upstream_base_url

Base URL of the chat provider.

Example:

```conf
upstream_base_url https://api.openai.com;
```

Local example:

```conf
upstream_base_url http://ollama:11434/v1;
```

---

## upstream_api_key

API key used for upstream authentication.

Example:

```conf
upstream_api_key sk-your-key;
```

Local providers may use placeholders:

```conf
upstream_api_key dummy;
```

---

# Embedding Provider Settings

Required when semantic caching is enabled.

---

## embedding_provider

Currently supported:

```conf
embedding_provider openai_compatible;
```

---

## embedding_base_url

Base URL of the embedding provider.

Example:

```conf
embedding_base_url https://api.openai.com;
```

Local example:

```conf
embedding_base_url http://ollama:11434/v1;
```

---

## embedding_api_key

API key used for embedding requests.

Example:

```conf
embedding_api_key sk-your-key;
```

---

## embedding_model

Embedding model used for semantic vectors.

Example:

```conf
embedding_model text-embedding-3-small;
```

Typical models:

| Model | Dimensions |
|---|---|
| text-embedding-3-small | 1536 |
| nomic-embed-text | 768 |

---

# Qdrant Settings

These directives configure semantic cache storage.

---

## qdrant_url

Qdrant endpoint.

Example:

```conf
qdrant_url http://qdrant:6334;
```

Notes:

- port `6334` = gRPC
- port `6333` = REST API

AI Cost Firewall uses gRPC internally.

---

## qdrant_api_key

Optional Qdrant authentication.

Example:

```conf
qdrant_api_key your-qdrant-key;
```

---

## qdrant_collection

Collection used for semantic cache.

Example:

```conf
qdrant_collection aif_semantic_cache;
```

---

## qdrant_vector_size

Embedding vector dimension.

Example:

```conf
qdrant_vector_size 1536;
```

Must match:

```conf
embedding_model
```

dimension.

---

# Vector Size Validation

If the Qdrant collection already exists, AI Cost Firewall validates:

```conf
qdrant_vector_size
```

against the existing collection dimension.

Mismatch example:

```text
existing collection vector size does not match qdrant_vector_size
```

---

# Cache Settings

---

## cache_ttl_seconds

Backward-compatible default TTL.

Example:

```conf
cache_ttl_seconds 86400;
```

Typical values:

| Seconds | Meaning |
|---|---|
| 3600 | 1 hour |
| 86400 | 1 day |
| 604800 | 7 days |
| 2592000 | 30 days |

---

## exact_cache_ttl_seconds

TTL for Redis exact cache entries.

Overrides:

```conf
cache_ttl_seconds
```

for exact cache only.

Example:

```conf
exact_cache_ttl_seconds 86400;
```

---

## semantic_cache_retention_seconds

Retention window for semantic cache entries.

Example:

```conf
semantic_cache_retention_seconds 604800;
```

Semantic entries include:

- inserted_at
- expires_at

Expired entries:

- skipped during lookup
- never reused
- not automatically deleted

---

# Semantic Cache Cleanup

Remove expired semantic entries:

```bash
ai-firewall --prune-expired-semantic-cache
```

Docker Compose example:

```bash
docker compose run --rm firewall \
  --config /configs/ai-firewall.conf \
  --prune-expired-semantic-cache
```

Recommended during maintenance windows.

---

# Exact Cache Settings

---

## exact_cache_enabled

Enable Redis-backed exact cache lookup and storage.

Example:

```conf
exact_cache_enabled true;
```

Default:

```text
true
```

When disabled:

- Redis exact-cache lookup is skipped
- exact-cache storage is skipped
- semantic cache can still be used if enabled

---

## exact_cache_fail_open

Controls runtime Redis/exact-cache failure behavior.

Example:

```conf
exact_cache_fail_open true;
```

When enabled:

- Redis lookup failures behave like cache misses
- Redis store failures do not fail the request
- requests continue upstream

When disabled:

- Redis lookup or store failures can return an error

This setting controls runtime request behavior. Startup and readiness behavior are controlled separately.

---

## exact_cache_store_enabled

Controls whether successful upstream responses are stored in exact cache.

Example:

```conf
exact_cache_store_enabled true;
```

When disabled:

- exact-cache reads may still happen
- exact-cache writes are skipped

Useful for read-only cache testing and debugging.

---

# Semantic Cache Settings

---

## semantic_cache_enabled

Enable semantic caching.

Example:

```conf
semantic_cache_enabled true;
```

Default:

```text
true
```

When disabled:

- embeddings skipped
- Qdrant not required
- exact cache still active

---

## semantic_cache_fail_open

Controls runtime semantic lookup behavior.

Example:

```conf
semantic_cache_fail_open true;
```

When enabled:

- semantic lookup failures behave like cache misses
- requests continue upstream

Does not bypass startup validation.

---

## semantic_cache_store_enabled

Controls whether successful upstream responses are stored in semantic cache.

Example:

```conf
semantic_cache_store_enabled true;
```

When disabled:

- semantic lookup can still be used
- embedding and store work for new semantic entries is skipped
- Qdrant writes are avoided

Useful for read-only semantic-cache testing and debugging.

---

## semantic_similarity_threshold

Similarity threshold for semantic reuse.

Example:

```conf
semantic_similarity_threshold 0.92;
```

Typical values:

| Threshold | Behavior |
|---|---|
| 0.85 | Aggressive reuse |
| 0.92 | Balanced |
| 0.97 | Strict reuse |

Lower values:

- higher semantic hit rate
- higher mismatch risk

Higher values:

- lower mismatch risk
- fewer semantic hits

---

# Request Limits

---

## max_request_body_bytes

Maximum allowed HTTP request body size.

Example:

```conf
max_request_body_bytes 1M;
```

Supported formats:

```text
1024
512K
1M
2M
```

Behavior:

- oversized requests are rejected early
- accidental upstream cost spikes are reduced
- runtime stability is protected

---

## max_prompt_chars

Maximum total prompt size across chat message content, measured in characters.

Example:

```conf
max_prompt_chars 200000;
```

Behavior:

- oversized prompts are rejected before upstream forwarding
- accidental huge prompts are blocked even when the HTTP body is otherwise valid
- complements `max_request_body_bytes`

`max_request_body_bytes` limits the full HTTP request body. `max_prompt_chars` limits parsed chat message content.

---

## request_timeout_seconds

Backward-compatible fallback timeout used when more specific timeout values are not configured.

Specific timeout settings override it:

- `upstream_timeout_seconds`
- `embedding_timeout_seconds`

Example:

```conf
request_timeout_seconds 120;
```

Default:

```text
120
```

In v0.3.x, prefer configuring `upstream_timeout_seconds` and `embedding_timeout_seconds` explicitly.

---

## upstream_timeout_seconds

Timeout for chat-completion upstream calls.

Example:

```conf
upstream_timeout_seconds 120;
```

If omitted, `request_timeout_seconds` is used as the fallback.

---

## embedding_timeout_seconds

Timeout for embedding provider calls.

Example:

```conf
embedding_timeout_seconds 30;
```

If omitted, `request_timeout_seconds` is used as the fallback.

---

# Per-request Cache Controls

---

## cache_bypass_header

Header used to bypass cache lookup and cache storage for a single request.

Example:

```conf
cache_bypass_header X-AIF-Cache-Bypass;
```

Default:

```text
X-AIF-Cache-Bypass
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

When enabled for a request:

- exact cache lookup is skipped
- semantic cache lookup is skipped
- exact cache storage is skipped
- semantic cache storage is skipped

Metric:

```text
aif_cache_bypass_requests_total
```

---

# Guard Orchestration

AI Firewall v0.3.0 can optionally orchestrate VCAL Security Guard and VCAL Privacy Guard.

Supported modes:

```text
AI Firewall only
AI Firewall + VCAL Security Guard
AI Firewall + VCAL Privacy Guard
AI Firewall + VCAL Security Guard + VCAL Privacy Guard
```

Recommended full enterprise order:

```text
Security Guard request scan
→ Privacy Guard scan/anonymize/redact
→ exact/semantic cache lookup or upstream LLM
→ Security Guard response scan
→ Privacy Guard restore
```

## security_guard_enabled

Enable VCAL Security Guard orchestration.

```conf
security_guard_enabled true;
```

Default: `false`.

## security_guard_url

Base URL for VCAL Security Guard.

```conf
security_guard_url http://vcal-security-guard:8091;
```

Do not include `/v1/scan`; AI Firewall appends the endpoint internally.

## security_guard_api_key

Service-to-service API key for VCAL Security Guard.

```conf
security_guard_api_key your-security-guard-key;
```

## security_guard_timeout_seconds

Timeout for Security Guard calls.

```conf
security_guard_timeout_seconds 3;
```

## privacy_guard_enabled

Enable VCAL Privacy Guard orchestration.

```conf
privacy_guard_enabled true;
```

Default: `false`.

## privacy_guard_url

Base URL for VCAL Privacy Guard.

```conf
privacy_guard_url http://vcal-privacy-guard:8090;
```

Do not include `/v1/scan` or `/v1/restore`; AI Firewall appends endpoints internally.

## privacy_guard_api_key

Service-to-service API key for VCAL Privacy Guard.

```conf
privacy_guard_api_key your-privacy-guard-key;
```

## privacy_guard_mode

Privacy Guard scan mode.

```conf
privacy_guard_mode anonymize;
```

Common values are `detect_only`, `redact`, and `anonymize`.

## privacy_guard_restore_enabled

Enable placeholder restoration on assistant responses.

```conf
privacy_guard_restore_enabled true;
```

## privacy_guard_timeout_seconds

Timeout for Privacy Guard scan and restore calls.

```conf
privacy_guard_timeout_seconds 3;
```

## guard_fail_open

Controls what AI Firewall does when an enabled guard is unavailable, times out, or returns an invalid contract.

```conf
guard_fail_open false;
```

For security-sensitive or privacy-sensitive deployments, fail-closed is recommended.

## Full enterprise example

```conf
security_guard_enabled true;
security_guard_url http://vcal-security-guard:8091;
security_guard_api_key dev-security-key;
security_guard_timeout_seconds 3;

privacy_guard_enabled true;
privacy_guard_url http://vcal-privacy-guard:8090;
privacy_guard_api_key dev-privacy-key;
privacy_guard_mode anonymize;
privacy_guard_restore_enabled true;
privacy_guard_timeout_seconds 3;

guard_fail_open false;
```

Security Guard should normally run in enforce mode for production-like AI Firewall tests:

```text
VCAL_SECURITY_GUARD_DEFAULT_MODE=enforce
```

---
# Model Validation

By default:

- only configured models allowed
- unknown models rejected

Example:

```conf
model_price gpt-4o-mini-2024-07-18 0.15 0.60;
```

---

## allow_unknown_models_pass_through

Optional proxy-style mode.

Example:

```conf
allow_unknown_models_pass_through true;
```

When enabled:

- unknown models forwarded upstream
- cost tracking unavailable for unknown models

Useful for:

- OpenRouter
- proxy deployments
- rapidly changing model catalogs

---

# Model Pricing

Cost tracking depends on configured model pricing.

---

## model_price

Syntax:

```text
model_price <model> <input-usd-per-1m> <output-usd-per-1m>;
```

Example:

```conf
model_price gpt-4o-mini-2024-07-18 0.15 0.60;
```

The configured model name must match the upstream response exactly.

---

## embedding_price

Embedding pricing used for net savings estimation.

Example:

```conf
embedding_price 0.020;
```

Used for:

- embedding overhead calculation
- net savings estimation

---

# Graceful Shutdown

---

## graceful_shutdown_timeout_seconds

Controls graceful shutdown timeout.

Example:

```conf
graceful_shutdown_timeout_seconds 10;
```

Default:

```text
10
```

Behavior:

- readiness disabled
- in-flight requests allowed to finish
- new requests rejected

---

# Configuration Validation

AI Cost Firewall provides static validation similar to:

```text
nginx -t
```

---

# Validate Configuration

Docker Compose:

```bash
docker compose run --rm firewall \
  --config /configs/ai-firewall.conf \
  --test-config
```

Binary:

```bash
cargo run -- --config configs/ai-firewall.conf --test-config
```

Expected output:

```text
configuration OK
```

Static validation checks:

- syntax
- required directives
- value ranges
- semantic cache completeness
- request-size parsing
- prompt-size limits
- timeout settings
- model validation behavior

Static validation does not contact:

- Redis
- Qdrant
- upstream providers
- embedding providers

---

# Print Loaded Configuration

Inspect the resolved runtime configuration:

```bash
cargo run -- --config configs/ai-firewall.conf --print-config
```

Sensitive values are masked automatically.

Example:

```text
upstream_api_key = sk-y...-key
embedding_api_key = sk-y...-key
```

---

# Runtime Dependency Validation

During startup and reload, AI Cost Firewall validates:

- Redis connectivity
- Qdrant connectivity
- vector-size compatibility
- semantic cache configuration
- runtime dependency initialization

---

# Readiness Dependency Behavior

The `/readyz` endpoint can be configured to require specific runtime dependencies.

## readiness_requires_redis

Controls whether `/readyz` fails when Redis is unavailable.

Example:

```conf
readiness_requires_redis true;
```

Useful when Redis/exact cache is considered required for serving production traffic.

---

## readiness_requires_qdrant

Controls whether `/readyz` fails when Qdrant is unavailable.

Example:

```conf
readiness_requires_qdrant false;
```

This can remain `false` when semantic cache is configured to fail open.

---

## readiness_requires_upstream

Controls whether `/readyz` fails when the upstream provider is unavailable.

Example:

```conf
readiness_requires_upstream false;
```

This is often left disabled because upstream providers may be external services with temporary availability changes.

---

# Environment Variables

AI Cost Firewall supports environment-based configuration.

Example:

```text
AIF_REDIS_URL=redis://127.0.0.1:6379
AIF_UPSTREAM_API_KEY=sk-xxxx
AIF_EMBEDDING_MODEL=text-embedding-3-small
AIF_MAX_REQUEST_BODY_BYTES=2M
AIF_MAX_PROMPT_CHARS=200000
AIF_CACHE_BYPASS_HEADER=X-AIF-Cache-Bypass

AIF_SECURITY_GUARD_ENABLED=true
AIF_SECURITY_GUARD_URL=http://vcal-security-guard:8091
AIF_SECURITY_GUARD_API_KEY=your-security-guard-key
AIF_SECURITY_GUARD_TIMEOUT_SECONDS=3

AIF_PRIVACY_GUARD_ENABLED=true
AIF_PRIVACY_GUARD_URL=http://vcal-privacy-guard:8090
AIF_PRIVACY_GUARD_API_KEY=your-privacy-guard-key
AIF_PRIVACY_GUARD_MODE=anonymize
AIF_PRIVACY_GUARD_RESTORE_ENABLED=true
AIF_PRIVACY_GUARD_TIMEOUT_SECONDS=3

AIF_GUARD_FAIL_OPEN=false
```

`.env` files load automatically in development environments.

---

# Metrics & Observability

Metrics endpoint:

```text
/metrics
```

---

## metrics_auth_required

Controls whether `/metrics` requires bearer-token authentication.

Example:

```conf
metrics_auth_required false;
```

Default:

```text
false
```

For private Docker networks, `false` is convenient for Prometheus scraping.

For exposed production deployments, use:

```conf
metrics_auth_required true;
metrics_auth_token your-prometheus-token;
```

---

## metrics_auth_token

Bearer token required when `metrics_auth_required` is enabled.

Example:

```conf
metrics_auth_token your-prometheus-token;
```

When enabled, Prometheus or curl must send:

```http
Authorization: Bearer your-prometheus-token
```

Core metrics:

```text
aif_requests_total
aif_cache_hits_total{cache_type="exact"}
aif_cache_hits_total{cache_type="semantic"}
aif_cache_misses
aif_upstream_calls_total
```

Semantic diagnostics:

```text
aif_semantic_candidates_checked_total
aif_semantic_threshold_results_total
aif_semantic_lookup_duration_seconds
aif_semantic_expired_entries_skipped_total
aif_cache_bypass_requests_total
```

Runtime health:

```text
aif_readiness_state
aif_shutdown_in_progress
aif_inflight_requests
```

Cost metrics:

```text
aif_model_cost_micro_usd_total
aif_gross_saved_micro_usd_total
aif_embedding_overhead_micro_usd_total
aif_net_saved_micro_usd_total
```

Guard orchestration metrics:

```text
aif_guard_requests_total
aif_guard_latency_seconds
aif_security_blocks_total
aif_privacy_restore_skipped_total
```



---

# Security Recommendations

Configuration files may contain sensitive credentials.

Recommended practices:

- restrict file permissions
- avoid committing secrets
- inject secrets externally in production
- use private deployment networks

Example:

```bash
chmod 600 configs/ai-firewall.conf
```

---

# Recommended Related Documents

See also:

- `docs/quickstart.md`
- `docs/provider-compatibility.md`
- `docs/operation.md`
- `docs/troubleshooting.md`
- `docs/metrics-and-costs.md`
