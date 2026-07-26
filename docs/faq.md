# AI Cost Firewall — FAQ

This FAQ is for users browsing the `docs/` directory in the AI Cost Firewall GitHub repository. It gives quick answers for installation, configuration, caching, observability, troubleshooting, and optional VCAL Security Guard / VCAL Privacy Guard integration and VCAL Audit evidence delivery.

For full configuration details, see:

```text
docs/config-reference.md
docs/provider-compatibility.md
docs/operation.md
docs/troubleshooting.md
```

---

## General

### What is AI Cost Firewall?

AI Cost Firewall is an OpenAI-compatible gateway for reducing repeated LLM API calls, controlling cost, improving latency for repeated requests, and exposing operational visibility.

It sits between applications and upstream LLM providers.

Typical flow:

```text
Application
→ AI Cost Firewall
→ optional Security Guard / Privacy Guard orchestration
→ cache lookup
→ upstream LLM provider only when needed
```

The firewall behaves similarly to:

```text
nginx for LLM APIs
```

but with LLM-specific controls such as exact cache, semantic cache, model pricing, token-cost metrics, request limits, and optional Security Guard / Privacy Guard orchestration.

---

### What problem does AI Cost Firewall solve?

LLM applications often recompute the same or similar answers many times.

AI Cost Firewall helps reduce:

- repeated upstream LLM calls
- unnecessary token usage
- latency for repeated questions
- lack of visibility into cache behavior
- lack of visibility into estimated cost savings
- uncontrolled LLM traffic patterns during pilots and production evaluations

Only cache misses are forwarded upstream.

---

### Which endpoint is currently supported?

Currently supported:

```text
/v1/chat/completions
```

The API is OpenAI-compatible, so many existing SDKs and applications can point to AI Cost Firewall instead of calling the upstream provider directly.

---

### Which providers are supported?

AI Cost Firewall supports practical OpenAI-compatible upstream and embedding providers, including:

- OpenAI
- Ollama
- LM Studio
- vLLM
- LiteLLM
- OpenRouter
- local OpenAI-compatible gateways
- self-hosted OpenAI-compatible gateways

AI Cost Firewall uses a flat OpenAI-compatible configuration model. It does not require provider-specific configuration blocks.

Example:

```conf
upstream_provider openai_compatible;
upstream_base_url https://api.openai.com;
upstream_api_key sk-your-api-key;

embedding_provider openai_compatible;
embedding_base_url https://api.openai.com;
embedding_api_key sk-your-api-key;
embedding_model text-embedding-3-small;
```

---

### Can the chat provider and embedding provider differ?

Yes.

For example, you can use a cloud provider for chat completions and a local model server for embeddings:

```conf
upstream_base_url https://api.openai.com;
embedding_base_url http://ollama:11434/v1;
```

Or the opposite:

```conf
upstream_base_url http://ollama:11434/v1;
embedding_base_url https://api.openai.com;
```

This is useful for:

- reducing embedding cost
- keeping embeddings local
- testing hybrid deployments
- comparing local and cloud model behavior

---

### What API key should be used for local providers?

Local providers often do not require authentication.

Accepted placeholder values:

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

When placeholder values are used, AI Cost Firewall does not forward upstream bearer authorization headers.

---

## Caching

### How does caching work?

AI Cost Firewall uses a two-layer cache strategy:

1. exact cache
2. semantic cache

Typical request flow:

```text
Client
→ AI Cost Firewall
→ Redis / Valkey exact cache
→ Qdrant semantic cache
→ OpenAI-compatible upstream
```

Only cache misses reach the upstream provider.

---

### What is the exact cache?

The exact cache stores responses for identical normalized requests.

Typical flow:

```text
normalized request hash
→ Redis / Valkey
→ cached response
```

Exact cache is useful when applications repeat the same prompt and request parameters.

---

### What is the semantic cache?

The semantic cache stores embeddings of normalized prompt text in Qdrant.

If a new prompt is semantically similar to a previously cached prompt, AI Cost Firewall may reuse the cached response even if the text is not identical.

Typical flow:

