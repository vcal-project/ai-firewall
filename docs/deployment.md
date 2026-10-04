# Production deployment notes

AI Cost Firewall is designed to run as a single stateless application process with external Redis/Qdrant and optional VCAL modules. v0.8.3 keeps the v0.8 Evaluation Mode model (`enforce` or non-disruptive `observe`), retains the v0.8.2 deployment hardening, and adds Assessment Context/runtime identity for reproducible Observe-mode assessment.

The AIF image is not OpenShift-specific. The normal `docker-compose.yml` deployment remains supported, while OpenShift manifests are isolated under `deploy/openshift/`.

## Security baseline

- Keep Redis, Qdrant, Guard and Audit endpoints on private networks.
- Use `guard_fail_open false` when Security Guard, Privacy Guard, or Usage Guard is an enforcement control.
- Protect `/metrics` or expose it only on a trusted monitoring network.
- Supply credentials through deployment secrets; never commit real secrets to the config file.
- Run the Firewall as non-root, read-only where possible, with Linux capabilities dropped and `no-new-privileges`. The v0.8.2 image uses a numeric non-root fallback user and an explicit `SIGTERM` stop signal; orchestrators such as OpenShift may assign their own runtime UID.

## Evaluation deployments

For a low-risk caching/cost pilot, configure:

```conf
aif_enforcement_mode observe;
```

In `observe`, live responses still come from the upstream provider. Redis and Qdrant are used as isolated evaluation state, and their temporary failure must not interrupt application traffic or make AIF unready solely because the evaluation cache is unavailable. Keep enough upstream capacity for the full live workload: observe-mode cache hits are hypothetical and do not reduce actual provider traffic.

Evaluation Mode controls AIF caching only. Existing Security Guard, Privacy Guard, and Usage Guard settings continue to enforce exactly as configured.

For assessment-capable deployments, keep Prometheus scraping AIF throughout the selected Observe period. v0.8.3 exposes the active runtime identity as `aif_runtime_info{version,config_schema,configuration_hash}` and exposes the current safe effective configuration at `GET /assessment-context`. Together these allow an assessment tool to verify that the selected period used one stable AIF runtime/configuration identity.

`/assessment-context` is sanitized and does not return API keys or credentials, but it still contains operational configuration metadata. Expose it only where that administrative/runtime metadata is intended to be visible.

## Graceful shutdown

The orchestrator must allow at least `graceful_shutdown_timeout_seconds` before sending SIGKILL. Docker Compose uses `stop_grace_period: 30s`; Kubernetes `terminationGracePeriodSeconds` should be configured to exceed the Firewall drain timeout.

During shutdown `/readyz` becomes unavailable before in-flight requests are drained. Audit delivery is then flushed on a best-effort basis.

## Startup and readiness policy

AIF exposes three probe endpoints in v0.8.2:

- `/healthz` reports process liveness.
- `/readyz` evaluates whether the instance should currently receive traffic and preserves the existing fail-open / Observe semantics.
- `/startupz` is a stricter orchestrator startup check. If an enabled Redis or Qdrant cache is also configured as required for readiness, `/startupz` requires that backend to have initialized successfully in the current process.

The stricter startup check prevents a pod that started before Redis/Qdrant from remaining on a startup-time no-op cache for its whole lifetime. Use `/startupz` as a Kubernetes/OpenShift `startupProbe`, `/readyz` as the `readinessProbe`, and `/healthz` as the `livenessProbe`.

Do not mark an optional fail-open cache as a required readiness dependency unless removing/restarting the pod is the intended policy.

## OpenShift deployment

v0.8.2 includes an OpenShift `restricted-v2` baseline under:

```text
deploy/openshift/
```

The example does not request `anyuid` or a custom SCC. It drops all Linux capabilities, disables privilege escalation, uses a read-only root filesystem, uses `RuntimeDefault` seccomp, and does not mount a service-account token. It deliberately leaves `runAsUser`/`runAsGroup` unset so OpenShift can assign a namespace-specific arbitrary UID.

Review `deploy/openshift/README.md` and replace the illustrative Service names, model IDs, credentials, and resource settings before applying the manifests.

## vLLM chat and embedding endpoints

A supported self-hosted pattern is to keep chat inference and embeddings on separate OpenAI-compatible endpoints, for example:

```text
Open WebUI -> AIF -> vLLM chat endpoint -> gpt-oss:20b
                  \
                   -> vLLM embedding endpoint -> Nomic model -> Qdrant
```

AIF v0.8.2 proxies `GET /v1/models` to the configured chat/inference upstream so OpenAI-compatible clients can discover chat models through AIF. The embedding endpoint is configured separately and is not exposed through that chat-side discovery route.

Before enabling semantic cache with a Nomic embedding service, verify the exact served embedding model ID and issue a test embedding request. Set `qdrant_vector_size` to the actual returned vector length rather than assuming a dimension from the model-family name.

## Backpressure

`max_inflight_requests` bounds total application requests and `max_inflight_upstream_requests` bounds simultaneous upstream LLM calls. Exceeding a limit produces a deterministic 503 rather than allowing unbounded work accumulation.

Controlled streaming holds an upstream concurrency slot while provider SSE is consumed and assembled. `max_stream_upstream_bytes` bounds cumulative provider SSE bytes accepted for each controlled request; it is a total-response-size limit, not an instantaneous memory-buffer limit. `upstream_timeout_seconds` also bounds idle time between provider SSE chunks, and a 15-minute absolute generation ceiling prevents indefinite drip-feed occupancy. Controlled streaming begins client delivery only after the complete response has passed response controls.
