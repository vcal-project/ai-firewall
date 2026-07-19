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

The version endpoint should report AI Cost Firewall `v0.4.0`.

---

## Streaming behavior

AI Cost Firewall v0.4.0 supports non-streaming chat completions only. Requests
with `"stream": true` are rejected with HTTP `422` before cache, guard, or
upstream processing.

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
- Cost metrics may remain zero or minimal unless local model pricing is configured.

If the observability overlay is enabled, Grafana dashboards should begin populating after a few minutes of repeated traffic.

---

## Evidence events

The firewall emits structured `vcal.evidence.event` schema v1.1 records to
application logs. Every trace that emits `request.received` ends with exactly
one `request.completed` or `request.failed` event.

Inspect evidence events with:

```bash
docker compose logs firewall | grep 'VCAL evidence event'
```
