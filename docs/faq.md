# AI Cost Firewall — FAQ

Concise answers to frequent questions. Detailed behavior belongs in the linked guide so that a single
explanation remains authoritative.

## Product and providers

### What is AI Cost Firewall?

A self-hosted OpenAI-compatible gateway that reduces repeated upstream calls using Redis/Valkey exact
caching and Qdrant semantic caching. AIF also exposes metrics and optionally integrates separately licensed
VCAL Guards and Audit. See [Architecture](architecture.md).

### Does AIF support native Anthropic, Gemini, Mistral, or Cohere APIs?

Not directly in this documented version. It expects OpenAI-compatible chat and embedding endpoints; such
providers may be used through a compatible translation gateway such as LiteLLM or OpenRouter. Compatibility
is not universal. See [Provider Compatibility](provider-compatibility.md).

### Can the chat model and embedding model run on different services?

Yes. Configure their separate OpenAI-compatible base URLs and verify embedding dimensions. `GET /v1/models`
proxies the configured **chat** upstream's model discovery; it does not enumerate a separate embedding
service. See [Provider Compatibility](provider-compatibility.md).

### Does AIF authenticate incoming application users?

The configured `upstream_api_key` authenticates AIF **to the provider**; it does not by itself authenticate
incoming clients. Protect the AIF ingress through your network/reverse proxy/API gateway and decide on an
appropriate cache scope for shared or user-specific responses. See [Architecture](architecture.md).

### Is the standalone VCAL Semantic Cache Server an AIF backend?

No. AIF currently uses Qdrant for semantic cache; the separately developed VCAL Semantic Cache Server is not
integrated as an AIF cache adapter.

## Caching and evaluation

### What is the difference between exact and semantic caching?

Exact reuse requires matching normalized request identity. Semantic reuse uses embeddings and similarity
plus candidate-validity checks. Cache eligibility can vary with tools, structured content, model settings,
and guards. See [How It Works](how-it-works.md).

### Can I disable one cache, or allow lookup but disable store?

Yes. Exact and semantic enable/store controls are separate. Cache failures have separate fail-open options.
See [Configuration Reference](config-reference.md).

### Can I bypass caching for one request?

Yes. The default `X-AIF-Cache-Bypass: true` header skips both lookups and stores. This also skips
shadow-cache work in Observe Mode. See [How It Works](how-it-works.md#per-request-bypass).

### What do `enforce` and `observe` change?

`enforce` may return eligible cached responses; `observe` always forwards the live request to the chat
provider and records hypothetical reuse using isolated shadow cache state. Observe does **not** reduce live
provider traffic or automatically change guard policy modes. See [Evaluation Mode](evaluation-mode.md).

### What is Assessment Context?

`GET /assessment-context` returns sanitized effective configuration information and a deterministic hash.
Combined with historical `aif_runtime_info` and bounded evaluation metrics, it supports reproducible
external assessments. AIF itself does not persist assessment reports. See [Assessment
Context](assessment-context.md).

### Are streaming completions cacheable?

Eligible streaming and non-streaming requests share cache identity. For `stream=true`, AIF assembles the
provider response and applies controls **before** replaying SSE. It is not raw token-by-token pass-through.
See [Controlled Streaming](controlled-streaming.md).

### Are tool calls and structured message arrays cacheable?

Completion-affecting tool/format fields participate in exact identity. Non-string message content bypasses
semantic caching. Supported text parts can be inspected by Privacy Guard, but do not assume equivalent
coverage in every other module. See [Structured Content](structured-content.md).

### Does AIF scan images or documents attached to a chat?

No general attachment processor is documented. Referenced files, opaque non-text payloads, and images are
not fetched and inspected as documents by AIF. See [Structured Content](structured-content.md).

## Configuration and operations

### Should `upstream_base_url` include `/v1/chat/completions`?

No. Use the provider's compatible base URL; AIF appends the chat-completions route. The correct `/v1` prefix
depends on the provider. See [Provider Compatibility](provider-compatibility.md).

### What if a Qdrant collection has a different vector size?

Startup validation reports a mismatch. Verify the actual embedding dimension and use a matching collection;
do not silently reuse vectors from a different model/dimension. See [Troubleshooting](troubleshooting.md).

### How do I validate the configuration or reload it?

AIF supports `--test-config`, masked `--print-config`, and nginx-style `SIGHUP` for reloadable settings.
Listener, request-body limit, inflight-request limit, and graceful-shutdown timeout are restart-only
settings. See [Operations](operation.md) and [Configuration Reference](config-reference.md).

### What do `/healthz`, `/startupz`, and `/readyz` mean?

Liveness, successful initialization of configured required dependencies, and current readiness are distinct.
In Observe Mode, optional evaluation cache failures do not block live upstream responses. See
[Operations](operation.md).

### What happens if Redis, Qdrant, embeddings, or a Guard fails?

The behavior depends on the enabled path, `*_fail_open`/`guard_fail_open`, and readiness settings.
Evaluation-only cache dependency failures are non-disruptive; unsupported recognized Privacy text structures
are not the same as a transport failure. See [Operations](operation.md) and
[Troubleshooting](troubleshooting.md).

## Optional modules and evidence

### Are VCAL Security, Privacy, Usage, or Audit required?

No. AIF Core works without these separately licensed products. Guards can be independently enabled; optional
VCAL Audit receives structured evidence. See [Architecture](architecture.md) and [Evidence and
Audit](audit-integration.md).

### Which Guard handles what?

Security Guard evaluates security rules; Privacy Guard inspects supported sensitive text and may
anonymize/redact and later restore placeholders; Usage Guard applies organizational usage policies. Coverage
for structured content is module-specific and must be checked for the deployed module/API versions. See
[Structured Content](structured-content.md).

### Does Privacy Guard protect shared cached entries?

For supported requests with complete effective tenant and privacy policy identity, AIF scopes cache entries
accordingly and stores pre-restoration placeholder responses. Missing effective policy identity or Privacy
Guard transport fail-open bypasses caches. This is not a replacement for application authorization. See
[Structured Content](structured-content.md).

### Is evidence delivery to Audit durable?

Not end-to-end from AIF: the HTTP sender uses a bounded **in-memory** queue with retries. Persistent Audit
storage begins after successful ingestion, and events can be dropped on queue overflow, exhausted retries,
or premature shutdown. See [Evidence and Audit](audit-integration.md).

## Metrics, troubleshooting, and releases

### How are savings calculated? Do Observe metrics reduce my bill?

Estimated provider spend and gross savings use configured model prices; embedding overhead reduces net
savings. Observe records potential savings separately from actual Enforce-mode avoidance and does not reduce
live upstream calls. See [Metrics and Costs](metrics-and-costs.md).

### Why do my dashboards have zero values?

Check that real traffic reaches AIF, that Prometheus scrapes its metrics endpoint, that the configured
provider is reachable, that cache is eligible, and that you are looking at the correct Observe/Enforce
series. See [Troubleshooting](troubleshooting.md).

### Is AIF guaranteed to achieve the README benchmark rates?

No. The supplied benchmarks were controlled local runs, often using a simulated upstream. They are not
production capacity or savings guarantees. See [Benchmark report](../BENCHMARKS.md) and [Evaluation
validation](../EVALUATION_VALIDATION.md).

### Where should I start?

[Quick Start](quickstart.md) → [Configuration Reference](config-reference.md) → [Operations](operation.md).
For version changes, use [Upgrading](upgrading.md) and the [GitHub
releases](https://github.com/vcal-project/ai-firewall/releases).
