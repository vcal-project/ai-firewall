# Local full stack with dashboards

Run a fully local AI Cost Firewall evaluation environment with Ollama, Redis, Qdrant, Prometheus, and Grafana.

## Files

- `docker-compose.yml` — runnable local stack including observability services.
- `ai-firewall.conf` — minimal AI Cost Firewall configuration for this pattern.
- `README.md` — setup, test request, expected behavior, and expected metrics.

---

## Start

Run commands from this directory:

```bash
cd deploy/examples/local-full-stack
```

Start the stack:

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

## Observability Stack

Prometheus:

```text
http://localhost:9090
```

Grafana:

```text
http://localhost:3000
```

The stack automatically provisions dashboards from:

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

The version endpoint should report AI Cost Firewall `v0.7.0`.

---

## Streaming behavior

AI Cost Firewall v0.7.0 supports controlled OpenAI-compatible streaming. Ordinary requests continue to use the normal JSON completion path and do not require Ollama streaming support. When a client sends `"stream": true`, the configured Ollama OpenAI-compatible endpoint must provide compatible SSE; AIF consumes and assembles the complete response before returning approved SSE to the client.

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

## Expected Behavior

- Fully local evaluation stack.
- Cache misses go to local Ollama.
- Semantic embeddings are generated locally through Ollama.
- Prometheus scrapes AI Cost Firewall metrics.
- Grafana automatically loads the Overview and Diagnostics dashboards.
- Semantic cache lookups are performed through Qdrant.
- Redis stores exact cache responses.

---

## Expected Metrics

After repeated and similar requests, check metrics:

```bash
curl -s http://localhost:8080/metrics | grep '^aif_'
```

Expected activity:

- `aif_requests_total` confirms traffic.
- `aif_cache_exact_hits` confirms exact cache reuse.
- `aif_cache_semantic_hits` may confirm semantic reuse.
- `aif_semantic_candidates_checked_total` confirms semantic lookup activity.
- `aif_semantic_threshold_results_total` shows semantic threshold pass/fail counts.
- `aif_semantic_lookup_duration_seconds` shows semantic lookup latency.
- After a controlled streaming request, `aif_stream_requests_total` and `aif_stream_completed_total` increase.
- On an upstream streaming miss, `aif_stream_upstream_chunks_total` and `aif_stream_upstream_bytes_total` show provider SSE intake.
- `aif_stream_upstream_errors_total` remains zero for successful provider streams.
- `aif_semantic_expired_entries_skipped_total` remains near zero in a fresh deployment.

The Grafana dashboards should begin populating after a few minutes of demo or repeated traffic.

---

## Evidence events

The firewall emits structured `vcal.evidence.event` schema v1.1 records to
application logs. Every trace that emits `request.received` ends with exactly
one `request.completed` or `request.failed` event.

Inspect evidence events with:

```bash
docker compose logs firewall | grep 'VCAL evidence event'
```
