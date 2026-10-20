# Controlled Streaming

AI Cost Firewall supports **controlled OpenAI-compatible chat-completion streaming** with `"stream": true`. Controlled streaming accepts provider SSE internally, assembles the complete response, applies response controls, and only then replays an OpenAI-compatible Server-Sent Events (`text/event-stream`) response to the client.

## Approval guarantee

The core guarantee is:

> No generated response content leaves AI Cost Firewall until AI Cost Firewall has approved the complete response.

## Request and response behavior

Controlled streaming uses the same request controls, cache identity, response controls, Privacy restoration, accounting, and evidence pipeline as non-streaming requests:

- Security Guard, Privacy Guard, and Usage Guard request-side processing runs before cache lookup or the upstream call.
- Exact and semantic cache lookup/store remain active for streaming requests. JSON and SSE delivery share transport-independent cache identity, so an eligible cached completion can be reused across `stream=false` and `stream=true`.
- On a cache miss, AI Cost Firewall requests provider SSE and consumes it internally. Provider chunks are parsed and assembled into the canonical chat-completion response before downstream delivery.
- Security Guard response scanning runs on the complete assembled or cached response before any generated response content is committed to the client.
- Eligible cache-miss responses are stored using the existing pre-Privacy-restore cache semantics.
- Privacy Guard placeholder restoration runs before controlled SSE delivery, including flows that created an anonymization mapping on the request path.
- Usage and model-cost accounting are derived from the assembled response. AI Cost Firewall requests upstream usage data for internal accounting; a downstream usage chunk is replayed when the client requests `stream_options.include_usage`.
- After all controls and accounting complete successfully, AI Cost Firewall encodes the approved canonical response as OpenAI-compatible SSE and terminates it with `[DONE]`.
- If the provider stream fails, is malformed, is truncated, exceeds the configured cumulative upstream SSE byte limit, becomes idle for too long, or exceeds the absolute generation ceiling before approval, AI Cost Firewall returns a normal HTTP error and does not expose partial provider content to the client.

## Configuration and limits

Controlled streaming is enabled by default and can be configured with:

```text
streaming_enabled true;
max_stream_upstream_bytes 8M;
upstream_timeout_seconds 120;
```

`streaming_enabled` permits clients to use `"stream": true`; it does not force streaming for ordinary requests. Requests with `stream=false` or no `stream` field continue to use the normal JSON completion path.

`max_stream_upstream_bytes` limits the **cumulative provider SSE bytes accepted for one controlled streaming request**. It is a total-response-size limit, not a measurement of instantaneous parser buffer occupancy.

For controlled provider streams, `upstream_timeout_seconds` also acts as the maximum idle gap between provider SSE body chunks after response headers have been received. In addition, controlled generation has a 15-minute absolute ceiling so a provider cannot hold an upstream concurrency permit indefinitely by continuously drip-feeding data.

Provider SSE support is required only for requests that use `"stream": true`. An OpenAI-compatible provider without streaming support remains usable for ordinary non-streaming chat-completion requests.

## Operational characteristics

Operational characteristics:

- controlled streaming prioritizes response-control guarantees over token-by-token immediacy; client SSE delivery begins only after upstream generation and response controls complete
- downstream SSE is generated from the canonical response and is OpenAI-compatible, but it is not guaranteed to be byte-for-byte identical to the provider's original SSE framing
- fragmented content and tool-call deltas are reconstructed before replay
- upstream and client time-to-first-byte are measured separately
- provider SSE chunks and cumulative bytes, generation duration, timeout/failure outcomes, upstream response size, approved client-buffer size, request errors, and terminal stream lifecycle are exposed through Prometheus metrics and structured evidence

See [Operational behavior](operation.md) for runtime failure handling and [Configuration Reference](config-reference.md) for streaming directives.
