# Production deployment notes

AI Cost Firewall v0.5.0 is designed to run as a single stateless application process with external Redis/Qdrant and optional VCAL modules.

## Security baseline

- Keep Redis, Qdrant, Guard and Audit endpoints on private networks.
- Use `guard_fail_open false` when Security Guard or Privacy Guard is an enforcement control.
- Protect `/metrics` or expose it only on a trusted monitoring network.
- Supply credentials through deployment secrets; never commit real secrets to the config file.
- Run the Firewall as non-root, read-only where possible, with Linux capabilities dropped and `no-new-privileges`.

## Graceful shutdown

The orchestrator must allow at least `graceful_shutdown_timeout_seconds` before sending SIGKILL. Docker Compose uses `stop_grace_period: 30s`; Kubernetes `terminationGracePeriodSeconds` should be configured to exceed the Firewall drain timeout.

During shutdown `/readyz` becomes unavailable before in-flight requests are drained. Audit delivery is then flushed on a best-effort basis.

## Readiness policy

`/healthz` reports process liveness. `/readyz` additionally evaluates configured required dependencies and current runtime dependency observations. Do not mark an optional fail-open cache as a required readiness dependency unless removing the pod is the intended policy.

## Backpressure

`max_inflight_requests` bounds total application requests and `max_inflight_upstream_requests` bounds simultaneous upstream LLM calls. Exceeding a limit produces a deterministic 503 rather than allowing unbounded work accumulation.
