# Configuration Reference

AI Cost Firewall uses a **simple nginx-style configuration syntax** where each directive consists of a name, a value, and a terminating semicolon.

Each directive is written as:

```text
directive value;
```

Example:

```text
listen_addr 0.0.0.0:8080;
```

Configuration directives are **case-sensitive** and each directive must
end with a semicolon (`;`).


## Configuration Overview

The configuration file is divided into the following logical sections:

-   Core settings
-   Upstream API
-   Embedding settings
-   Vector database
-   Cache settings
-   Semantic cache

---

## v0.1.7 Notes

v0.1.7 hardens practical OpenAI-compatible provider support while keeping the flat configuration model.

Key additions:

- OpenAI-compatible base URLs may use either the provider root URL or its `/v1` base path
- full endpoint paths such as `/v1/chat/completions` and `/v1/embeddings` are rejected during validation
- upstream and embedding providers may use different base URLs
- placeholder API keys are supported for local providers without authentication:
  - `dummy`
  - `none`
  - `null`
  - `-`
- clearer diagnostics for invalid base URLs, authentication failures, unsupported endpoint paths, timeouts, and TLS/certificate failures
- more tolerant handling of OpenAI-compatible response quirks, including missing `model` and partial `usage` fields

---

## Previous (v0.1.6)

v0.1.6 hardens configuration diagnostics and semantic cache startup behavior.

Key additions:

- `--test-config` is a static validation command and exits with `configuration OK`
- `--print-config` prints a masked configuration view
- runtime semantic fail-open behavior is documented separately from strict startup initialization
- existing Qdrant collections are validated against `qdrant_vector_size`
- expired semantic entries are filtered before similarity ranking, preventing expired entries from blocking valid semantic hits
- placeholder API keys do not create bearer auth headers for OpenAI-compatible upstream or embedding providers:
  - `dummy`
  - `none`
  - `null`
  - `-`

## Previous (v0.1.5)

v0.1.5 introduced semantic cache lifecycle control:

- `exact_cache_ttl_seconds`
- `semantic_cache_retention_seconds`
- `inserted_at` and `expires_at` payload fields
- manual cleanup with `--prune-expired-semantic-cache`
- semantic store metrics

## Previous (v0.1.4)

v0.1.4 introduced operational hardening and observability:

- error classification
- upstream timeout visibility
- graceful shutdown and readiness handling
- semantic cache diagnostics

These changes remain part of the system in v0.1.6.

---

## Minimal Config Example

```conf
listen_addr 0.0.0.0:8080;

redis_url redis://redis:6379;

upstream_provider openai_compatible;
upstream_base_url https://api.openai.com;
upstream_api_key sk-xxxx;

embedding_provider openai_compatible;
embedding_base_url https://api.openai.com;
embedding_api_key sk-xxxx;
embedding_model text-embedding-3-small;

qdrant_url http://qdrant:6334;
qdrant_collection aif_semantic_cache;
qdrant_vector_size 1536;

# Backward-compatible default
cache_ttl_seconds 86400;

# Optional lifecycle controls
#exact_cache_ttl_seconds 86400;
#semantic_cache_retention_seconds 604800;

request_timeout_seconds 120;
graceful_shutdown_timeout_seconds 10;  # default
max_request_body_bytes 1048576;

semantic_cache_enabled true;
semantic_similarity_threshold 0.92;

# Model validation behavior
# By default, only models defined via `model_price` are allowed.
allow_unknown_models_pass_through false;

# Chat-completion pricing (USD per 1M tokens)
# model_price <model> <input_usd_per_1m_tokens> <output_usd_per_1m_tokens>;

model_price gpt-4o-mini-2024-07-18 0.15 0.60;
model_price gpt-4.1-mini-2025-04-14 0.30 1.20;

# Embedding pricing (optional, used for cost estimation only)
embedding_price 0.020;
```

## OpenAI-compatible providers

