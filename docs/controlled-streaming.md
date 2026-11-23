# Streaming Delivery: Progressive and Controlled SSE

AI Cost Firewall supports OpenAI-compatible chat completions with `"stream": true`. It can deliver generated content **progressively**, as upstream SSE arrives, or use **controlled delivery**, which assembles and approves the complete response before replaying SSE. Cached completions can also be rendered as OpenAI-compatible SSE. The historical filename of this guide is retained so existing documentation links continue to work.

## Delivery strategies

Configure the strategy with:

```conf
streaming_enabled true;
streaming_delivery_mode auto;
max_stream_upstream_bytes 8M;
```

| Mode | Behavior | When to use |
|---|---|---|
| `controlled` | Buffer provider SSE, assemble the completion, apply response controls, then replay approved SSE. | Strict pre-delivery inspection; the default for backward compatibility. |
| `progressive` | Forward eligible upstream SSE incrementally while assembling the canonical response for finalization and accounting. | Low-latency streams with a configuration compatible with progressive delivery. Explicit selection is rejected when incompatible configured capabilities are enabled. |
| `auto` | Choose a compatible strategy from configured capabilities; each request can still take a cache-replay or controlled path. | Deployments that should use progressive streaming where safe and fall back to controlled delivery as needed. |

**The default is `controlled` unless `streaming_delivery_mode` is explicitly set.** `streaming_enabled` only permits streaming requests; it does not convert ordinary JSON requests to SSE. Set `AIF_STREAMING_DELIVERY_MODE` when using environment-based configuration.

With `auto`, the selected strategy is not a promise that every request will forward upstream tokens progressively. Cache hits, guard capabilities, request-specific cache-store policy, and other eligibility checks affect the actual delivery path. For example, an eligible cache hit can be rendered as SSE without calling the upstream model. An enabled Guard requiring complete-response inspection selects controlled delivery.

## Approval and error boundaries

**Controlled delivery guarantee:** No generated response content leaves AIF until it has assembled and approved the complete response. A malformed, truncated, failed, oversized, idle, or over-duration provider stream fails before downstream SSE is committed.

**Progressive delivery trade-off:** An eligible stream can reach the client before upstream generation finishes. If a failure occurs **after** downstream SSE has started, the gateway cannot retract already delivered content or replace the committed HTTP status with a pre-stream error. Partial content must not be treated as a completed answer. Use controlled delivery when response approval before exposure is required.

Request-side Guard checks still happen before cache lookup or upstream inference. Progressive delivery is not a way to bypass enabled response controls: configurations requiring complete response inspection are handled using controlled delivery. Privacy placeholder restoration and other module constraints can also require controlled delivery.

## Cache-aware JSON and SSE reuse

- Redis exact cache and Qdrant semantic cache remain available for eligible requests, in both Observe and Enforce modes.
- JSON and SSE delivery share transport-independent exact-cache identity: `stream` and `stream_options` do not cause separate exact entries.
- A completed, cache-eligible response can be served as JSON or synthesized into OpenAI-compatible SSE terminated by `[DONE]`; it is not a byte-for-byte replay of the original provider framing.
- In **Observe mode**, shadow cache hits are recorded but the client still receives the live upstream result. In **Enforce mode**, an eligible cache hit can avoid an upstream call.
- Cache admission applies to **both streaming and non-streaming** response paths. A completion ending with `finish_reason: "length"`, or without usable assistant output, is delivered when valid but not cached. Valid tool-call completions can still be cache-eligible.
- Semantic reuse requires more than an embedding similarity threshold: compatible generation settings and earlier conversation context are checked separately. For example, changing `max_tokens` or `max_completion_tokens` prevents reuse across incompatible budgets. Unscoped legacy semantic entries do not satisfy the updated compatibility check.
- Suitable semantic hits may warm the Redis exact cache for the incoming request. Cache admission is checked on lookup as well as storage, including against previously stored unsuitable responses.

Semantic similarity remains a heuristic; application-specific distinctions can still matter. See [How It Works](how-it-works.md) and [Structured Content](structured-content.md).

## Limits and operational telemetry

`max_stream_upstream_bytes` caps the **cumulative provider SSE bytes** accepted during a request, not instantaneous parser-buffer size. For controlled provider SSE, `upstream_timeout_seconds` also limits the idle gap between chunks, and an absolute generation ceiling prevents indefinite drip-feed occupancy. Consult [Configuration Reference](config-reference.md) for the active limits and timeout semantics.

Monitor the actual delivery path with:

```text
aif_stream_delivery_selected_total{path="progressive_upstream"}
aif_stream_delivery_selected_total{path="controlled_upstream"}
aif_stream_delivery_selected_total{path="exact_cache"}
aif_stream_delivery_selected_total{path="semantic_cache"}
aif_stream_delivery_selected_total{path="policy_completion"}
```

Additional stream metrics record completed/failed streams, upstream chunks and bytes, upstream and client time to first byte, and generation duration. A reported configured or selected delivery strategy is **not** a substitute for checking the actual path counter. See [Metrics and Costs](metrics-and-costs.md) and [Troubleshooting](troubleshooting.md).
