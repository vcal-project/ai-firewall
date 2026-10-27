# AI Cost Firewall — Operations

This is the runtime operations guide for AIF deployments. See [Quick Start](quickstart.md) for first
installation, [Configuration Reference](config-reference.md) for all directives,
[Troubleshooting](troubleshooting.md) for symptom-based diagnostics, and [Deployment](deployment.md) for
production/OpenShift manifests.

## Operating model

AIF routes OpenAI-compatible chat requests through optional guards and exact/semantic caches to a configured
upstream. Supported mode settings are:

```conf
aif_enforcement_mode enforce;
# or: aif_enforcement_mode observe;
```

Under `enforce`, eligible cache hits may avoid upstream traffic. Under `observe`, all live requests still
reach the upstream while separate Redis/Qdrant shadow state measures potential reuse. **Observe is not a
Guard observe mode.** See [Evaluation Mode](evaluation-mode.md).

## Startup and dependency validation

On startup, AIF checks configuration validity, cache/embedding completeness, and applicable dependency
initialization. Semantic caching requires an embedding model with a vector dimension matching the Qdrant
collection. A mismatch is a startup configuration problem, not a cache miss.

Relevant settings include:

```conf
exact_cache_enabled true;
exact_cache_fail_open true;
semantic_cache_enabled true;
semantic_cache_fail_open true;
readiness_requires_redis true;
readiness_requires_qdrant false;
readiness_requires_upstream false;
```

A cache marked required for startup/readiness has stricter initialization semantics than an optional
fail-open cache. In Observe Mode, failures in Redis/Qdrant/embeddings used **only for evaluation** do not
interrupt live inference and are recorded in evaluation error telemetry. Static invalid configuration can
still prevent startup.

### Probe endpoints

| Endpoint | Purpose | Typical orchestration use |
|---|---|---|
| `GET /healthz` | Process liveness, independent of downstream services | `livenessProbe` |
| `GET /startupz` | Process completed required cache initialization | `startupProbe` |
| `GET /readyz` | Current ability to serve traffic under configured dependency policy | `readinessProbe` |

```bash
curl -i http://localhost:8080/healthz
curl -i http://localhost:8080/startupz
curl -i http://localhost:8080/readyz
```

Normal responses are 200 (`OK`, `started`, `ready` respectively). During graceful shutdown `/startupz` and
`/readyz` return 503. `/startupz` is intentionally strict when enabled Redis/Qdrant backends were declared
required for readiness and failed to initialize in the current process. `/readyz` remains healthy when an
**optional evaluation-only** backend is down in Observe Mode. A required upstream can cause `/readyz` to
fail without independently blocking `/startupz`.

Do not treat a healthy probe as a complete chat/embedding connectivity test: verify a real model request
separately.

## Configuration validation and reload

### Validate and inspect before deployment

```bash
docker compose run --rm firewall \
  --config /configs/ai-firewall.conf --test-config

docker compose run --rm firewall \
  --config /configs/ai-firewall.conf --print-config
```

Expected static validation output: `configuration OK`. `--test-config` checks syntax and required values
**without contacting Redis, Qdrant, or upstream services**; `--print-config` masks secrets. You can also run
the binary with `--config configs/ai-firewall.conf --test-config`.

### SIGHUP reload

```bash
docker compose kill -s HUP firewall
# binary/systemd deployment: send SIGHUP to the AIF process
```

Reload validates the replacement configuration and rebuilds eligible runtime dependencies before swapping
the active runtime. A failed reload leaves the prior valid runtime active. Reload is not automatic file
watching.

Changes to `listen_addr`, `max_request_body_bytes`, `max_inflight_requests`,
`graceful_shutdown_timeout_seconds`, `upstream_ca_bundle`, and `embedding_ca_bundle`
require a **process restart** and are rejected if attempted through SIGHUP. The CA
trust store is loaded into provider HTTP clients at startup; rotating PEM file
**contents** without changing the configured path also requires restarting AIF. A
successful reload that changes effective safe configuration identity updates `aif_runtime_info`; see
[Assessment Context](assessment-context.md).

Container environments may also require a recreate, rather than just a restart, when changing environment
variables, mounted Secrets, or Compose settings. Plan this according to your deployment method.

## Failure policies and request protection