AI Cost Firewall supports OpenAI-compatible model and embedding endpoints through the flat provider model:

```text
upstream_provider openai_compatible;
upstream_base_url <base-url>;
upstream_api_key <key-or-placeholder>;

embedding_provider openai_compatible;
embedding_base_url <base-url>;
embedding_api_key <key-or-placeholder>;
```

The base URL may be either the provider root URL or its `/v1` base path:

```text
https://api.openai.com
https://api.openai.com/v1
http://ollama:11434
http://ollama:11434/v1
http://lmstudio:1234/v1
http://vllm:8000/v1
http://litellm:4000/v1
```

Do not configure the full endpoint path:

```text
# Wrong
upstream_base_url http://ollama:11434/v1/chat/completions;

# Correct
upstream_base_url http://ollama:11434/v1;
```

For local providers that do not require authentication, use a placeholder key:

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

When a placeholder key is used, AI Cost Firewall does not send the `Authorization: Bearer ...` header upstream.

The main model upstream and embedding provider may use different base URLs:

```text
upstream_base_url http://ollama:11434/v1;
embedding_base_url https://api.openai.com;
```

This is useful when chat completions are served locally but embeddings are provided by a different OpenAI-compatible service.

## Model validation

AI Cost Firewall validates the `model` field before forwarding requests upstream.

- Only models defined via `model_price` are considered supported
- Requests with unknown models are rejected with 400 Bad Request
- This prevents accidental or unauthorized upstream usage

Example:

```bash
{
  "error": {
    "code": 400,
    "message": "Unsupported model: gpt-unknown",
    "type": "validation_error"
  }
}
```

## Cache Lifecycle and TTL

AI Cost Firewall separates lifecycle control between cache layers:

- `exact_cache_ttl_seconds` — TTL for Redis exact cache
- `semantic_cache_retention_seconds` — retention window for semantic cache entries
- `cache_ttl_seconds` — backward-compatible default for both

### Behavior

**Exact cache (Redis):**
- TTL enforced automatically by Redis

**Semantic cache (Qdrant):**
- entries include `inserted_at` and `expires_at`
- expired entries are filtered during lookup before similarity ranking
- entries are NOT automatically deleted

This ensures:

- consistent cache behavior
- predictable reuse window
- no accidental reuse of stale responses

## Semantic Cache Cleanup

Expired semantic cache entries are ignored automatically during lookup.

To physically remove expired entries:

```bash
ai-firewall --prune-expired-semantic-cache
```

Recommended usage:

```bash
systemctl stop ai-firewall
ai-firewall --config /path/to/ai-firewall.conf --prune-expired-semantic-cache
systemctl start ai-firewall
```

Notes:

- pruning removes only expired entries (`expires_at <= now`)
- valid entries remain untouched
- Qdrant does not return exact deletion counts
- command can run during operation, but maintenance window is recommended

When `exact_cache_ttl_seconds` or `semantic_cache_retention_seconds` are explicitly set, they override `cache_ttl_seconds` for their respective cache layers.

In this case, `cache_ttl_seconds` acts only as a fallback default.

### Observability

v0.1.4 introduced visibility into semantic cache lifecycle:

- number of candidates evaluated
- threshold pass / fail decisions
- expired entries skipped during lookup

This helps diagnose:

- low semantic hit rates
- overly strict similarity thresholds
- short TTL configurations

## graceful_shutdown_timeout_seconds

Controls how long the firewall waits for in-flight requests to complete during shutdown.

Example:

```text
graceful_shutdown_timeout_seconds 10;
```

Default: 10 seconds

## Optional: allow pass-through

If you want the gateway to behave like a transparent proxy:

```conf
allow_unknown_models_pass_through true;
```

In this mode:

- Unknown models are forwarded upstream
- Cost tracking will not be applied for unknown models
- The firewall behaves more like a proxy

## Common Pitfall