```text
prompt embedding
→ Qdrant similarity search
→ cached response if similarity is high enough
```

The similarity threshold is configured with:

```conf
semantic_similarity_threshold 0.92;
```

Higher values are stricter and produce fewer semantic hits.

Lower values increase reuse but may reduce precision.

---

### Do I need both Redis and Qdrant?

No.

Minimum deployment:

- AI Cost Firewall
- Redis or Valkey

Qdrant is required only when semantic caching is enabled.

---

### Can semantic cache be disabled?

Yes.

```conf
semantic_cache_enabled false;
```

When semantic cache is disabled:

- embeddings are skipped
- Qdrant is not required
- exact cache can still be used

---

### Can exact cache be disabled?

Yes.

```conf
exact_cache_enabled false;
```

This is useful for debugging, semantic-cache-only testing, or upstream pass-through evaluations.

---

### Can cache storage be disabled while lookup remains enabled?

Yes, if your version includes explicit store controls.

Examples:

```conf
exact_cache_store_enabled false;
semantic_cache_store_enabled false;
```

This is useful when you want to test cache lookup behavior without adding new entries.

---

### Can I bypass cache for one request?

Yes.

By default:

```http
X-AIF-Cache-Bypass: true
```

This skips exact lookup, semantic lookup, and cache storage for that request.

The bypass header name can be configured:

```conf
cache_bypass_header X-AIF-Cache-Bypass;
```

---

### Are streaming responses cached?

Streaming support depends on the active request path and configuration.

In standalone gateway mode, streaming requests may be forwarded upstream, but streaming responses are generally not stored in semantic cache.

When VCAL Privacy Guard orchestration is enabled, use non-streaming requests unless your deployed version explicitly documents streaming support for the privacy-guard path. Privacy restoration requires full assistant message content, which is not naturally available until a stream is complete.

---

### Are tool-calling and structured outputs cached?

Semantic cache may be skipped for:

- tool-calling requests
- function-calling requests
- structured outputs
- request shapes that contain non-deterministic or complex structured data

These request types often depend on exact structure, tool schemas, or execution context, so reuse must be more conservative.

---

## Configuration

### What configuration style does AI Cost Firewall use?

AI Cost Firewall uses nginx-style directives:

```conf
directive value;
```

Example:

```conf
listen_addr 0.0.0.0:8080;
```

Directives are case-sensitive and must end with a semicolon.

---

### What is the provider configuration model?

AI Cost Firewall uses flat OpenAI-compatible provider configuration.

Use `upstream_*` directives for chat completions:

```conf
upstream_provider openai_compatible;
upstream_base_url https://api.openai.com;
upstream_api_key sk-your-api-key;
```

Use `embedding_*` directives for semantic-cache embeddings:

```conf
embedding_provider openai_compatible;
embedding_base_url https://api.openai.com;
embedding_api_key sk-your-api-key;
embedding_model text-embedding-3-small;
```

Provider-specific configuration blocks are not required.

---

### Should the base URL include `/v1/chat/completions`?

No.

Configure either the provider root URL or the `/v1` base path.

Correct examples:

```conf
upstream_base_url https://api.openai.com;
upstream_base_url http://ollama:11434/v1;
```

Wrong example:

```conf
upstream_base_url http://ollama:11434/v1/chat/completions;
```

AI Cost Firewall appends OpenAI-compatible endpoint paths internally.

---

### Which Qdrant port should be used?

AI Cost Firewall uses Qdrant gRPC on:

```text
6334
```

Qdrant REST usually runs on:

```text
6333
```

Example:

```conf
qdrant_url http://qdrant:6334;
```

---

### Why does startup fail with vector-size mismatch?

The embedding dimension must match:

```conf
qdrant_vector_size
```

Examples:

| Embedding Model | Vector Size |
|---|---:|
| `text-embedding-3-small` | 1536 |
| `nomic-embed-text` | 768 |

A mismatch may produce an error similar to:

```text
existing collection vector size does not match qdrant_vector_size
```

Fix it by either:

- setting `qdrant_vector_size` to the embedding model dimension
- using a different Qdrant collection
- recreating the collection if it was created with the wrong vector size

