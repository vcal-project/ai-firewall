# AI Cost Firewall — FAQ

Short answers about deploying AI Cost Firewall as a self-hosted control point for production AI.
For version-specific behavior and procedures, follow the linked technical guides.

## Product and providers

### What is AI Cost Firewall?

AI Cost Firewall (AIF) is a **self-hosted, OpenAI-compatible control point for production AI**.
It uses **Redis exact caching** and **Qdrant semantic caching** to identify opportunities to avoid
repeated upstream model calls. AIF provides Prometheus metrics and structured evidence; optional,
separately licensed VCAL modules add privacy, security, organizational usage-policy, auditing, and
compliance capabilities. See [Architecture](architecture.md).

### Does AIF require replacing my existing AI platform?

No. Keep your OpenAI-compatible applications, model endpoints, and observability tools. Point compatible
client requests at AIF instead of directly at the provider. See [Deployment](deployment.md) and
[Provider Compatibility](provider-compatibility.md).

### Does the AIF core have a software license fee?

No. The open-source AIF core has **no software license fee**. Annual **VCAL Production Support** is
optional, and additional VCAL modules are separately licensed. See [VCAL pricing](https://vcal-project.com/pricing).

### Is a VCAL-operated service part of the request path?

No. AIF runs in infrastructure you control (on-premises, private cloud, or elsewhere); there is no
VCAL-operated SaaS gateway in the request path. See [Deployment](deployment.md).

### Does AIF support native Anthropic, Gemini, Mistral, or Cohere APIs?

Not directly. AIF expects OpenAI-compatible chat and embedding endpoints. Those providers may be used
through a compatible translation gateway such as LiteLLM or OpenRouter; compatibility is not universal.
See [Provider Compatibility](provider-compatibility.md).

### Can the chat model and embedding model run on different services?

Yes. Configure their separate OpenAI-compatible base URLs and verify embedding dimensions. `GET /v1/models`
proxies the configured **chat** upstream's model discovery; it does not enumerate a separate embedding
service. See [Provider Compatibility](provider-compatibility.md).

### Does AIF authenticate incoming application users?

The configured `upstream_api_key` authenticates AIF **to the provider**, not incoming application users.
Protect AIF ingress using appropriate network and authentication controls, and ensure cache reuse is
appropriate for your workload. AIF is not a replacement for application authorization. See
[Architecture](architecture.md).

### Is the standalone VCAL Semantic Cache Server an AIF backend?

No. **VCAL Semantic Cache Server** is a separate, standalone HTTP semantic-cache service for other
applications and RAG pipelines. AIF uses Redis and Qdrant; no native AIF adapter for Semantic Cache Server
is currently available.

## Caching and Observe/Enforce modes

### What is the difference between exact and semantic caching?

Exact reuse requires matching normalized request identity. Semantic reuse uses embeddings and similarity
plus candidate-validity checks. Cache eligibility can vary with tools, structured content, model settings,
and guards. See [How It Works](how-it-works.md).

### Can I disable one cache, or allow lookup but disable store?

Yes. Exact and semantic enable/store controls are separate. Cache failures have separate fail-open options.
See [Configuration Reference](config-reference.md).

### Can I bypass caching for one request?

Yes. The default `X-AIF-Cache-Bypass: true` header skips both cache lookups and stores, including
isolated evaluation-cache work in Observe mode. See [How It Works](how-it-works.md#per-request-bypass).

### How do Observe mode and Enforce mode differ?

**Observe mode** (`observe`) forwards requests that pass enabled controls to the upstream model while using
**isolated evaluation caches** to estimate potential cache hits, avoided calls and tokens, and cost savings.
It never substitutes cached responses for live upstream responses. **Enforce mode** (`enforce`) can serve
eligible cached results, avoiding upstream model calls. Observe mode does not change the behavior of
independently configured VCAL Guards. See [Evaluation Mode](evaluation-mode.md).

### What is Assessment Context?

`GET /assessment-context` returns sanitized effective configuration information and a deterministic hash.
Together with historical `aif_runtime_info` and evaluation metrics from a defined observation period,
it supports reproducible external assessments. AIF itself does not persist assessment reports. See [Assessment
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

No. AIF does **not** fetch, parse, or scan referenced documents, file attachments, images, or opaque
non-text content. Supported structured text parts have separate inspection rules. See
[Structured Content](structured-content.md).

## Configuration and operations

### How do I trust an OpenShift service CA for internal HTTPS models?

Inject or mount a PEM CA bundle **inside the AIF container**, then configure
`upstream_ca_bundle` for LLM requests (normal and streaming) and
`embedding_ca_bundle` for embedding requests. These settings add trust roots;
full TLS verification remains enabled. `SSL_CERT_FILE` alone is not sufficient
for AIF's current explicit loader. Restart AIF after a path or CA-content change.
See [Private HTTPS / custom CA bundles](private-ca-tls.md).

### Should `upstream_base_url` include `/v1/chat/completions`?

No. Use the provider's compatible base URL; AIF appends the chat-completions route. The correct `/v1` prefix
depends on the provider. See [Provider Compatibility](provider-compatibility.md).

### What if a Qdrant collection has a different vector size?

Startup validation reports a mismatch. Verify the actual embedding dimension and use a matching collection;
do not silently reuse vectors from a different model/dimension. See [Troubleshooting](troubleshooting.md).

### How do I validate the configuration or reload it?

AIF supports `--test-config`, masked `--print-config`, and nginx-style `SIGHUP` for reloadable settings.
Listener, request-body limit, inflight-request limit, graceful-shutdown timeout, and
custom CA bundle paths are restart-only settings. Rotating CA file contents also
requires a restart. See [Operations](operation.md) and [Configuration Reference](config-reference.md).

### What do `/healthz`, `/startupz`, and `/readyz` mean?

Liveness, successful initialization of configured required dependencies, and current readiness are distinct.
In Observe mode, optional evaluation cache failures do not block live upstream responses. See
[Operations](operation.md).

### What happens if Redis, Qdrant, embeddings, or a Guard fails?

The behavior depends on the enabled path, `*_fail_open`/`guard_fail_open`, and readiness settings.
Evaluation-only cache dependency failures are non-disruptive; unsupported recognized Privacy text structures
are not the same as a transport failure. See [Operations](operation.md) and
[Troubleshooting](troubleshooting.md).

## Optional VCAL modules and structured evidence

### Are optional VCAL modules required to use AIF?

No. The open-source AIF core works independently. **Privacy Guard**, **Security Guard**, and **Usage Guard**
can be enabled separately or together; **VCAL Audit** can receive structured evidence, and **VCAL Compliance**
supports governance workflows using retained evidence. These modules are separate commercial products.
See [Architecture](architecture.md) and [Audit Integration](audit-integration.md).

### Do I need VCAL Console to run AIF?

No. **VCAL Console** is optional and distinguishes actual production outcomes from potential Observe-mode
savings. It is included with an active annual VCAL Production Support subscription for AIF or an applicable
VCAL module license; no separate Console license is required.

### Which Guard handles what?

**Privacy Guard** detects and redacts or anonymizes supported sensitive text and can restore approved
placeholders; **Security Guard** detects and blocks supported text-based security risks; **Usage Guard**
allows, warns, or blocks requests according to organizational AI usage policy. Structured-content coverage
is specific to each module and its adapter. See [Structured Content](structured-content.md).

### Does Privacy Guard protect shared cached entries?

For supported requests with complete effective tenant and privacy policy identity, AIF scopes cache entries
accordingly and stores pre-restoration placeholder responses. Missing effective policy identity or Privacy
Guard transport fail-open bypasses caches. This is not a replacement for application authorization. See
[Structured Content](structured-content.md).

### Is evidence delivery to Audit durable?

Not end-to-end from AIF: evidence delivery uses a bounded **in-memory** queue with retries. Persistent
retention begins after successful VCAL Audit ingestion; events can be lost on queue overflow, exhausted
retries, or premature shutdown. See [Audit Integration](audit-integration.md).

## Metrics, troubleshooting, and releases

### Do Observe-mode savings estimates reduce upstream model calls?

No. In Observe mode, all allowed requests still reach the upstream provider. AIF estimates **potential**
avoided calls, tokens, and cost, kept separate from Enforce-mode outcomes. Savings estimates use configured
model prices, and net savings account for embedding overhead. See [Metrics and Costs](metrics-and-costs.md).

### Why do my dashboards have zero values?

Check that real traffic reaches AIF, that Prometheus scrapes its metrics endpoint, that the configured
provider is reachable, that cache is eligible, and that you are looking at the appropriate Observe-mode evaluation or
Enforce-mode production series. See [Troubleshooting](troubleshooting.md).

### Is AIF guaranteed to achieve the README benchmark rates?

No. The supplied benchmarks were controlled local runs, often using a simulated upstream. They are not
production capacity or savings guarantees. See [Benchmark report](../BENCHMARKS.md) and [Evaluation
validation](../EVALUATION_VALIDATION.md).

### Where should I start?

[Quick Start](quickstart.md) → [Configuration Reference](config-reference.md) → [Operations](operation.md).
For version changes, consult [Upgrading](upgrading.md) and
[GitHub Releases](https://github.com/vcal-project/ai-firewall/releases).
For guided deployment and product documentation, visit the
[AIF documentation portal](https://ai-firewall.docs.vcal-project.com/).
