# AI Cost Firewall — Quick Start Guide

This guide explains how to prepare the configuration file and run the **AI Cost Firewall** locally.

The firewall acts as an **OpenAI-compatible API gateway*** that sits between applications and LLM providers to reduce cost and latency through
caching.

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
OpenAI API
```

## Quickest Start (Docker)

If you want to quickly test the firewall, the easiest method is using Docker Compose:

```bash
docker compose -f deploy/docker-compose.yml up -d
```

This starts:

- Redis
- Qdrant
- AI Cost Firewall
- Prometheus
- Grafana

You only need to edit:

```text
configs/ai-firewall.conf
```

Copy the example configuration:

```bash
cp configs/ai-firewall.conf.example configs/ai-firewall.conf
```

Adjust the settings:

```bash
nano configs/ai-firewall.conf
```

After starting the stack:

Firewall API:

```bash
http://localhost:8080
```

Prometheus:
```bash
http://localhost:9090
```

Grafana:

```bash
http://localhost:3000
```

### Verifying the Container Image (Optional)

The `vcalproject/ai-firewall` container image is signed with Cosign.
If you want to verify the integrity and authenticity of the image before running it, you can check the signature using the public key provided in this repository.

Example:

```bash
docker pull vcalproject/ai-firewall:v0.1.0

cosign verify \
  --key cosign.pub \
  vcalproject/ai-firewall:v0.1.0
```

If the verification succeeds, the image was produced and signed by the project maintainers and has not been tampered with.

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

Redis can be installed either via the system package manager or via Docker. For quick local testing, installing Redis via `apt` is usually the simplest option.

#### Ubuntu / Debian

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

### Qdrant (for semantic cache, optional)

Semantic caching requires a vector database.

Run Qdrant using Docker:

``` bash
docker run -p 6334:6334 qdrant/qdrant
```

For MVP testing you can disable semantic cache.

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

Example configuration:

``` conf
listen_addr 0.0.0.0:8080;

redis_url redis://127.0.0.1:6379;

upstream_base_url https://api.openai.com;
upstream_api_key sk-your-openai-key;

embedding_base_url https://api.openai.com;
embedding_api_key sk-your-openai-key;
embedding_model text-embedding-3-small;

qdrant_url http://127.0.0.1:6334;
qdrant_collection aif_semantic_cache;
qdrant_vector_size 1536;

cache_ttl_seconds 86400;
request_timeout_seconds 120;

semantic_cache_enabled false;
semantic_similarity_threshold 0.92;

# Chat-completion pricing (USD per 1M tokens)
# model_price <model> <input_usd_per_1m_tokens> <output_usd_per_1m_tokens>;

model_price gpt-4o-mini-2024-07-18 0.15 0.60;
model_price gpt-4.1-mini-2025-04-14 0.30 1.20;

# These prices affect only aif_cost_saved_micro_usd.
# Embedding costs are not yet included in v0.1.0.
```

> Note: `model_price` matching is exact in v0.1.0.
If the versioned model name is `gpt-4o-mini-2024-07-18`, you must add that exact name to the configuration for `aif_cost_saved_micro_usd` to be calculated correctly.

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

``` bash
cargo run -- --config configs/ai-firewall.conf --test-config
```

Expected output:

    configuration OK
    runtime dependencies initialized successfully

---

## 6. Print the Loaded Configuration

You can inspect the resolved configuration:

``` bash
cargo run -- --config configs/ai-firewall.conf --print-config
```

Secrets are automatically masked in the output.

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

---

## 8. Test the Proxy

Send a test request:

``` bash
curl http://localhost:8080/v1/chat/completions \
  -H "Authorization: Bearer <your-key>" \
  -H "Content-Type: application/json" \
  -d '{
    "model": "gpt-4o-mini",
    "messages": [
      {"role": "user", "content": "Say hello."}
    ]
  }'
```

The firewall will forward the request to the upstream OpenAI API.

---

## 9. View Metrics

Prometheus metrics are available at:

    http://localhost:8080/metrics

Example metrics:

    aif_requests_total
    aif_cache_exact_hits
    aif_cache_semantic_hits
    aif_tokens_saved
    aif_cost_saved_micro_usd

---

## 10. Reload Configuration (Hot Reload)

The firewall supports **nginx-style reload**.

Reload configuration without restarting:

``` bash
kill -HUP <firewall_pid>
```

Example:

``` bash
kill -HUP $(pgrep ai-firewall)
```

Logs will show:

    received SIGHUP, reloading config
    config and runtime successfully reloaded

---

## 11. Default Configuration Path

If no `--config` flag is provided, the firewall automatically looks for:

    configs/ai-firewall.conf

or

    /etc/ai-firewall/ai-firewall.conf

---

## Summary

Running the firewall locally requires:

1. Redis
2. Qdrant (optional for semantic cache)
3. OpenAI API key
4. Configuration file
5. cargo run --release

The firewall then acts as a **drop-in OpenAI-compatible API gateway** that reduces cost and latency through exact and semantic caching.
