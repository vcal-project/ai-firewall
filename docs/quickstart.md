# AI Cost Firewall — Quick Start Guide

This guide explains how to prepare the configuration file and run the **AI Cost Firewall** locally.

The firewall acts as an **OpenAI-compatible API gateway** that sits between applications and LLM providers to reduce cost and latency through caching.

```text
Client
   │
   ▼
AI Cost Firewall
   │
   ├── Redis (exact cache)
   ├── Qdrant (semantic cache)
   │
   ▼
OpenAI-compatible upstream
```

## Quickest Start (Docker)

If you want to quickly test the firewall, the easiest method is cloning the repository and using Docker Compose:

Clone the repository and prepare the configuration:

```bash
git clone https://github.com/vcal-project/ai-firewall.git
cd ai-firewall
cp configs/ai-firewall.conf.example configs/ai-firewall.conf
```
Provider-specific examples are available under `configs/examples/` for OpenAI, Ollama, LM Studio, vLLM, LiteLLM, and OpenRouter.

Edit the configuration file and add your API keys:

```bash
nano configs/ai-firewall.conf
```

You should also specify the exact model names returned by your LLM provider (used for cost calculation), for example:

```text
gpt-4o-mini-2024-07-18
```

> The repository already includes all required Prometheus and Grafana configuration 

## Start the stack

```bash
docker compose pull
docker compose up -d
```

## Expected outcome

After startup:

- Firewall API is available at http://localhost:8080
- Prometheus is available at http://localhost:9090
- Grafana is available at http://localhost:3000
- No startup errors in logs
- Configuration validated successfully

Check logs:

```bash
docker compose logs -f firewall
```

Check health and readiness:

```bash
curl -i http://localhost:8080/healthz
curl -i http://localhost:8080/readyz
```

### Verifying the Container Image (Optional)

The `vcalproject/ai-firewall:vx.x.x` container image is signed with Cosign.
If you want to verify the integrity and authenticity of the image before running it, you can check the signature using the public key provided in this repository.

Public key:

```text
https://raw.githubusercontent.com/vcal-project/ai-firewall/main/security/cosign.pub
```

Example:

```bash
docker pull vcalproject/ai-firewall:v0.1.7

cosign verify \
  --key cosign.pub \
  vcalproject/ai-firewall:v0.1.7
```

If the verification succeeds, the image was produced and signed by the project maintainers and has not been tampered with.

## Common Startup Errors

### Missing model_price

```text
configuration error: no allowed models configured
```

Fix: define at least one `model_price` or enable pass-through.

### Semantic cache misconfigured

```text
configuration error: semantic_cache_enabled=true requires: embedding_model, qdrant_url
```

```text
configuration error: embedding_api_key must not be empty when semantic_cache_enabled=true. For local embedding providers without authentication, use dummy, none, null, or -
```

Fix: add required fields or disable semantic cache.

### Invalid request size

```text
configuration error: invalid AIF_MAX_REQUEST_BODY_BYTES value 'abc'
```

Fix: use values like `1M`, `512K`, `1048576`.

### Qdrant unavailable with semantic cache enabled

If `semantic_cache_enabled true;` is configured, Qdrant must be reachable during startup.

Fix: start Qdrant, correct `qdrant_url`, or disable semantic cache for local testing.

### Qdrant vector size mismatch

If the existing Qdrant collection was created for a different embedding dimension, startup fails clearly.

Fix: recreate the collection or set `qdrant_vector_size` to match the embedding model.

## Example Request

Before sending requests, ensure model name matches provider exactly.

```bash
curl http://localhost:8080/v1/chat/completions \
  -H "Content-Type: application/json" \
  -d '{
    "model": "gpt-4o-mini-2024-07-18",
    "messages": [
      {"role": "user", "content": "Say hello."}
    ]
  }'
```

> By default, AI Cost Firewall does not require client-side authorization on incoming requests.
> The `upstream_api_key` in the configuration is used by the firewall when calling the upstream LLM provider.
> For production deployments, place the firewall behind an authenticated reverse proxy, API gateway, VPN, or private network boundary.

---

## 1. Build from Source

Install the following components.

### Rust

Install Rust using `rustup`:

``` bash
curl https://sh.rustup.rs -sSf | sh
source $HOME/.cargo/env
```

Verify installation:

``` bash
rustc --version
cargo --version
```

### Redis

The firewall uses Redis for **exact request caching**.

Redis can be installed either via the system package manager or via Docker.

#### Option 1 — Docker (recommended for quick start)

```bash
docker run -d -p 6379:6379 redis:8
```

Verify
```bash
docker ps
docker exec -it <redis-container> redis-cli ping
```

Expected output:

```bash
PONG
```

#### Option 2 — System package (Ubuntu / Debian)

``` bash
sudo apt install redis-server
```

#### macOS

``` bash
brew install redis
```

Start Redis:

