# Benchmarks

This page summarizes benchmark results for AI Cost Firewall v0.2.0.

The benchmark profile uses a local simulated OpenAI-compatible upstream provider. This isolates AI Cost Firewall, Redis, Qdrant, cache behavior, and Prometheus metrics without external API cost, provider rate-limit noise, or variable upstream latency.

These results are not universal capacity limits. Real-world performance depends on hardware, deployment configuration, Redis/Qdrant latency, upstream provider latency, request size, cache hit ratio, semantic-cache settings, and model pricing.

## Methodology

The benchmarks were executed against the AI Cost Firewall HTTP API using a controlled request mix. The benchmark client sends OpenAI-compatible `/v1/chat/completions` requests to AI Cost Firewall, while AI Cost Firewall uses Redis for exact cache, Qdrant for semantic cache, and a local simulated OpenAI-compatible provider for chat completions and embeddings.

The simulated upstream provider is used to isolate AI Cost Firewall behavior from external API cost, provider rate limits, internet latency, and third-party service variability.

The benchmark measures:

- HTTP success and failure rate
- sustained requests per second
- p50, p95, and p99 latency
- exact cache hits
- semantic cache hits
- cache misses
- avoided upstream chat calls
- embedding overhead
- simulated gross and net savings
- embedding timeouts and shutdown rejections

## Test environment

| Component | Description |
|---|---|
| AI Cost Firewall | v0.2.0 |
| Deployment | Docker Compose |
| Benchmark client | k6 |
| Upstream provider | Local simulated OpenAI-compatible provider |
| Exact cache | Redis |
| Semantic cache | Qdrant |
| Metrics | Prometheus-format `/metrics` endpoint |
| API endpoint tested | `/v1/chat/completions` |
| Model name used in requests | `gpt-4o-mini-2024-07-18` |

## Traffic profile

The cache-effectiveness benchmark uses a repeated enterprise-style traffic profile:

| Request category | Target share |
|---|---:|
| Exact cache candidates | 65% |
| Semantic cache candidates | 20% |
| Cache miss candidates | 15% |

The exact final cache-hit ratio may differ from the target request mix because the cache warms during the benchmark. Over longer runs, repeated semantic and miss candidates may become exact or semantic hits.

## Tools

| Tool | Purpose |
|---|---|
| k6 | Generates controlled HTTP load and reports latency, throughput, and failure rate |
| Redis | Stores exact cache entries |
| Qdrant | Stores and searches semantic cache vectors |
| Local mock OpenAI-compatible provider | Simulates chat completions and embeddings without external API calls |
| Prometheus metrics endpoint | Provides AI Cost Firewall counters for cache hits, misses, model calls, costs, and errors |

## Limitations

These benchmarks are designed to validate AI Cost Firewall behavior under controlled local conditions. They should not be interpreted as universal production capacity limits.

The benchmark results depend on:

- host CPU and memory
- Docker networking
- Redis and Qdrant performance
- request body size
- cache hit ratio
- semantic similarity threshold
- embedding dimensions
- upstream provider latency
- deployment topology
- whether the load generator runs on the same VM

The high-load benchmark was executed on a single VM. Higher RPS values caused instability in that single-VM environment, so the 500 RPS result should be interpreted as the highest successful single-VM local simulated-upstream run so far, not as a maximum AI Cost Firewall capacity claim.

## Cache-effectiveness benchmark

| Metric | Result |
|---|---:|
| Benchmark mode | Local simulated OpenAI-compatible upstream |
| Target throughput | 30 RPS |
| Duration | 30 minutes |
| Completed requests | 54,001 |
| HTTP failures | 0 |
| Error rate | 0.00% |
| p50 latency | 1.27 ms |
| p95 latency | 9.03 ms |
| p99 latency | 12.58 ms |
| Exact cache hits | 46,026 |
| Semantic cache hits | 7,359 |
| Cache misses | 616 |
| Aggregate cache-hit rate | 98.86% |
| Avoided upstream chat calls | 53,385 |
| Embedding timeouts | 0 |
| Simulated net cost reduction | 98.59% |

## High-load single-VM benchmark

| Metric | Result |
|---|---:|
| Benchmark mode | Single-VM simulated upstream |
| Target throughput | 500 RPS |
| Actual throughput | 499.31 RPS |
| Duration | 5 minutes |
| Completed requests | 149,800 |
| HTTP failures | 0 |
| Error rate | 0.00% |
| Success rate | 100.00% |
| Dropped k6 iterations | 201 |
| p50 latency | 1.72 ms |
| p95 latency | 26.45 ms |
| p99 latency | 58.87 ms |
| Max latency | 4.39 s |
| Exact cache hits | 127,184 |
| Semantic cache hits | 22,202 |
| Cache misses | 414 |
| Aggregate cache-hit rate | 99.72% |
| Avoided upstream chat calls | 149,386 |
| Embedding timeouts | 0 |
| Simulated net cost reduction | 99.44% |
