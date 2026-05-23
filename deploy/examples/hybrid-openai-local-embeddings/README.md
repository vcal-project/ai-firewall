# OpenAI upstream with local embeddings

Use OpenAI for chat completions while generating embeddings locally through Ollama.

## Files

- `docker-compose.yml` — runnable deployment stack for this pattern.
- `docker-compose.observability.yml` — optional Prometheus + Grafana overlay.
- `ai-firewall.conf` — minimal AI Cost Firewall configuration for this pattern.
- `README.md` — setup, test request, expected behavior, and expected metrics.

---

## Start

Run commands from this directory:

```bash
cd deploy/examples/hybrid-openai-local-embeddings
```

Edit `ai-firewall.conf` and replace:

```text
sk-your-openai-key
```

Then start the deployment:

```bash
docker compose up -d
```

Pull the local embedding model:

```bash
docker compose exec ollama ollama pull nomic-embed-text
```

Restart the firewall after the embedding model becomes available:

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

- The first request should go upstream.
- The second identical request should be served from the exact cache.
- Similar follow-up prompts may hit the semantic cache after embeddings are generated and stored in Qdrant.

---

## Expected Behavior

- Cache misses go to OpenAI for chat completions.
- Semantic cache embeddings are generated locally by Ollama.
- This avoids cloud embedding calls while still using a cloud chat model.
- Qdrant vector size is `768` for `nomic-embed-text`.

---

## Expected Metrics

After repeated and similar requests, check metrics:

```bash
curl -s http://localhost:8080/metrics | grep '^aif_'
```

Expected activity:

- `aif_requests_total` increases on each request.
- `aif_cache_exact_hits` increases after repeated identical prompts.
- `aif_semantic_candidates_checked_total` confirms semantic lookup activity.
- `aif_semantic_threshold_results_total` shows semantic threshold pass/fail counts.
- `aif_semantic_lookup_duration_seconds` shows semantic lookup latency.
- Net savings should reflect chat savings while local embeddings may contribute little or no embedding overhead cost depending on configuration.