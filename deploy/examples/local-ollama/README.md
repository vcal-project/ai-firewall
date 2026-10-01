# Local Ollama provider

Run AI Cost Firewall against a local OpenAI-compatible Ollama endpoint for both chat completions and embeddings.

## Files

- `docker-compose.yml` — runnable local deployment stack for this pattern.
- `docker-compose.observability.yml` — optional Prometheus + Grafana overlay.
- `ai-firewall.conf` — minimal AI Cost Firewall configuration for this pattern.
- `README.md` — setup, test request, expected behavior, and expected metrics.

---

## Start

Run commands from this directory:

```bash
cd deploy/examples/local-ollama
```

Start the deployment:

```bash
docker compose up -d
```

Pull the local chat and embedding models:

```bash
docker compose exec ollama ollama pull llama3.2:3b
docker compose exec ollama ollama pull nomic-embed-text
```

Restart the firewall after the models become available:

```bash
docker compose restart firewall
```

---

## Optional Observability Stack

Start Prometheus and Grafana:

```bash
docker compose \
  -f docker-compose.yml \
  -f docker-compose.observability.yml \
  up -d
```

Grafana:

```text
http://localhost:3000
```

Prometheus:

```text
http://localhost:9090
```

The dashboards are loaded from:

```text
deploy/grafana/dashboards/
```

including:

```text
ai-cost-firewall-overview.json
ai-cost-firewall-diagnostics.json
```

---

## Validate the Deployment

Validate that the firewall is listening:

```bash
curl http://localhost:8080/healthz
curl http://localhost:8080/readyz
curl http://localhost:8080/version
```

Expected:

```text
OK
READY
```

The version endpoint should report AI Cost Firewall `v0.8.0`.

---

## Evaluation Mode

These examples default to normal production behavior:

```text
aif_enforcement_mode enforce;
```

AI Cost Firewall v0.8.0 also supports a non-disruptive Evaluation Mode. To evaluate cache opportunities without serving cached responses to the application, change the configuration to:

```text
aif_enforcement_mode observe;
```

Then recreate only the firewall container so the active configuration is reloaded:

```bash
docker compose up -d --no-deps --force-recreate firewall
```

Verify the active mode:

```bash
curl -s http://localhost:8080/version
```

The version response should report `v0.8.0`, `aif_enforcement_mode: observe`, and an evaluation cache scope.

In `observe` mode:

- every eligible live request still reaches the configured upstream provider;
- exact and semantic cache decisions use isolated evaluation state;
- a would-have exact or semantic hit is recorded but does not replace the upstream response;
- shadow misses populate only evaluation cache state;
- cache-bypass requests perform no evaluation lookup or store;
- Redis, Qdrant, or embedding failures used only for evaluation do not interrupt live application traffic;
- production cache-hit and production savings counters are not incremented by shadow hits;
- controlled JSON/SSE cache identity remains transport-independent.

Useful evaluation metrics include:

```text
aif_evaluation_exact_hits_total
aif_evaluation_semantic_hits_total
aif_evaluation_misses_total
aif_evaluation_tokens_avoided_total
aif_evaluation_gross_saved_micro_usd_total
aif_evaluation_net_saved_micro_usd_total
aif_evaluation_errors_total
```

The AIF enforcement mode controls caching and cost-optimization behavior only. It does not independently switch VCAL Security Guard, Privacy Guard, or Usage Guard into observe mode.

---

## Streaming behavior

AI Cost Firewall v0.8.0 retains the controlled OpenAI-compatible streaming path introduced in v0.7.0. Ordinary requests continue to use the normal JSON completion path and do not require Ollama streaming support. When a client sends `"stream": true`, the configured Ollama OpenAI-compatible endpoint must provide compatible SSE; AIF consumes and assembles the complete response before returning approved SSE to the client.

`streaming_enabled true;` permits controlled streaming; it does not force ordinary requests to stream.