If:

```conf
allow_unknown_models_pass_through false
AND
no model_price entries are defined
```

then all requests will be rejected.

## Model Pricing and Cost Tracking

Cost savings are calculated only for models defined via model_price  

The model name must exactly match the upstream response

Example:

```text
model_price gpt-4o-mini-2024-07-18 0.15 0.60;
```

If the upstream returns:

```text
gpt-4o-mini-2024-07-18
```

then cost is tracked

If it returns:

```text
gpt-4o-mini
```

then no cost tracking

AI Cost Firewall supports runtime config reload via SIGHUP in addition to static configuration at startup.

Runtime behavior (graceful shutdown, readiness, and config reload) is described in:

[docs/operation.md](docs/operation.md)

## Qdrant Notes

AI Cost Firewall uses the Qdrant gRPC interface by default, which runs on port `6334`.  

The REST API port (`6333`) is not used by the firewall.

When `semantic_cache_enabled true;` is configured, Qdrant must be reachable during startup. If the collection already exists, its vector size is validated against `qdrant_vector_size`. A mismatch fails startup with a clear configuration/runtime initialization error.

`semantic_cache_fail_open true;` applies to runtime semantic lookup failures. It does not allow startup to continue when Qdrant initialization fails.

## Running with explicit config

Alternatively, the configuration file can be specified explicitly:

```bash
ai-firewall --config /path/to/ai-firewall.conf
```

---

## Observability and Diagnostics

AI Cost Firewall exposes Prometheus metrics that reflect configuration behavior and runtime state.

### Key operational metrics

- `aif_readiness_state` — readiness (1 = ready, 0 = not ready)
- `aif_shutdown_in_progress` — shutdown state
- `aif_inflight_requests` — active request count

### Error classification

Errors are categorized into:

- `validation_error`
- `upstream_error`
- `upstream_timeout`
- `upstream_authentication_error`
- `upstream_not_found`
- `upstream_rate_limited`
- `upstream_tls_error`
- `upstream_dns_error`
- `upstream_connect_error`
- `internal_error`

Metric:

```text
aif_errors_total{class=...}
```

**Upstream behavior**

- `aif_upstream_request_duration_seconds`
- `aif_upstream_timeouts_total`

**Embedding provider diagnostics**

- `aif_embedding_request_duration_seconds`
- `aif_embedding_timeouts_total`

**Semantic cache diagnostics**

- `aif_semantic_candidates_checked_total`
- `aif_semantic_threshold_results_total{result="pass|fail"}`
- `aif_semantic_expired_entries_skipped_total`
- `aif_semantic_lookup_duration_seconds`
- `aif_semantic_store_total`
- `aif_semantic_store_errors_total`

---

## model_price

Defines the pricing used to estimate cost savings from cached chat-completion responses.

Syntax:

```text
model_price <model> <input_usd_per_1m_tokens> <output_usd_per_1m_tokens>;
```

Example:

```text
model_price gpt-4o-mini 0.15 0.60;
```

## embedding_price

Defines embedding cost used for net savings estimation.

Example:

```text
embedding_price 0.020;
```

Note:

This pricing is used to estimate cost savings when a cached `/v1/chat/completions` response is reused.

---

## Core Settings

### listen_addr

Address where the firewall HTTP server listens.

Example:

```text
listen_addr 0.0.0.0:8080;
```

Typical values:

```text
0.0.0.0:8080
127.0.0.1:8080
```

### Environment Variables (Optional)

If no configuration file is provided, AI Cost Firewall falls back to environment variables.

For convenience, you can use a `.env` file in development:

```conf
AIF_REDIS_URL=redis://127.0.0.1:6379
AIF_UPSTREAM_API_KEY=sk-xxxx
AIF_EMBEDDING_MODEL=text-embedding-3-small
AIF_EMBEDDING_PRICE_USD_PER_1M_TOKENS=0.020
AIF_MAX_REQUEST_BODY_BYTES=2M
```

