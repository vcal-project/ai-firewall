
# Provider Compatibility

AI Cost Firewall supports practical OpenAI-compatible model and embedding providers while keeping a flat, provider-agnostic configuration model.

This document describes tested provider patterns, compatibility notes, operational recommendations, guard-orchestration notes, and common deployment considerations.

---

## Compatibility Philosophy

AI Cost Firewall does not implement provider-specific configuration blocks.

Instead, it expects providers to expose OpenAI-compatible APIs.

This keeps configuration simple and portable across:

- cloud providers
- local inference servers
- proxy gateways
- self-hosted OpenAI-compatible deployments

The same configuration structure is used for all providers:

```text
upstream_provider openai_compatible;
upstream_base_url <base-url>;
upstream_api_key <key-or-placeholder>;

embedding_provider openai_compatible;
embedding_base_url <base-url>;
embedding_api_key <key-or-placeholder>;
```

The upstream provider and embedding provider may use different endpoints.

---

## Supported Provider Patterns

| Provider | Status | Typical Use |
|---|---|---|
| OpenAI | Fully tested | Cloud chat + embeddings |
| Ollama | Supported | Fully local deployments |
| LM Studio | Supported | Desktop local inference |
| vLLM | Supported | Self-hosted GPU inference |
| LiteLLM | Supported | Multi-provider aggregation |
| OpenRouter | Supported | OpenAI-compatible routing layer |

---

## Base URL Configuration

AI Cost Firewall expects provider base URLs, not full endpoint paths.

### Correct

```text
upstream_base_url https://api.openai.com;
```

or:

```text
upstream_base_url http://ollama:11434/v1;
```

### Wrong

```text
upstream_base_url http://ollama:11434/v1/chat/completions;
```

AI Cost Firewall automatically appends OpenAI-compatible endpoint paths internally.

---

## Authentication Behavior

### Providers Requiring Authentication

Typical cloud providers:

- OpenAI
- OpenRouter

Example:

```text
upstream_api_key sk-your-key;
```

---

### Local Providers Without Authentication

Typical local deployments:

- Ollama
- LM Studio
- local vLLM

Use placeholder values:

```text
upstream_api_key dummy;
embedding_api_key dummy;
```

Accepted placeholders:

```text
dummy
none
null
-
```

---

## OpenAI

### Status

Fully tested reference implementation.

### Recommended Usage

- cloud chat completions
- cloud embeddings
- hybrid cloud deployments

### Example

```text
upstream_base_url https://api.openai.com;
embedding_base_url https://api.openai.com;
```

### Recommended Embedding Model

```text
text-embedding-3-small
```

Typical vector size:

```text
1536
```

### Notes

- strict model validation recommended
- ideal reference deployment for first evaluation
- best compatibility baseline

---

## Ollama

### Status

Supported.

### Typical Usage

- fully local deployments
- local embeddings
- air-gapped evaluation
- low-cost development environments

### Recommended Base URL

```text
http://ollama:11434/v1
```

### Typical Models

Chat:

```text
llama3.2:3b
```

Embeddings:

```text
nomic-embed-text
```

### Typical Vector Size

```text
768
```

### Required Step

Pull models manually:

```bash
docker compose exec ollama ollama pull llama3.2:3b
docker compose exec ollama ollama pull nomic-embed-text
```

Restart firewall afterward:

```bash
docker compose restart firewall
```

### Notes

- HTTP is usually sufficient inside Docker networks
- startup failures may occur if embedding model is unavailable
- semantic cache behavior depends heavily on embedding quality

---

## LM Studio

### Status

Supported.

### Typical Usage

- local desktop evaluation
- development environments
- lightweight testing

### Example Base URL

```text
http://host.docker.internal:1234/v1
```

### Notes

- verify OpenAI-compatible mode is enabled
- verify embeddings endpoint support
- embeddings may require explicit model loading
- desktop sleep or restart may interrupt connectivity

---

## vLLM

### Status

Supported.

### Typical Usage

- GPU-backed inference servers
- self-hosted production inference
- high-throughput deployments

### Example Base URL

```text
http://vllm:8000/v1
```

### Model discovery

AIF v0.8.2 proxies:

```text
GET /v1/models
```

to the configured chat/inference vLLM endpoint. This is useful for clients such as Open WebUI that perform OpenAI-compatible model discovery through the gateway.

### Separate chat and embedding services

Chat and embeddings may be served by separate vLLM Services:

```text
upstream_base_url http://vllm-chat:8000/v1;
embedding_base_url http://vllm-embeddings:8000/v1;
```

For a Nomic embedding service, determine the exact served model ID from the embedding server and verify the returned embedding length before setting `qdrant_vector_size`. Do not assume a fixed dimension solely from the Nomic family name or from an Ollama example.

### Notes

- tune request timeouts for large models
- verify embeddings support separately from chat inference
- startup latency may be higher for large models
- `GET /v1/models` through AIF discovers the chat upstream, not a separately configured embedding server
- useful for self-hosted OpenAI-compatible APIs

---

## LiteLLM

### Status

Supported.

### Typical Usage

- provider aggregation
- routing layer
- centralized API gateway
- multi-provider environments

### Example Base URL

```text
http://litellm:4000/v1
```

### Notes

- useful as an upstream aggregation layer
- provider-specific model names may vary
- pricing visibility depends on LiteLLM configuration
- consider enabling pass-through mode for unknown models

---

## OpenRouter

### Status

Supported.

### Typical Usage

- multi-provider routing
- rapid cloud evaluation
- model experimentation

### Example Base URL

```text
https://openrouter.ai/api/v1
```

### Recommended Setting

```text
allow_unknown_models_pass_through true;
```

### Notes

OpenRouter model names may vary by provider and model family:

Examples:

```text
openai/gpt-4o-mini
anthropic/claude-3-opus
google/gemini-pro
```

Strict model validation may reject unknown names unless explicitly configured.

---

## Hybrid Provider Patterns

AI Cost Firewall supports separate upstream and embedding providers.

Examples:

| Chat Provider | Embedding Provider |
|---|---|
| OpenAI | Ollama |
| OpenRouter | OpenAI |
| Ollama | OpenAI |
| LiteLLM | Ollama |

This is useful for:

- reducing embedding cost
- local semantic caching
- separating privacy-sensitive embeddings
- hybrid cloud/local deployments

---

## Embedding Compatibility

Semantic cache correctness depends heavily on embedding compatibility.

### Important Requirements

The configured vector size must match the actual embedding vector returned by the serving endpoint. Model-family defaults are useful examples but should not replace a runtime check when the server supports configurable or model-specific dimensions.

Examples:

| Embedding Model | Vector Size |
|---|---|
| text-embedding-3-small | 1536 |
| nomic-embed-text (common Ollama configuration) | 768 |

### Common Error

```text
existing collection vector size does not match qdrant_vector_size
```

### Recommended Practice

Use separate Qdrant collections for different embedding models.

---

## Semantic Cache Considerations

Semantic cache quality depends on:

- embedding quality
- similarity threshold
- prompt consistency
- provider behavior

Recommended starting threshold:

```text
semantic_similarity_threshold 0.92;
```

Lower thresholds:

- increase reuse
- increase risk of incorrect matches

Higher thresholds:

- reduce incorrect matches
- reduce semantic hit rate

---

## Guard Orchestration and Provider Compatibility

VCAL Security Guard, VCAL Privacy Guard, and VCAL Usage Guard operate at the AI Firewall layer and are independent of the selected OpenAI-compatible upstream provider.

Provider compatibility still matters for OpenAI-compatible chat request/response shape, model naming, streaming behavior, tool/function response formats, and embedding endpoint behavior when semantic cache is enabled.

Privacy Guard inspects supported string and structured text-content arrays; Security Guard and Usage Guard have separate, more limited text inspection coverage. Non-text content such as images, audio, video, binary payloads, and referenced attachments is not inspected by these integrations. Recognized unsupported text-bearing content is rejected by Privacy Guard when enabled.

AI Cost Firewall supports ordinary JSON chat completions and OpenAI-compatible SSE with progressive or controlled delivery. Optional Guards that need complete-response inspection require the controlled delivery path.

---
## Evaluation Mode and Provider Capacity

Provider compatibility requirements do not change in AIF v0.8.0 Evaluation Mode, but provider traffic behavior does.

With:

```conf
aif_enforcement_mode observe;
```

would-have cache hits still generate a real upstream chat-completion request. Plan provider rate limits, concurrency, and budget for the full observed workload during the pilot. Evaluation metrics estimate what enforcement could avoid later; they do not represent traffic already avoided.

For `stream=true`, the upstream must still provide compatible SSE in observe mode because the live upstream stream is consumed and approved even when shadow cache evaluation reports a would-have hit.