``` bash
redis-server
```

Verify:

``` bash
redis-cli ping
```

Expected output:

```text
PONG
```

> On RHEL / Rocky Linux, Redis may not be available in default repositories. Using Docker is recommended for consistency across environments.

### Qdrant (for semantic cache, optional)

Semantic caching requires a vector database.

Run Qdrant using Docker:

``` bash
docker run -d --rm --name qdrant \
  -p 6333:6333 \
  -p 6334:6334 \
  qdrant/qdrant
```

Verify:

```bash
curl http://127.0.0.1:6333/healthz
```

For MVP testing you can disable semantic cache.

> Semantic cache entries are not automatically deleted.  
> Expired entries are filtered during lookup and remain stored until manually pruned.

---

## 2. Build the Firewall

Clone the repository:

``` bash
git clone https://github.com/vcal-project/ai-firewall.git
cd ai-firewall
```

Build the release binary:

``` bash
cargo build --release
```

The executable will appear here:

    target/release/ai-firewall

---

## 3. Create the Configuration File

Create the directory:

``` bash
mkdir -p configs
```

Create the file:

    configs/ai-firewall.conf

Example configuration

```conf
listen_addr 0.0.0.0:8080;

redis_url redis://127.0.0.1:6379;

upstream_provider openai_compatible;
upstream_base_url https://api.openai.com;
upstream_api_key sk-your-openai-key;

embedding_provider openai_compatible;
embedding_base_url https://api.openai.com;
embedding_api_key sk-your-openai-key;
embedding_model text-embedding-3-small;

qdrant_url http://127.0.0.1:6334;
qdrant_collection aif_semantic_cache;
qdrant_vector_size 1536;

# Backward-compatible default
cache_ttl_seconds 86400;

# Optional lifecycle controls
# exact_cache_ttl_seconds 86400;
# semantic_cache_retention_seconds 604800;

request_timeout_seconds 120;

semantic_cache_enabled false;
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

### OpenAI-compatible provider URLs

For OpenAI-compatible local providers, `upstream_base_url` and `embedding_base_url` may use either the provider root URL or its `/v1` base path:

```text
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

For local providers without authentication, use placeholder keys:

```text
upstream_api_key dummy;
embedding_api_key dummy;
```

The main model upstream and embedding provider may use different base URLs.

### Semantic cache lifecycle

Semantic cache entries now include lifecycle metadata:

- `inserted_at`
- `expires_at`

Behavior:

- expired entries are filtered during lookup before similarity ranking
- expired entries are NOT deleted automatically
- semantic cache correctness does not depend on pruning

To remove expired entries manually:

```bash
./target/release/ai-firewall \
  --config configs/ai-firewall.conf \
  --prune-expired-semantic-cache
```

Recommended usage:

```bash
systemctl stop ai-firewall
./target/release/ai-firewall \
  --config configs/ai-firewall.conf \
  --prune-expired-semantic-cache
systemctl start ai-firewall
```

If the binary was installed into your $PATH, for example as /usr/local/bin/ai-firewall, you can use:

```bash
ai-firewall --config configs/ai-firewall.conf --prune-expired-semantic-cache
```

For systemd or installed binary deployments, use the production config path:

```bash
systemctl stop ai-firewall
ai-firewall \
  --config /etc/ai-firewall/ai-firewall.conf \
  --prune-expired-semantic-cache
systemctl start ai-firewall
```

For Docker Compose deployments, run pruning as a one-off container:

```bash
docker compose run --rm firewall \
  --config /configs/ai-firewall.conf \
  --prune-expired-semantic-cache
```

Alternatively, you can use environment variables (via a `.env` file), but the config file is the recommended approach.

---

### Model validation (IMPORTANT)

AI Cost Firewall validates the `model` field before forwarding requests upstream.

#### Default behavior (strict mode)

- Only models defined via `model_price` are allowed
- Unknown models are rejected with `400 Bad Request`
- Prevents accidental or unauthorized upstream usage

Example error:

```json
{
  "error": {
    "code": 400,
    "message": "Unsupported model: gpt-unknown",
    "type": "validation_error"
  }
}
```

---

#### Optional: allow pass-through

To allow unknown models:

```conf
allow_unknown_models_pass_through true;
```

In this mode:

- Unknown models are forwarded upstream
- Cost tracking is not applied to unknown models
- Firewall behaves more like a proxy

---

#### Common pitfall

If:

- `allow_unknown_models_pass_through = false`
- AND no `model_price` entries are defined

then requests will be rejected

---

#### Cost tracking note

If the upstream returns a versioned model name such as:

```
gpt-4o-mini-2024-07-18
```

That exact name must be present in the configuration:

```conf
model_price gpt-4o-mini-2024-07-18 0.15 0.60;
```

Otherwise:

- Cost savings will NOT be calculated
- `aif_cost_saved_micro_usd` will remain zero

---

