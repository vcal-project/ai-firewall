# OpenRouter upstream with OpenAI embeddings

Use OpenRouter as the OpenAI-compatible chat upstream and OpenAI as the embedding provider.

## Files

- `docker-compose.yml` — runnable local deployment stack for this pattern.
- `docker-compose.observability.yml` — optional Prometheus + Grafana overlay.
- `ai-firewall.conf` — minimal AI Cost Firewall configuration for this pattern.
- `README.md` — setup, test request, expected behavior, and expected metrics.

---

## Start

Run commands from this directory:

```bash
cd deploy/examples/openrouter
```

Edit `ai-firewall.conf` and replace:

```text
your-openrouter-key
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

Validate that the firewall is listening:

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
    "model": "openai/gpt-4o-mini",
    "messages": [
      {"role": "user", "content": "Explain Redis briefly."}
    ]
  }'
```

Run the same request twice.

- The first request should go upstream to OpenRouter.
- The second identical request should be served from the exact cache.
- Similar follow-up prompts may hit the semantic cache after embeddings are generated and stored in Qdrant.

---

## Expected Behavior

- Chat cache misses go to OpenRouter.
- Embeddings go to OpenAI using `text-embedding-3-small`.
- Qdrant vector size is `1536`.
- Unknown models are passed through because OpenRouter model names vary by provider.
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
- `aif_cache_semantic_hits` may increase after sufficiently similar prompts.
- `aif_semantic_candidates_checked_total` confirms semantic lookup attempts.
- `aif_semantic_threshold_results_total` shows semantic threshold pass/fail counts.
- `aif_semantic_lookup_duration_seconds` shows semantic lookup latency.
- Provider-specific cost metrics may require adding `model_price` entries for the selected OpenRouter model.

If the observability overlay is enabled, Grafana dashboards should begin populating after a few minutes of repeated traffic.