- Variables follow the AIF_ prefix convention
- `.env` is loaded automatically if present
- Intended for development and simple deployments

If neither a config file nor required environment variables are provided, the application will fail to start.

### redis_url

Redis-compatible connection string used for the **exact request cache**.

The firewall works with Redis and Redis-compatible servers such as Valkey.

When running via Docker Compose, service names are used as hostnames.

Example:

```text
redis_url redis://redis:6379;
```

---

## Upstream API

These settings define the **LLM provider** the firewall forwards
requests to.

### upstream_provider

Provider mode for the chat-completion upstream.

Currently supported value:

```text
upstream_provider openai_compatible;
```

### upstream_base_url

Base URL of the OpenAI-compatible chat-completion provider.

The value may be the provider root URL or its `/v1` base path.

Examples:

```text
upstream_base_url https://api.openai.com;
upstream_base_url http://ollama:11434/v1;
upstream_base_url http://vllm:8000/v1;

### upstream_api_key

API key used to authenticate requests to the upstream provider.

Example:

```text
upstream_api_key sk-xxxx;
```

For local providers that do not require authentication, use `dummy`, `none`, `null`, or `-`. Placeholder keys do not create an upstream `Authorization: Bearer ...` header.

Do not configure the full endpoint path.

Wrong:

```text
upstream_base_url http://ollama:11434/v1/chat/completions;
```

---

## Embedding Provider Settings

These settings are required when semantic caching is enabled, because prompt embeddings must be generated before performing semantic search.

### embedding_provider

Provider mode for the embedding endpoint.

Currently supported value:

```text
embedding_provider openai_compatible;
```

### embedding_base_url

Base URL of the OpenAI-compatible embedding provider.

The value may be the provider root URL or its `/v1` base path.

Examples:

```text
embedding_base_url https://api.openai.com;
embedding_base_url http://ollama:11434/v1;
embedding_base_url http://embedding-gateway:8080/v1;

Example:

```text
embedding_base_url https://api.openai.com;
```

Do not configure the full endpoint path

Wrong:

```text
embedding_base_url http://ollama:11434/v1/embeddings;
```

### embedding_api_key

API key used for embedding requests.

Example:

```text
embedding_api_key sk-xxxx;
```

For local embedding providers that do not require authentication, use `dummy`, `none`, `null`, or `-`.

### embedding_model

Embedding model used to generate vector representations.

Example:
```text
embedding_model text-embedding-3-small;
```

---

## Vector Database (Qdrant)

These settings configure the **semantic cache backend**.

### qdrant_url

URL of the Qdrant server.  

When running via Docker Compose, service names are used as hostnames.

Example:

```text
qdrant_url http://qdrant:6334;
```

### qdrant_api_key

Optional API key for Qdrant authentication.

Example:

```text
qdrant_api_key your-qdrant-key;
```

### qdrant_collection

Name of the Qdrant collection used to store cached embeddings.

Example:

```text
qdrant_collection aif_semantic_cache;
```

### qdrant_vector_size

Dimension of embedding vectors.

Example:

```text
qdrant_vector_size 1536;
```
This must match the dimensionality of the embedding model used to generate vectors.

If the Qdrant collection already exists, AI Cost Firewall validates the existing collection vector size during startup. If the existing collection was created with a different vector size, startup fails clearly instead of producing confusing runtime errors.

Example:

| Model | Dimensions |
|------|-------------|
| text-embedding-3-small | 1536 |

---

## Cache Settings

### cache_ttl_seconds

Time-to-live for cached responses in Redis.

Example:

```text
cache_ttl_seconds 86400;
```

Example values:

```text
3600     1 hour
86400    1 day
604800   7 days
2592000  30 days
```

### request_timeout_seconds

Timeout for upstream API requests.

Example:

```text
request_timeout_seconds 120;
```
Default: 120 seconds