`max_stream_upstream_bytes` limits cumulative provider SSE bytes for a single controlled stream; it is not an instantaneous memory-buffer limit. `upstream_timeout_seconds` also bounds the maximum idle gap between provider SSE chunks, and AIF applies a 15-minute absolute ceiling to one provider-side controlled generation.

---

## Example Request

```bash
curl http://localhost:8080/v1/chat/completions \
  -H "Content-Type: application/json" \
  -d '{
    "model": "llama3.2:3b",
    "messages": [
      {"role": "user", "content": "Explain Redis briefly."}
    ]
  }'
```

Run the same request twice.

- The first request should go upstream to Ollama.
- The second identical request should be served from the exact cache.
- Similar follow-up prompts may hit the semantic cache after embeddings are generated and stored in Qdrant.

### Controlled streaming request

To test controlled streaming, add `"stream": true`:

```bash
curl -N http://localhost:8080/v1/chat/completions \
  -H "Content-Type: application/json" \
  -d '{
    "model": "llama3.2:3b",
    "messages": [
      {"role": "user", "content": "Explain Redis briefly."}
    ],
    "stream": true,
    "stream_options": {"include_usage": true}
  }'
```

The client receives `text/event-stream` only after AIF has assembled and approved the complete response. Exact/semantic cache reuse remains available for controlled streaming, including reuse across JSON and SSE delivery modes.

---

### Enforce vs observe expectations

With the supplied default `aif_enforcement_mode enforce;`, repeated identical requests can be served from the exact cache and sufficiently similar requests can be served from the semantic cache.

If you switch to `observe`, the same cache opportunities are evaluated using shadow state, but the live upstream provider is still called. Use the evaluation metrics above to confirm would-have hits rather than expecting production cache-hit counters to increase.

---

## Expected Behavior

- All model traffic stays local inside Docker.
- Cache misses go to Ollama.
- Embeddings are generated locally through Ollama using `nomic-embed-text`.
- Qdrant vector size is set to `768`, matching `nomic-embed-text`.
- Redis stores exact cache entries.
- Qdrant stores semantic cache vectors.

---

## Expected Metrics

After repeated and similar requests, check metrics:

```bash
curl -s http://localhost:8080/metrics | grep '^aif_'
```

Expected activity:

- `aif_requests_total` increases on each request.
- `aif_cache_exact_hits` increases after repeated identical prompts.
- `aif_cache_semantic_hits` may confirm semantic cache reuse.
- `aif_semantic_candidates_checked_total` increases during semantic lookups.
- `aif_semantic_threshold_results_total` shows semantic threshold pass/fail counts.
- `aif_semantic_lookup_duration_seconds` shows semantic lookup latency.
- After a controlled streaming request, `aif_stream_requests_total` and `aif_stream_completed_total` increase.
- On an upstream streaming miss, `aif_stream_upstream_chunks_total` and `aif_stream_upstream_bytes_total` show provider SSE intake.
- `aif_stream_upstream_errors_total` remains zero for successful provider streams.
- Cost metrics may remain zero or minimal unless local model pricing is configured.

In `enforce` mode, the production cache and savings counters describe actions actually applied to live traffic. In `observe` mode, use the `aif_evaluation_*` counters for would-have cache outcomes and estimated avoided usage; do not interpret production cache-hit/savings counters as evaluation results.

If the observability overlay is enabled, Grafana dashboards should begin populating after a few minutes of repeated traffic.

---

## Evidence events

The firewall emits structured `vcal.evidence.event` schema v1.1 records to
application logs. Every trace that emits `request.received` ends with exactly
one `request.completed` or `request.failed` event.

In Evaluation Mode, cache-related evidence also distinguishes the hypothetical
decision from the live action using attributes such as `enforcement_mode`,
`decision`, `would_action`, and `applied_action`.

Inspect evidence events with:

```bash
docker compose logs firewall | grep 'VCAL evidence event'
```