---

### Can configuration be validated before startup?

Yes.

AI Cost Firewall supports validation similar to:

```text
nginx -t
```

Example:

```bash
cargo run -- --config configs/ai-firewall.conf --test-config
```

Expected output:

```text
configuration OK
```

Static validation checks include:

- syntax
- required directives
- semantic cache configuration
- request-size parsing
- selected runtime controls
- model-pricing configuration shape

Static validation does not contact runtime dependencies.

---

### Can the loaded configuration be inspected?

Yes.

Example:

```bash
cargo run -- --config configs/ai-firewall.conf --print-config
```

Sensitive values are masked automatically.

---

### Can configuration be reloaded without restart?

Yes.

AI Cost Firewall supports nginx-style reloads using `SIGHUP`.

Docker Compose example:

```bash
docker compose kill -s HUP ai-firewall
```

Binary deployment example:

```bash
kill -HUP $(pgrep ai-firewall)
```

Some settings may still require process or container restart depending on how the deployment injects configuration and environment variables.

---

### Do containers need to be recreated after config or environment changes?

Often, yes.

If you change files mounted into the container, a `SIGHUP` reload may be enough for reloadable configuration.

If you change Docker Compose environment variables, image tags, networks, mounted files, or service definitions, recreate the affected containers:

```bash
docker compose up -d --force-recreate ai-firewall
```

For dependency changes such as Redis, Qdrant, Prometheus, or VCAL Privacy Guard, recreate the affected services as needed.

---

## Cost and savings

### How are cost savings calculated?

Cost savings are calculated for cached chat-completion responses.

Inputs include:

- prompt tokens
- completion tokens
- configured `model_price`
- optional `embedding_price`

Example:

```conf
model_price gpt-4o-mini-2024-07-18 0.15 0.60;
embedding_price 0.020;
```

The `model_price` values are USD per 1M tokens.

---

### What is gross vs net savings?

Gross savings are avoided chat-completion costs.

Metric:

```text
aif_gross_saved_micro_usd_total
```

Embedding overhead is the cost of generating embeddings for semantic lookup.

Metric:

```text
aif_embedding_overhead_micro_usd_total
```

Net savings are gross savings minus embedding overhead.

Metric:

```text
aif_net_saved_micro_usd_total
```

---

### Why do cost metrics show zero?

The most common reason is that `model_price` does not exactly match the upstream model name.

Example upstream model:

```text
gpt-4o-mini-2024-07-18
```

Matching configuration:

```conf
model_price gpt-4o-mini-2024-07-18 0.15 0.60;
```

If the model name differs, AI Cost Firewall may allow or reject the request depending on:

```conf
allow_unknown_models_pass_through false;
```

When unknown models are not allowed, requests using unconfigured models are rejected.

---

## Observability

### What metrics are exposed?

Prometheus metrics are available at:

```text
/metrics
```

Example metrics include:

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

Depending on the version and enabled features, AI Cost Firewall may also export:

- semantic diagnostics
- runtime health metrics
- timeout metrics
- embedding metrics
- cache bypass metrics
- guard orchestration metrics

---

### Can metrics be protected?

Yes, if metrics authentication is enabled in your version.

Example:

```conf
metrics_auth_required true;
metrics_auth_token your-token;
```

When enabled, `/metrics` requires a bearer token.

---

### How do I check health, readiness, and version?

Use:

```bash
curl -s http://localhost:8080/healthz
curl -s http://localhost:8080/readyz
curl -s http://localhost:8080/version
```

Use `/healthz` to check whether the process is alive.

Use `/readyz` to check whether required dependencies are ready.

Use `/version` to confirm the running release and compatibility model.

---

## Request limits and timeouts

### Can request body size be limited?

Yes.

Example:

```conf
max_request_body_bytes 1M;
```

This protects the gateway from unexpectedly large request bodies.

---

### Can prompt size be limited?

Yes, if your version includes prompt-length controls.

Example:

```conf
max_prompt_chars 20000;
```

This helps prevent oversized prompts from reaching cache lookup, embedding generation, or upstream providers.

---

### Can upstream and embedding timeouts be configured separately?