This prevents indefinite blocking on slow or unresponsive upstream providers.

### exact_cache_ttl_seconds

TTL for exact cache entries (Redis).

Overrides `cache_ttl_seconds` for exact cache only.

Example:

```text
exact_cache_ttl_seconds 86400;
```

---

## Semantic Cache

These settings control **semantic similarity caching**.

### semantic_cache_enabled

Enable or disable semantic caching.

Example:

```text
semantic_cache_enabled true;
```

Default: `true`

### semantic_cache_fail_open

Controls runtime behavior when semantic cache lookup fails.

Example:

```text
semantic_cache_fail_open true;
```

When set to `true`, runtime semantic lookup failures are treated as cache skips and the request continues upstream.

This does not disable startup initialization. If `semantic_cache_enabled true;` is configured, Qdrant must still be reachable during startup.

### semantic_similarity_threshold

Similarity threshold for reusing cached responses.

Example:

```text
semantic_similarity_threshold 0.92;
```

Default: `0.92`

Typical values:

```text
0.85  aggressive caching (higher hit rate, higher risk of mismatched answers)
0.92  balanced (recommended)
0.97  strict (only very similar prompts reused)
```

### max_request_body_bytes

Maximum allowed size of incoming request body.

Example:

```text
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

- Requests exceeding the limit are rejected early (HTTP 413)
- Large payloads do not reach upstream providers
- Protects against accidental or malicious oversized prompts

Notes:

- Values below 1K trigger a startup warning
- Applies to /v1/chat/completions endpoint

### semantic_cache_retention_seconds

Retention window for semantic cache entries.

Controls how long entries remain valid for reuse.

Example:

```text
semantic_cache_retention_seconds 604800;
```

Behavior:

Entries include `inserted_at` and `expires_at`
Expired entries are skipped during lookup
Expired entries are not reused

Notes:

Entries are not deleted automatically from Qdrant
Longer retention increases reuse but may increase collection size
Use `ai-firewall --prune-expired-semantic-cache` to remove expired entries

---

## Example Configuration File

A default configuration template is provided in the repository:

```text
configs/ai-firewall.conf.example
```

Copy the template and edit it for your deployment:

```bash
cp configs/ai-firewall.conf.example configs/ai-firewall.conf
nano configs/ai-firewall.conf
```

When started from the project root directory, the firewall automatically loads the configuration from:

```text
configs/ai-firewall.conf
```

---

## Configuration Validation

AI Cost Firewall provides a static configuration validation command similar to `nginx -t`.

Validation does **not start the HTTP server** and does **not** connect to Redis, Qdrant, embedding providers, or upstream LLM providers.

It checks that:

- the configuration file syntax is valid
- required directives are present
- values have valid formats and ranges
- semantic cache settings are complete when `semantic_cache_enabled true;` is configured
- at least one `model_price` is configured unless `allow_unknown_models_pass_through true;` is enabled

### Validate a configuration file

Run:

```bash
cargo run -- --config configs/ai-firewall.conf --test-config
```

Expected output:

```text
configuration OK
```

If an error is detected, the firewall prints a detailed message and exits with a non-zero status.

Example:

```text
configuration error: unknown directive "redsi_url"
```

### Print the resolved configuration

You can also inspect the fully loaded configuration using:

```bash
cargo run -- --config configs/ai-firewall.conf --print-config
```

Sensitive fields such as API keys are automatically masked in the output.

Example:

```text
upstream_api_key = sk-y...-key
embedding_api_key = sk-y...-key
qdrant_api_key = <not set>
```

---

## Security Note

The configuration file may contain sensitive credentials, including LLM API keys and embedding API keys.

Recommended practices:

- restrict file permissions
- never commit real API keys to version control
- store configuration outside version control
- use environment-based secret injection in production

Example:

```bash
chmod 600 configs/ai-firewall.conf
```