| Failure or condition | Enforce path | Observe path |
|---|---|---|
| Redis exact lookup/store failure | Skip or fail according to `exact_cache_fail_open` | Evaluation error; continue live upstream |
| Qdrant/embedding semantic lookup failure | Skip or fail according to `semantic_cache_fail_open` | Evaluation error; continue live upstream |
| Cache-bypass header | Forward upstream, no cache lookup/store | Forward upstream, no shadow lookup/store |
| Guard transport/contract failure | `guard_fail_open` policy determines operational failure handling | AIF Observe does **not** override Guard failure policy |
| Recognized unsupported Privacy text structure | Privacy inspection contract handling; do not assume fail-open passthrough | Same applicable Privacy safety contract |

Guard evaluation order is Security (request), Privacy (request), Usage (request), response Security, and
Privacy restore. AIF caches pre-restoration responses and uses mappings from the current request. For
guard-specific structured-content coverage and cache scoping see [Structured
Content](structured-content.md).

### Limits and upstream timeouts

Use `max_request_body_bytes` for total body size and `max_prompt_chars` for parsed prompt characters.
`request_timeout_seconds`, `upstream_timeout_seconds`, and `embedding_timeout_seconds` govern distinct
operations; use the [Configuration Reference](config-reference.md) for defaults and fallback rules. Keep
client, load balancer, and upstream timeouts mutually consistent.

Controlled `stream=true` requests buffer/assemble the provider response until response controls approve it.
`max_stream_upstream_bytes` is a **cumulative** upstream SSE limit; `upstream_timeout_seconds` additionally
bounds the idle gap between provider chunks, with a 15-minute absolute generation ceiling described in
[Controlled Streaming](controlled-streaming.md). This trades token-level immediacy for pre-delivery policy
controls.

## Graceful shutdown and concurrency

On `SIGTERM`/`SIGINT`, AIF disables readiness, rejects new requests with a service-unavailable response,
gives in-flight requests time to drain, then exits. Configure `graceful_shutdown_timeout_seconds` for
expected request duration, especially if controlled streaming can take longer.

Monitor `aif_inflight_requests`, `aif_shutdown_in_progress`, and `aif_shutdown_rejections_total`. The
container image includes a stop signal and supports numeric non-root execution; the [OpenShift
baseline](../deploy/openshift/) is designed for restricted execution.

## Cache lifecycle and verification

Semantic entries have `inserted_at` and `expires_at`. Expiry is checked at lookup independently of eventual
physical deletion. `semantic_cache_retention_seconds` controls the eligible lifetime; clean up expired
collection entries operationally as described by the configured deployment tools.

When diagnosing cache behavior, first check whether requests are eligible (especially structured content,
tools, and bypass), then exact/semantic store controls, model and scope identity, embedding dimension,
threshold, expiry, and backend errors. Do not clear a production collection to solve a dimension mismatch
without assessing the data impact. See [How It Works](how-it-works.md) and
[Troubleshooting](troubleshooting.md).

## Metrics, logs, and evidence

```bash
curl -sS http://localhost:8080/metrics
docker compose logs --tail=200 firewall
```

Metrics may require authentication when `metrics_auth_required` is configured. The built-in Grafana
dashboards cover request rate, cache outcomes, gross and net cost estimates, semantic diagnostics, pressure,
and high-level guard outcomes. Evaluation savings are **not** real savings and are exported separately. See
[Metrics and Costs](metrics-and-costs.md).

To enable structured evidence logs, the repository documents:

```text
RUST_LOG=info,vcal_evidence=info
```

Each request trace has one terminal lifecycle event. Optional VCAL Audit delivery uses an in-memory
queue/batches/retries and is **best effort**: client requests typically succeed even when Audit delivery
fails. Event loss is possible after queue saturation, retry exhaustion, or termination before flush. See
[Evidence and Audit](audit-integration.md).

### Assessment Context

```bash
curl -sS http://localhost:8080/assessment-context
```

The endpoint returns an allow-listed, sanitized effective configuration and deterministic identity.
`aif_runtime_info` provides historical version/config identity; retain Prometheus samples for the assessment
window. AIF does not persist external assessment reports. See [Assessment Context](assessment-context.md).

## Operator checklist

1. Confirm running image/tag and the expected `aif_enforcement_mode` with `/version`.
2. Validate startup/readiness probes and make a known good live chat request.
3. Verify Prometheus scrape and Grafana data; distinguish production from evaluation metrics.
4. Check Redis/Qdrant readiness policy, Qdrant dimension, embedding path, and cache eligibility.
5. Confirm ingress authentication boundary and keep credentials out of Git and logs.
6. If optional guards are enabled, test a known allow and known block case under the configured failure policy.
7. If Audit is enabled, test receipt of evidence and alert on dropped/retried batches.
8. On upgrade, compare configuration and follow [Upgrading](upgrading.md); use [Troubleshooting](troubleshooting.md) for failures.