Yes, if your version includes separate timeout controls.

Examples:

```conf
upstream_timeout_seconds 120;
embedding_timeout_seconds 30;
```

Older configurations may still use:

```conf
request_timeout_seconds 120;
```

for compatibility.

---

---

### What changed in v0.4.1?

AI Cost Firewall v0.4.1 adds production wiring for buffered delivery of structured evidence events to VCAL Audit.

The release includes:

- evidence schema `vcal.evidence.event` version `1.1`
- stable `trace_id` correlation
- exactly one terminal `request.completed` or `request.failed` event for each received trace
- bounded in-memory queueing
- configurable batching and flush intervals
- HTTP timeout, retry, and backoff controls
- configurable Audit endpoint, API key, and producer instance ID

---

### Is VCAL Audit required?

No. AI Cost Firewall remains usable as a standalone caching and cost-control gateway.

VCAL Audit is an optional commercial evidence receiver for retained event storage, trace reconstruction, NDJSON export, and hash-chain verification.

---

### Is Audit delivery guaranteed?

Not in v0.4.1.

The AI Firewall sender uses a bounded in-memory queue. An undelivered batch can be dropped after retry exhaustion, when the queue is full, or if the process terminates before queued events are flushed.

Deployments requiring guaranteed producer-side delivery need a future disk-backed spool or durable broker.

---

### Are streaming requests supported?

No. AI Cost Firewall v0.4.1 supports non-streaming chat completions only.

Requests with:

```json
{"stream": true}
```

are rejected with HTTP `422` before cache, guard, or upstream processing.

## VCAL Security Guard

### What is VCAL Security Guard?

VCAL Security Guard is an optional enterprise security module that can be integrated with AI Firewall.

It uses deterministic, auditable rules to detect text-based LLM security risks such as prompt injection, jailbreak attempts, system-prompt extraction attempts, unsafe tool-use instructions, data-exfiltration attempts, and common cyber-abuse patterns.

It is a rule-based first control layer. It should not be described as complete prompt-injection or universal jailbreak prevention.

---

### Is VCAL Security Guard required to use AI Firewall?

No. AI Firewall can run without VCAL Security Guard.

---

### How does VCAL Security Guard work with AI Firewall?

When enabled, AI Firewall can call Security Guard before cache lookup or upstream forwarding.

If the request is blocked, AI Firewall returns a structured error such as:

```json
{
  "error": {
    "code": 403,
    "guard": "security",
    "type": "security_request_blocked",
    "stage": "request",
    "rule_id": "VSG-PA-003"
  }
}
```

Security Guard can also scan assistant responses before Privacy Guard restore and before the response is returned to the client.

---

### What Security Guard settings are used by AI Firewall?

```conf
security_guard_enabled true;
security_guard_url http://vcal-security-guard:8091;
security_guard_api_key your-shared-api-key;
security_guard_timeout_seconds 3;
guard_fail_open false;
```

For production-like enforcement tests, Security Guard should normally use:

```text
VCAL_SECURITY_GUARD_DEFAULT_MODE=enforce
```

---
## VCAL Privacy Guard

### What is VCAL Privacy Guard?

VCAL Privacy Guard is an optional privacy-protection module that can be integrated with AI Cost Firewall.

It can detect, redact, anonymize, and restore sensitive values in LLM traffic before requests are sent to an upstream provider.

Examples of sensitive values include:

- emails
- IP addresses
- phone numbers
- API keys
- bearer tokens
- JWTs
- private keys
- credit-card-like values
- URL tokens
- custom sensitive patterns

---

### Is VCAL Privacy Guard required to use AI Cost Firewall?

No.

AI Cost Firewall can run without VCAL Privacy Guard.

VCAL Privacy Guard is useful for deployments that need stronger privacy controls, anonymization, redaction, or sensitive-data handling before prompts reach an upstream LLM provider.

---

### How does VCAL Privacy Guard work with AI Cost Firewall?

When enabled, AI Cost Firewall can call VCAL Privacy Guard before forwarding a request upstream.

Example original prompt:

```text
Analyze login from 185.23.10.5 by john@example.com
```

