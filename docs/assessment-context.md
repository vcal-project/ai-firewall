# Assessment Context and Configuration Identity

AIF exposes sanitized runtime/configuration context so a bounded Observe-mode period can be interpreted as a reproducible assessment without altering request-path behavior.

Capabilities include:

- `GET /assessment-context`
- Assessment Context schema `1.0`
- deterministic SHA-256 configuration identity
- historical runtime identity through the `aif_runtime_info` Prometheus metric
- `/version` advertisement of the supported Assessment Context schema
- safe effective configuration context for cache, embedding, request-path, pricing, and mode interpretation
- stricter SIGHUP handling for restart-only settings

The evidence schema remains `1.1`, and Observe/Enforce cache semantics are unchanged.

## Assessment Context endpoint

The endpoint:

```bash
curl -s http://localhost:8080/assessment-context | jq
```

returns an allow-listed effective configuration snapshot intended for assessment and reporting tools.

It includes context such as:

- product version and schema versions
- enforcement mode and effective cache scope
- exact-cache state, TTL, timeout, and effective prefix
- semantic-cache state, threshold, retention, and effective collection
- embedding provider/model/vector size
- upstream provider and timeout behavior
- request-path limits relevant to interpretation
- configured pricing assumptions
- optional-module state
- `configuration_hash`

The response does **not** expose provider API keys, credentials, tenant identifiers, private license material, or other secret configuration values.

The configuration hash is deterministic for the safe effective configuration represented by the Assessment Context. Pricing entries are normalized before hashing so ordering alone does not change the identity. Secret-only changes do not change the public configuration hash.

## Historical runtime identity

AIF exports the currently active runtime identity as:

```text
aif_runtime_info{version="<running-version>",config_schema="1",configuration_hash="sha256:..."} 1
```

Assessment/reporting systems can query this metric over an exact historical window to verify that one AIF version/configuration identity remained active throughout the period.

When a supported runtime reload changes the effective configuration, AIF replaces the current `aif_runtime_info` label set. Prometheus can therefore retain the previous series historically while the current process exposes only the active identity.

## Reload safety

AIF supports nginx-style SIGHUP configuration reloads but rejects reloads that attempt to change settings requiring a process restart.

Restart-only settings include:

```text
listen_addr
max_request_body_bytes
max_inflight_requests
graceful_shutdown_timeout_seconds
```

This prevents the process from reporting a new configuration identity for settings that the running process could not actually apply.

See also [Operational behavior](operation.md), [Evaluation Mode](evaluation-mode.md), and [Metrics and dashboards](metrics-and-costs.md).
