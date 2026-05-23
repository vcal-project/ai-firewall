# OpenAI cloud provider

Use OpenAI for both chat completions and embeddings. This is the fastest cloud evaluation path.

## Files

- `docker-compose.yml` — runnable local deployment stack for this pattern.
- `docker-compose.observability.yml` — optional Prometheus + Grafana overlay.
- `ai-firewall.conf` — minimal AI Cost Firewall configuration for this pattern.
- `README.md` — setup, test request, expected behavior, and expected metrics.

---

## Start

Run commands from this directory:

```bash
cd deploy/examples/openai-cloud
```

Edit `ai-firewall.conf` and replace:

```text
sk-your-openai-key
```

Then start the deployment:

```bash
docker compose up -d
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

```bash
curl http://localhost:8080/healthz
curl http://localhost:8080/readyz
```

Expected:

```text
OK
READY
```

---

## Example Request

```bash
curl http://localhost:8080/v1/chat/completions \
  -H "Content-Type: application/json" \
  -d '{
    "model": "gpt-4o-mini-2024-07-18",
    "messages": [
      {"role": "user", "content": "Explain Redis briefly."}
    ]
  }'
```

Run the same request twice.

- The first request should go upstream to OpenAI.
- The second identical request should be served from the exact cache.
- Similar follow-up prompts may hit the semantic cache after embeddings are generated and stored in Qdrant.

---

## Expected Behavior

- Redis and Qdrant start locally.
- AI Cost Firewall forwards cache misses to OpenAI.
- OpenAI is used for both chat completions and embeddings.
- Exact cache should work on repeated identical prompts.
- Semantic cache should work on sufficiently similar prompts.
- Redis stores exact cache entries.
- Qdrant stores semantic cache vectors.

---

## Expected Metrics

After repeated and similar requests, check metrics:

```bash
curl -s http://localhost:8080/metrics | grep '^aif_'
```

Expected activity:

- `aif_requests_total` increases for `/v1/chat/completions`.
- `aif_cache_exact_hits` increases after repeated identical prompts.
- `aif_cache_semantic_hits` may increase after similar prompts.
- `aif_semantic_candidates_checked_total` confirms semantic lookup activity.
- `aif_semantic_threshold_results_total` shows semantic threshold pass/fail counts.
- `aif_semantic_lookup_duration_seconds` shows semantic lookup latency.
- Cost and savings metrics should reflect configured chat and embedding model prices.

If the observability overlay is enabled, Grafana dashboards should begin populating after a few minutes of repeated traffic.