---

## Streaming Compatibility

AI Cost Firewall accepts OpenAI-compatible SSE from providers when clients set `stream=true`. The provider must emit parseable stream events for content and supported tool/function-call deltas. AIF assembles a canonical completion for finalization even when eligible upstream content is forwarded progressively.

- In `controlled` mode, AIF consumes SSE internally, applies full-response controls and then re-encodes approved OpenAI-compatible SSE, ending with `[DONE]`. Provider framing is not reproduced byte-for-byte.
- In `progressive` mode, eligible upstream SSE can reach the client as it arrives. Once response headers or content have been committed, upstream failures cannot be converted into an ordinary pre-stream HTTP error or retract previously sent content.
- With `auto`, configuration and request eligibility determine the selected path. Enabled Guards requiring complete-response inspection use controlled delivery.
- Redis and Qdrant cache identity is shared across JSON and SSE when the request and cached completion are eligible. Cached completions can be rendered as SSE without an upstream model call.
- `stream_options.include_usage` is supported for controlled/cache-derived SSE where the provider supplies the required accounting information.
- Ineligible completions, including `finish_reason: "length"`, can be delivered to the client but are not stored for future reuse.

Configuration:

```conf
streaming_enabled true;
streaming_delivery_mode auto;
max_stream_upstream_bytes 8M;
upstream_timeout_seconds 120;
```

`max_stream_upstream_bytes` caps cumulative provider SSE bytes accepted during a streaming request; it is not a peak parser-buffer setting. Consult [Streaming Delivery](controlled-streaming.md) for the controlled commit barrier, progressive failure trade-off, and stream telemetry. Validate provider-specific SSE framing and accounting semantics against the deployed upstream.

---

## Timeout Recommendations

Recommended starting point:

```text
upstream_timeout_seconds 120;
embedding_timeout_seconds 30;
```

where `request_timeout_seconds` is a fallback. For controlled streaming, `upstream_timeout_seconds` also defines provider-body idle timeout; it does not replace the separate 15-minute absolute generation ceiling.

Local GPU models and large cloud models may require higher values.

Monitor:

```text
aif_upstream_request_duration_seconds
```

and:

```text
aif_upstream_timeouts_total
```

---

## TLS Recommendations

### Cloud Providers

Use HTTPS:

```text
https://
```

This encrypts:

- prompts
- responses
- API keys
- embedding traffic

---

### Local Providers

Private HTTPS endpoints (including OpenShift service-ca certificates) are
supported with `upstream_ca_bundle` / `embedding_ca_bundle`, which add PEM trust
roots without weakening certificate verification. Read the
[private CA guide](private-ca-tls.md).

For local providers explicitly configured for plaintext HTTP, inside trusted
Docker networks:

```text
http://
```

is usually sufficient and simpler operationally.

---

## Startup Validation Behavior

When semantic cache is enabled:

```text
semantic_cache_enabled true;
```

AI Cost Firewall validates:

- embedding configuration
- Qdrant availability
- vector-size compatibility
- provider configuration

Startup validation is intentionally strict to prevent silent misconfiguration and unexpected upstream cost.

---

## Recommended Deployment Examples

See:

```text
deploy/examples/
```

Available patterns include:

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
- expected behavior
- metrics guidance
- observability overlays

---

## Operational Recommendations

### Best First Evaluation Path

For a production traffic assessment with minimal cache-related behavioral change, use `aif_enforcement_mode observe;`. Keep in mind that guard modules still enforce according to their own configuration, and the upstream provider continues to receive the full live request volume.

Recommended:

```text
deploy/examples/openai-cloud/
```

Fastest path to validation.

---

### Best Fully Local Deployment

Recommended:

```text
deploy/examples/local-full-stack/
```

Includes:

- Ollama
- Redis
- Qdrant
- Prometheus
- Grafana

---

### Best Hybrid Pattern

Recommended:

```text
deploy/examples/hybrid-openai-local-embeddings/
```

Useful for:

- reducing embedding cost
- local semantic cache privacy
- hybrid deployments

---

## Troubleshooting

See:

- `docs/troubleshooting.md`
- `docs/config-reference.md`
- `docs/operation.md`

Common issues include:

- wrong provider base URLs
- missing embedding models
- TLS certificate failures
- vector-size mismatch
- provider timeout behavior
- OpenAI-compatible API differences
