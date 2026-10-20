# How AI Cost Firewall Works

This guide explains the **request and cache decision path**. It complements the
[Architecture](architecture.md) overview, the [Configuration Reference](config-reference.md), and the
focused [Evaluation Mode](evaluation-mode.md) and [Controlled Streaming](controlled-streaming.md) guides.

## Processing sequence

A typical successful request is processed as follows:

1. Parse the OpenAI-compatible chat request, validate body/prompt limits, and preserve completion-affecting JSON fields.
2. Run any enabled **Security Guard request scan**, **Privacy Guard scan/anonymization**, and **Usage Guard request policy evaluation**, in that order.
3. Determine whether the request bypasses cache and whether exact and semantic caching are eligible.
4. Query exact cache in Redis/Valkey; if eligible and necessary, query semantic cache in Qdrant using an embedding from the configured embedding provider.
5. In `enforce`, use an eligible cache hit; otherwise request a live response from the chat provider. In `observe`, always request the live response and record the would-have cache decision separately.
6. Normalize the completion; run enabled response Security scanning; write eligible cache state (production or isolated shadow), then restore Privacy placeholders using the current request mapping.
7. Record usage, cost, metrics, and evidence; return JSON or replay the approved completion as SSE. Evidence delivery to optional VCAL Audit is asynchronous.

The ordering is important: Security Guard may block before any model/cache call, and Privacy Guard
placeholder restoration occurs **after** eligible pre-restoration cache storage. Usage Guard is request-side
in the AIF orchestration described by this documentation. See [Structured Content](structured-content.md)
for the limits of content inspection.

## Cache decision summary

```mermaid
flowchart TD
    A[Validated and guard-approved request] --> B{Bypass or unsafe for cache?}
    B -->|Yes| U[Call upstream]
    B -->|No| C{Exact cache hit?}
    C -->|Yes| D[Eligible cached completion]
    C -->|No| E{Semantic eligible?}
    E -->|No| U
    E -->|Yes| F[Embed + Qdrant candidate lookup]
    F -->|Hit| D
    F -->|Miss| U
    D --> G{AIF mode?}
    G -->|Enforce| R[Apply response controls and deliver]
    G -->|Observe| U
    U --> H[Assemble/normalize upstream response]
    H --> J[Response controls + eligible cache updates]
    J --> R
```

The diagram omits request error branches, guard-specific behavior, and exact/shadow warming to stay
readable. In Observe Mode, `D` is a *hypothetical* decision and is never returned to the client.

## Request identity and cache eligibility

### Exact request identity

The exact cache hashes normalized request data with SHA-256. Completion-affecting OpenAI-compatible request
and message extension fields are preserved in the identity: examples include tools, tool choice, tool-call
IDs, `response_format`, reasoning settings, and other accepted extensions. Delivery-only `stream` and
`stream_options` do not split JSON and SSE cache identities.

Illustrative Redis key:

```text
aif:exact:<sha256>
```

An exact hit can avoid embeddings and an upstream chat call in `enforce`. Exact cache may also serve
eligible structured-content requests; the full JSON content participates in the key. `exact_cache_enabled`,
`exact_cache_store_enabled`, TTL, and `exact_cache_fail_open` control the path. See [Configuration
Reference](config-reference.md).

### Semantic request eligibility

AIF derives semantic text from eligible **plain-string messages**. Requests containing non-string message
content, `tools`, or `response_format` bypass semantic lookup and store. This restriction prevents a
similarity result from ignoring completion-critical structured information.

For eligible text requests, AIF requests an embedding and searches Qdrant. A candidate must satisfy the
configured similarity threshold, be unexpired, contain a valid cached payload, and meet the applicable
model/identity/scope requirements. Example:

```text
Query:       Explain Redis briefly
Candidate:   What is Redis used for?
Similarity:  0.94
Threshold:   0.92
Result:      candidate meets the similarity threshold
```

Crossing the threshold is **necessary but not sufficient**: the other eligibility checks still apply.
Similarity thresholds require application-specific tuning, particularly where apparently similar questions
can require different factual answers.

### Embedding vector and retention

The embedding model and Qdrant collection must use the same vector dimension. Verify the actual vector
length returned by the configured endpoint rather than assuming dimensions from a model's name. The
embedding endpoint may be different from the chat endpoint.