### About service hostnames

When running inside Docker Compose, use service hostnames:

```text
redis://redis:6379
http://qdrant:6334
```

---

## 4. Protect the Configuration File

The config file contains API credentials.

Restrict permissions:

``` bash
chmod 600 configs/ai-firewall.conf
```

Never commit real API keys to Git.

---

## 5. Validate the Configuration

The firewall provides a command similar to `nginx -t`.

Validate the configuration:

### Local binary

``` bash
./target/release/ai-firewall --config configs/ai-firewall.conf --test-config
```
Or, if the release binary has not been built yet:

```bash
cargo run -- --config configs/ai-firewall.conf --test-config
```

Expected output:

```text
configuration OK
```

The command exits immediately after validation and does not start the server.

`--test-config` is static validation only. It does not connect to Redis, Qdrant, embedding providers, or upstream LLM providers.

### Docker Compose

Use the firewall service from docker-compose.yml:

```bash
docker compose run --rm firewall \
  --config /configs/ai-firewall.conf \
  --test-config
```

Expected output:

```text
configuration OK
```

> If your Compose service has a different name, check it with:

```bash
docker compose ps --services
```

---

## 6. Print the Loaded Configuration

You can inspect the resolved configuration:

### Local binary

``` bash
./target/release/ai-firewall --config configs/ai-firewall.conf --print-config
```

Or, if the release binary has not been built yet:

```bash
cargo run -- --config configs/ai-firewall.conf --print-config
```

### Docker Compose

```bash
docker compose run --rm firewall \
  --config /configs/ai-firewall.conf \
  --print-config
```

Secrets are automatically masked in the output.

The command prints the loaded configuration and exits without starting the server.

---

## 7. Start the Firewall

Run the service. 
 
If the configuration file exists at the default location:

``` bash
cargo run --release
```

If the configuration file is located elsewhere, specify it explicitly:

```bash
cargo run --release -- --config /path/to/ai-firewall.conf
```

Example log output:

    INFO loading config file configs/ai-firewall.conf
    INFO listening on 0.0.0.0:8080

The firewall is now running at:

    http://localhost:8080

### Runtime dependency validation

Runtime dependencies are initialized when the firewall starts normally.

- Redis is required for exact caching.
- Qdrant is required when `semantic_cache_enabled true;` is configured.
- If the Qdrant collection already exists, its vector size must match `qdrant_vector_size`.

Check liveness and readiness:

```bash
curl -i http://localhost:8080/healthz
curl -i http://localhost:8080/readyz
```

Expected result when the service is running normally:

```text
HTTP/1.1 200 OK
```

---

## 8. Test the Proxy

Send a test request:

``` bash
curl http://localhost:8080/v1/chat/completions \
  -H "Content-Type: application/json" \
  -d '{
    "model": "gpt-4o-mini-2024-07-18",
    "messages": [
      {"role": "user", "content": "Say hello."}
    ]
  }'
```

The firewall will forward the request to the configured OpenAI-compatible upstream.

---

## 9. View Metrics

Prometheus metrics are available at:

    http://localhost:8080/metrics

You can check them directly from the CLI:

```bash
curl http://localhost:8080/metrics
```

Example metrics:

    aif_requests_total
    aif_cache_exact_hits
    aif_cache_semantic_hits
    aif_tokens_saved
    aif_cost_saved_micro_usd
    aif_semantic_store_total
    aif_semantic_store_errors_total
    aif_embedding_request_duration_seconds
    aif_embedding_timeouts_total

This endpoint works without Prometheus or Grafana. Prometheus and Grafana are included in the Docker Compose stack for scraping, dashboards, and visualization.

> Note: `aif_inflight_requests` counts active HTTP requests to the firewall, including the `/metrics` scrape itself. When checking metrics with `curl http://localhost:8080/metrics`, it is normal to see `aif_inflight_requests 1` even when no chat request is running.

---

## 10. Reload Configuration (Hot Reload)

The firewall supports **nginx-style reload**.

### Systemd / binary deployment

Reload configuration without restarting:

``` bash
kill -HUP <firewall_pid>
```

Example:

``` bash
kill -HUP $(pgrep ai-firewall)
```

### Docker Compose deployment

Send the SIGHUP signal to the running container:

```bash
docker compose kill -s HUP firewall
```

### Expected behavior

Logs will show:

    received SIGHUP, reloading config
    config and runtime successfully reloaded

The server continues running and starts using the updated configuration.

---

## 11. Default Configuration Path

If no `--config` flag is provided, the firewall automatically looks for:

    configs/ai-firewall.conf

or

    /etc/ai-firewall/ai-firewall.conf

---

## Summary

To run locally:

1. Configure API key and models
2. Start Docker stack
3. Send requests to http://localhost:8080

The firewall then acts as a drop-in OpenAI-compatible API gateway that reduces cost and latency through exact and semantic caching.
