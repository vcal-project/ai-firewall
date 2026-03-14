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

## Minimal Config Example

```text
listen_addr 0.0.0.0:8080;

redis_url redis://redis:6379;

upstream_base_url https://api.openai.com;
upstream_api_key sk-xxxx;

embedding_base_url https://api.openai.com;
embedding_api_key sk-xxxx;
embedding_model text-embedding-3-small;

qdrant_url http://qdrant:6334;
qdrant_collection aif_semantic_cache;
qdrant_vector_size 1536;

cache_ttl_seconds 86400;
request_timeout_seconds 120;

semantic_cache_enabled true;
semantic_similarity_threshold 0.92;

# Chat-completion pricing (USD per 1M tokens)
# model_price <model> <input_usd_per_1m_tokens> <output_usd_per_1m_tokens>;

model_price gpt-4o-mini-2024-07-18 0.15 0.60;
model_price gpt-4.1-mini-2025-04-14 0.30 1.20;

# These prices affect only aif_cost_saved_micro_usd.
# Embedding costs are not yet included in v0.1.0.
```

Note: AI Cost Firewall uses the Qdrant gRPC interface by default, which runs on port `6334`.
The REST API port (`6333`) is not used by the firewall.

> Note: `model_price` matching is **exact** in v0.1.0.
If the upstream API returns a versioned model name such as `gpt-4o-mini-2024-07-18`, that exact name must be present in the configuration for `aif_cost_saved_micro_usd` to be calculated.

This configuration is sufficient to run the firewall with Redis, Qdrant, and OpenAI APIs using the default Docker Compose setup.

Alternatively, the configuration file can be specified explicitly:

```bash
ai-firewall --config /path/to/ai-firewall.conf
```

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

Note:

This pricing is used to estimate cost savings when a cached `/v1/chat/completions` response is reused.

Embedding requests used for semantic caching are **not included** in the cost accounting in the current version.

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

### upstream_base_url

Base URL of the upstream API.

Example:

```text
upstream_base_url https://api.openai.com;
```

### upstream_api_key

API key used to authenticate requests to the upstream provider.

Example:

```text
upstream_api_key sk-xxxx;
```

---

## Embedding Settings

These settings are required when semantic caching is enabled, because prompt embeddings must be generated before performing semantic search.

### embedding_base_url

Base URL of the embedding API.

Example:

```text
embedding_base_url https://api.openai.com;
```

### embedding_api_key

API key used for embedding requests.

Example:

```text
embedding_api_key sk-xxxx;
```

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

when started from the project root directory.

---

## Configuration Validation

AI Cost Firewall provides a configuration validation command similar to nginx -t.
This command checks the configuration syntax and verifies that all runtime dependencies can be initialized.

Validation does **not start the HTTP server**, but it ensures that:
- the configuration file syntax is valid
- required directives are present
- Redis connectivity can be initialized
- Qdrant configuration is valid (if semantic cache is enabled)


### Validate a configuration file

Run:

```bash
cargo run -- --config configs/ai-firewall.conf --test-config
```

Expected output:

```bash
configuration OK
runtime dependencies initialized successfully
```

If an error is detected, the firewall prints a detailed message and exits with a non-zero status.

Example:

```bash
configuration error: unknown directive "redsi_url"
```

### Print the resolved configuration

You can also inspect the fully loaded configuration using:

```bash
cargo run -- --config configs/ai-firewall.conf --print-config
```

Sensitive fields such as API keys are automatically masked in the output.

Example:

```bash
upstream_api_key: ********
embedding_api_key: ********
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