Semantic entries include insertion and expiry metadata. Expired entries are excluded from reuse even if
physical collection cleanup has not run. `semantic_cache_retention_seconds` controls retention; cleanup is
an operational task, not a correctness prerequisite. See [Operations](operation.md).

### Per-request bypass

A request carrying the configured truthy cache-bypass header skips **both exact and semantic lookup and
storage**, including isolated Observe-mode operations:

```http
X-AIF-Cache-Bypass: true
```

`true`, `1`, `yes`, and `on` are accepted by the documented default behavior. This is distinct from
disabling a cache backend globally. Request bypass counts are recorded in `aif_cache_bypass_requests_total`.

## Enforce Mode and Observe Mode

| Decision | `enforce` live path | `observe` live path / shadow state |
|---|---|---|
| Exact hit | Return eligible Redis response | Record would-have exact hit; call upstream; do not run shadow semantic lookup |
| Exact miss, semantic hit | Return eligible Qdrant response, optionally warm exact | Record would-have semantic hit; call upstream; potentially warm shadow exact |
| Both miss | Call upstream and store if eligible | Call upstream and populate only shadow caches if eligible |
| Bypass | Call upstream; do not look up/store | Call upstream; no shadow lookup/store |

Observe Mode maintains separate Redis key space and Qdrant collection, does not promote shadow entries
automatically into production, and keeps evaluation estimates separate from real savings. Evaluation-only
dependency failures are non-blocking for live inference. See [Evaluation Mode](evaluation-mode.md).

## Privacy and other guard effects on caching

When Privacy Guard is active, supported text is inspected before cache lookup. If Privacy Guard provides the
complete effective **tenant and policy scope** (policy ID, version, hash), AIF isolates exact/semantic cache
reuse by that effective identity. If the successful scan lacks complete policy identity, or a Privacy Guard
transport failure is allowed to fail open, cache lookup and store are bypassed for that request.

When anonymization is active, cache storage retains the **placeholder-containing** response, not a restored
response containing the current user's sensitive data. Restoration must use the mapping created for the
**current** request. This protects against cross-request placeholder restoration. See [Structured
Content](structured-content.md).

Security Guard and Usage Guard coverage of nested structured content must not be inferred from Privacy
Guard's capabilities. Do not assume AIF downloads or scans referenced attachments.

## Upstream and response paths

For a cache miss/bypass (and **every** Observe request), the gateway calls the configured OpenAI-compatible
upstream. Set its **base URL**, not a URL that already ends in `/v1/chat/completions`. Example compatible
base URLs may be `https://api.openai.com`, `http://ollama:11434/v1`, or `http://vllm:8000/v1`, depending on
the provider. See [Provider Compatibility](provider-compatibility.md).

For `stream=false` or omitted `stream`, AIF processes a JSON completion. For `stream=true`, it consumes
provider SSE, assembles the whole response, applies response controls, then generates downstream SSE with
`[DONE]`. A client does **not** receive upstream tokens immediately as generated. Provider failures before
approval do not expose partial content. See [Controlled Streaming](controlled-streaming.md).

Independent cache store switches allow lookup without writing new entries. Responses are normalized before
storage and response processing; unsuccessful or otherwise ineligible flows must not be assumed cacheable.

## Limits, failures, and operational signals

- `max_request_body_bytes` caps the HTTP body; `max_prompt_chars` caps parsed prompt content. Configuration
  validity is checked at startup.
- `exact_cache_fail_open` and `semantic_cache_fail_open` determine whether runtime cache problems skip the
  affected path or fail the request under enforcement. They do not disable static validation.
- `/startupz` and `/readyz` distinguish required dependency initialization from current readiness; consult
  [Operations](operation.md) when designing container probes.
- Metrics distinguish exact/semantic hits, misses, bypasses, costs, latency, guard results, and
  evaluation-only opportunities. See [Metrics and Costs](metrics-and-costs.md).
- Structured evidence contains per-trace decisions and one terminal event. Optional HTTP delivery to VCAL
  Audit can lose events when its bounded in-memory delivery queue is exhausted. See [Evidence and
  Audit](audit-integration.md).

## Related guides

[Architecture](architecture.md) · [Configuration Reference](config-reference.md) · [Evaluation
Mode](evaluation-mode.md) · [Structured Content](structured-content.md) ·
[Troubleshooting](troubleshooting.md)
