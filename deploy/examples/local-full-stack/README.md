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