Example prompt sent upstream:

```text
Analyze login from [IP_1] by [EMAIL_1]
```

If response restoration is enabled, AI Cost Firewall can restore placeholders in the final assistant response before returning it to the client.

Example upstream response:

```text
[EMAIL_1] logged in from [IP_1]
```

Example final restored response:

```text
john@example.com logged in from 185.23.10.5
```

---

### What Privacy Guard settings are used by AI Cost Firewall?

Typical settings include:

```conf
privacy_guard_enabled true;
privacy_guard_url http://vcal-privacy-guard:8090;
privacy_guard_api_key your-shared-api-key;
privacy_guard_mode anonymize;
privacy_guard_restore_enabled true;
privacy_guard_timeout_seconds 10;
guard_fail_open false;
```

Common environment variables include:

```text
AIF_PRIVACY_GUARD_ENABLED=true
AIF_PRIVACY_GUARD_URL=http://vcal-privacy-guard:8090
AIF_PRIVACY_GUARD_API_KEY=your-shared-api-key
AIF_PRIVACY_GUARD_MODE=anonymize
AIF_PRIVACY_GUARD_RESTORE_ENABLED=true
AIF_GUARD_FAIL_OPEN=false
```

Use matching API keys on both sides.

---

### Which Privacy Guard mode should I use?

Common modes are:

```text
detect_only
redact
anonymize
```

Use `detect_only` to identify sensitive data without changing text.

Use `redact` to remove or mask sensitive values.

Use `anonymize` to replace sensitive values with reversible placeholders such as:

```text
[EMAIL_1]
[IP_1]
```

For AI Firewall orchestration with response restoration, `anonymize` is usually the most useful mode.

---

### What happens if VCAL Privacy Guard is unavailable?

This depends on:

```conf
guard_fail_open
```

Fail-open behavior allows the request to continue if the guard is unavailable.

Fail-closed behavior rejects the request if the guard is unavailable.

For privacy-sensitive deployments, fail-closed behavior is recommended:

```conf
guard_fail_open false;
```

This prevents unprocessed sensitive data from being sent upstream when the privacy guard cannot process the request.

---

### Does VCAL Privacy Guard replace application security controls?

No.

VCAL Privacy Guard is an additional privacy and data-protection layer. It does not replace:

- application authorization
- identity and access management
- data classification
- audit logging
- encryption
- network security
- compliance review

Use it as part of a broader security and governance model.

---

### Does Privacy Guard affect semantic cache safety?

It can improve privacy posture because sensitive values can be replaced before upstream calls and cache operations.

For example:

```text
john@example.com
```

can become:

```text
[EMAIL_1]
```

This helps avoid storing or forwarding raw sensitive values.

Deployments should still review cache-retention settings, access controls, metrics exposure, logs, and backup/snapshot handling.

---

## Full Security + Privacy Mode

### Can AI Firewall use Security Guard and Privacy Guard together?

Yes. AI Firewall v0.4.1 can orchestrate both modules in a single request/response flow:

```text
Client
→ AI Firewall
→ Security Guard request scan
→ Privacy Guard anonymize/redact
→ exact/semantic cache or upstream LLM
→ Security Guard response scan
→ Privacy Guard restore
→ Client
```

This lets the firewall block malicious prompts before privacy mapping or upstream processing, while still anonymizing sensitive text before it reaches Redis, Qdrant, semantic cache payloads, or the upstream LLM.

---

### What happens to streaming requests when guards are enabled?

Guarded streaming requests are rejected in the current guard contract.

Use non-streaming requests when Security Guard or Privacy Guard orchestration is enabled.

---

### What happens if a request contains images or other non-text content?

The current guard modules inspect text content only.

Non-text content such as images, audio, video, and binary payloads may be preserved where possible and forwarded upstream, but it is not scanned, anonymized, or classified by AI Firewall guard modules.

If the client application extracts OCR text, captions, or metadata from non-text content and sends that extracted text through AI Firewall, that extracted text can be scanned and anonymized normally.

---
## Troubleshooting

### Why do I get `upstream_not_found`?

Usually this means the provider returned:

```text
404
```

The most common cause is a wrong base URL.

Correct:

```conf
upstream_base_url http://ollama:11434/v1;
```

Wrong:

```conf
upstream_base_url http://ollama:11434/v1/chat/completions;
```

AI Cost Firewall appends endpoint paths internally.

---

### Why do I get `upstream_tls_error`?

TLS verification failed.

Typical causes:

- self-signed certificate
- hostname mismatch
- invalid SAN
- corporate TLS interception
- proxy or gateway TLS rewriting

For trusted local networks, local providers often work more reliably using:

```text
http://
```

For production, prefer properly configured TLS.

---

### Why is semantic cache not producing hits?

Common causes:

- prompts are not similar enough
- threshold is too strict
- semantic cache is disabled
- embeddings are unavailable
- Qdrant is unavailable
- entries expired
- request contains tool calls or structured output
- request parameters changed enough to prevent safe reuse

Inspect metrics such as:

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

### Why are requests still reaching upstream providers?

Common causes:

- first request is not cached yet
- prompt changed
- request parameters changed
- semantic similarity is below threshold
- cache bypass header is set
- exact cache is disabled
- semantic cache is disabled
- streaming or tool-calling path bypasses semantic storage
- cache entry expired

Important request fields include:

- model
- messages
- temperature
- top_p
- max_tokens
- tools
- response format

---

### Why can’t the firewall connect to Redis or Qdrant in Docker Compose?

Inside Docker Compose:

```text
localhost
```

refers to the container itself.

Use Docker service names instead.

Correct:

```conf
redis_url redis://redis:6379;
qdrant_url http://qdrant:6334;
```

Wrong:

```conf
redis_url redis://127.0.0.1:6379;
qdrant_url http://127.0.0.1:6334;
```

---

### Why is the container unhealthy even though logs look normal?

Check the healthcheck command in Docker Compose.

Common causes:

- healthcheck points to the wrong port
- healthcheck uses `localhost` from the wrong container context
- `/readyz` fails because Redis, Qdrant, upstream, or embeddings are unavailable
- service needs more startup time
- container image does not include the tool used by the healthcheck, such as `curl`

Check:

```bash
docker compose ps
docker compose logs ai-firewall
curl -s http://localhost:8080/healthz
curl -s http://localhost:8080/readyz
```

---

## Operations

### What happens during graceful shutdown?

Shutdown sequence:

1. readiness is disabled
2. new requests are rejected
3. in-flight requests continue
4. process exits after the configured timeout

Configured by:

```conf
graceful_shutdown_timeout_seconds 10;
```

---

### What deployment examples are included?

Ready-to-run deployment examples are available under:

```text
deploy/examples/
```

Common patterns include:

```text
openai-cloud/
local-ollama/
hybrid-openai-local-embeddings/
openrouter/
local-full-stack/
```

Each example may include:

- Docker Compose deployment
- minimal configuration
- example requests
- expected behavior
- expected metrics

---

### Is AI Cost Firewall production-ready?

AI Cost Firewall is suitable for pilots, demos, controlled evaluations, and early production-style deployments where operators can validate behavior, dependencies, limits, and observability before broader rollout.

The project includes:

- Rust async runtime
- OpenAI-compatible gateway behavior
- Redis / Valkey exact cache
- Qdrant semantic cache
- Prometheus metrics
- Grafana dashboards
- health and readiness endpoints
- version reporting
- graceful shutdown
- configuration validation
- configuration reload
- runtime diagnostics

For production use, review and harden:

- authentication and authorization around exposed services
- Redis and Qdrant network access
- metrics exposure
- TLS and mTLS requirements
- timeout and request-size limits
- cache-retention policy
- backup and snapshot handling
- log handling for sensitive data
- Privacy Guard fail-open/fail-closed behavior if enabled

---

## Where can I learn more?

Documentation:

```text
docs/
```

Important documents:

```text
docs/quickstart.md
docs/config-reference.md
docs/provider-compatibility.md
docs/operation.md
docs/troubleshooting.md
```

Source code:

```text
https://github.com/vcal-project/ai-firewall
```
