# Upgrade and Rollback Guide

## v0.8.5 to v0.8.6

v0.8.6 is described in the AIF release preparation as a hardening/compatibility update: Usage Guard v0.2.0 structured-content compatibility and enforcement-safety work, trusted-context foundation, input validation, and a `rustls` dependency security update. **Check the published tag and release notes** for exact migration and compatibility requirements; structured-content coverage must be validated in the deployment.

1. Back up the active AIF configuration, container manifests, and monitoring configuration; record the prior image digest for rollback.
2. Verify the actual AIF image version and the deployed VCAL Usage Guard version. Do not assume that preserving an array means every nested text-bearing form is inspected.
3. Test ordinary and structured-content allow, warn/block, and operational failure paths under your effective Usage Guard policy and `guard_fail_open` setting before directing production traffic.
4. Recheck Privacy and Security Guard interoperability, exact/semantic cache eligibility, and Observe/Enforce parity for your workload.
5. Verify `/healthz`, `/startupz`, `/readyz`, `/version`, and a known live chat request. Inspect evidence and metrics for Guard decisions and unexpected bypasses.
6. Regenerate the dependency vulnerability report against the **actual** published image and lockfile rather than assuming the dependency advisory is mitigated by a tag name.

Check the final [GitHub release](https://github.com/vcal-project/ai-firewall/releases) for changes to exact directive names, module-version requirements, and any migration notes.

## v0.8.4 to v0.8.5

The documented v0.8.5 maintenance scope is Security Guard structured-content compatibility, including preservation of JSON message content during scans and correction of a structured-text request inspection bypass. The compatibility target described for this release is VCAL Security Guard v0.3.0.

1. Back up deployment settings and record currently configured Guard policies.
2. Verify AIF and Security Guard version compatibility for your published images.
3. Test a known malicious structured-text request and ensure it is blocked **before** upstream delivery; also test an allowed structured request to ensure its JSON shape is preserved.
4. Confirm normal string, controlled streaming, Privacy Guard, and Usage Guard paths continue to work as configured.
5. Check the [v0.8.5 release notes](https://github.com/vcal-project/ai-firewall/releases) for final configuration/API changes; do not infer universal nested-content inspection coverage across all Guard modules.

## v0.8.3 to v0.8.4

This update integrates structured-content inspection with VCAL Privacy Guard and strengthens tenant/policy isolation of exact and semantic cache entries. Evaluation Mode, controlled streaming, and Assessment Context remain available.

1. Back up the configuration and deployment manifests; deploy `vcalproject/ai-firewall:v0.8.4` alongside a compatible VCAL Privacy Guard v0.3.0 deployment when Privacy Guard is enabled.
2. Verify `/healthz`, `/startupz`, `/readyz`, `/version`, and `/assessment-context`.
3. Exercise supported string and text-content array requests through Privacy Guard. Recognized unsupported text-bearing structures are rejected instead of silently bypassing privacy inspection. Referenced file attachments remain outside inspection scope.
4. Check that the effective privacy policy ID, version, and hash are available. Without a complete policy identity, or when Privacy Guard fails open, AIF bypasses cache lookup and storage for the request.
5. Verify cross-tenant and cross-policy exact/semantic cache isolation. Previously cached privacy-protected entries may become cold because the cache identity has changed; unguarded cache behavior remains compatible where safe.
6. Validate current-request placeholder restoration and ensure stored cache values contain placeholders rather than restored personal data. Recheck JSON and controlled-SSE behavior in both `observe` and `enforce` modes.

Structured-content arrays remain ineligible for semantic reuse; eligible requests can still use exact cache. No Privacy Guard source modification or attachment processing is part of this update.

## v0.8.2 to v0.8.3

v0.8.3 adds Assessment Context and deterministic runtime/configuration identity. It does not change the default enforcement mode, Observe/Enforce request-path semantics, cache data formats, or evidence schema, and it requires no Redis/Qdrant data migration.

1. Back up the current configuration and deployment manifests.
2. Deploy `vcalproject/ai-firewall:v0.8.3`.
3. Verify `/healthz`, `/startupz`, `/readyz`, `/version`, and `GET /v1/models` as before.
4. Verify the new endpoint:

   ```bash
   curl -s http://localhost:8080/assessment-context | jq
   ```

5. Confirm `/version` reports release `0.8.3`, release title `Assessment Context`, and Assessment Context schema `1.0`.
6. Confirm Prometheus is scraping the new runtime identity metric:

   ```text
   aif_runtime_info{version="0.8.3",config_schema="1",configuration_hash="sha256:..."} 1
   ```

7. If you use Observe-mode assessment/reporting, keep Prometheus history for the full selected assessment period so configuration stability can be verified from `aif_runtime_info`.
8. Review automation that sends `SIGHUP`: v0.8.3 rejects reloads that attempt to change `listen_addr`, `max_request_body_bytes`, `max_inflight_requests`, or `graceful_shutdown_timeout_seconds`. Restart/recreate the process for those fields.
9. If VCAL Console v0.3 Assessment is used, verify that its current AIF context matches the single historical runtime identity for the selected period before finalization.

No new configuration directive is mandatory solely for upgrading from v0.8.2. The Assessment Context endpoint is additive and is derived from the existing effective configuration. Secrets remain excluded from the endpoint and from `aif_runtime_info` labels.

The evidence schema remains `1.1`.

## v0.8.1 to v0.8.2

v0.8.2 is a deployment-hardening and OpenAI-compatibility release. It does not change the default AIF enforcement mode and requires no Redis/Qdrant data migration.

1. Back up the current configuration and deployment manifests.
2. Deploy `vcalproject/ai-firewall:v0.8.2`; the image remains a generic OCI image suitable for Docker/Compose as well as Kubernetes/OpenShift.
3. Verify `/healthz`, `/startupz`, `/readyz`, `/version`, and `GET /v1/models`.
4. Review exact-cache behavior for OpenAI-compatible extension fields. v0.8.2 includes flattened request/message fields (for example tool schemas, tool-call IDs, response-format/provider extensions) in exact-cache identity. Existing v0.8.1 exact-cache entries are not migrated; for deterministic comparison, use a fresh cache namespace or clear test cache state.
5. Requests with non-string message content now bypass semantic lookup/store. They can still pass through to the upstream and use exact cache when otherwise eligible.
6. **Historical v0.8.2 behavior:** the guard integrations at that time did not inspect nested content-part arrays. Later releases introduced module-specific improvements; see the v0.8.4–v0.8.6 upgrade notes and [Structured Content](structured-content.md).
7. For Kubernetes/OpenShift, use `/startupz` as the startup probe. If an enabled Redis/Qdrant cache is marked required for readiness, a process that did not initialize that backend will fail `/startupz` so the orchestrator can restart it.
8. OpenShift users can start from `deploy/openshift/`; these assets are optional and do not replace the Docker Compose deployment.
9. If chat is served by vLLM, `GET /v1/models` can now be requested through AIF. If embeddings are served by a separate vLLM/Nomic endpoint, discover and test that endpoint directly, then set `qdrant_vector_size` to the actual returned vector length before enabling semantic cache.

No configuration directive is mandatory solely for upgrading from v0.8.1. Existing deployments that do not use the new OpenShift assets or model-discovery route can retain their current deployment style.

## v0.7.0 to v0.8.0

v0.8.0 introduces AI Cost Firewall Evaluation Mode. The default remains `enforce`, so existing v0.7.0 deployments retain normal production cache behavior unless `aif_enforcement_mode observe;` is explicitly configured.

1. Back up the current configuration, deployment manifests, and Grafana/Prometheus provisioning files.
2. Add or review the new directive:

   ```conf
   aif_enforcement_mode enforce;
   ```

3. For a non-disruptive pilot, change it to:

   ```conf
   aif_enforcement_mode observe;
   ```

4. In `observe`, provision Redis and Qdrant for evaluation state, but do not expect cache hits to reduce real upstream traffic. Every eligible live request still reaches the upstream provider.
5. Confirm `/version` reports the intended `aif_enforcement_mode` and `effective_cache_scope`. In observe mode, confirm the effective semantic collection is the isolated evaluation collection.
6. Update monitoring to include `aif_enforcement_mode_info` and the `aif_evaluation_*` metrics. Do not combine these hypothetical savings with normal production `aif_cache_*` or savings counters.
7. Verify cache bypass: a bypassed observe request must perform no shadow exact/semantic lookup and no shadow store.
8. Verify JSON-to-SSE and SSE-to-JSON requests share the same shadow exact-cache identity while still calling the live upstream provider.
9. Test Redis and Qdrant loss in `observe`; requests and `/readyz` should remain healthy, evaluation errors should increment, and evaluation should recover automatically after the dependency returns.
10. If VCAL Audit is enabled, verify evidence carries `enforcement_mode`, `decision`, `would_action`, and `applied_action` so hypothetical decisions remain distinguishable from actions applied to live traffic.

Evaluation cache state is intentionally isolated from production state and is not promoted automatically when switching from `observe` to `enforce`. No Redis, Qdrant, or Audit data migration is required solely for this release.

`aif_enforcement_mode` controls AIF caching/cost optimization only. It does not add `observe` modes to Security Guard, Privacy Guard, or Usage Guard; those modules continue to follow their existing configuration.
