# Upgrading AI Cost Firewall

## v0.6.x to v0.7.0

v0.7.0 introduces controlled OpenAI-compatible streaming. `stream=true` no longer has to be rejected: AI Cost Firewall can consume provider SSE internally, reconstruct the complete response, run response controls and Privacy restoration, and only then replay approved SSE to the client.

1. Back up the current configuration, deployment manifests, and Grafana provisioning files.
2. Add or review the controlled-streaming directives:

   ```conf
   streaming_enabled true;
   max_stream_upstream_bytes 8M;
   upstream_timeout_seconds 120;
   ```

3. If the deployment must preserve the previous behavior of rejecting `stream=true`, explicitly set `streaming_enabled false`.
4. Review stream size, timeout, and concurrency sizing. `max_stream_upstream_bytes` is a cumulative provider-SSE response limit. Controlled streaming holds the upstream concurrency permit through provider generation/assembly; `upstream_timeout_seconds` limits idle gaps between chunks and a 15-minute absolute generation ceiling prevents indefinite drip-feed occupancy.
5. Update dashboards and alerts to the controlled-streaming metrics, especially `aif_stream_upstream_*`, `aif_stream_generation_duration_seconds`, and `aif_stream_client_time_to_first_byte_seconds`.
6. Validate both `stream=false` and `stream=true` requests, including `stream_options.include_usage` if clients use it.
7. If Security Guard and Privacy Guard are enabled, verify that response Security scanning and Privacy restoration succeed for controlled streaming.
8. Validate JSON-to-SSE and SSE-to-JSON cache reuse for an eligible request.
9. Test a deterministic mid-stream provider failure and verify that no partial model content is exposed to the client and the request returns an HTTP error before SSE commit.
10. Test a provider that returns headers and then stalls; verify the idle timeout fires, no partial content is committed, and the upstream permit is released. The absolute 15-minute generation ceiling can be covered by unit tests or an explicit long-running E2E test.
11. If Audit is enabled, verify `upstream.stream.completed` / `upstream.stream.failed` evidence and exactly one terminal `request.completed` or `request.failed` event per trace.

Cache keys intentionally ignore the delivery-only `stream` and `stream_options` fields, enabling transport-independent reuse. No Redis, Qdrant, or Audit data migration is required for this change.

The principal behavioral guarantee is that no generated response content leaves AI Cost Firewall until the complete response has been approved.

## v0.5.x to v0.6.0

Usage Guard integration is optional and disabled by default, so existing deployments can upgrade without enabling the new module.

1. Back up the current configuration and deployment manifests.
2. Deploy the new AI Cost Firewall image.
3. If Usage Guard will be enabled, deploy VCAL Usage Guard on a network reachable by AI Firewall and configure its service API key and policy.
4. Add the optional `usage_guard_*` directives to `configs/ai-firewall.conf`, or leave `usage_guard_enabled false`.
5. Review `guard_fail_open`; production policy-enforcement deployments should normally use `false`.
6. Verify `/healthz`, `/readyz`, `/version`, and `/metrics`.
7. If Usage Guard is enabled, send both a known allowed request and a known policy-blocked request.
8. Verify `aif_guard_requests_total{guard="usage"}` and `aif_usage_blocks_total`.
9. If Audit is enabled, verify the Usage Guard policy evidence on the same trace.

No Redis, Qdrant, or Audit data migration is required for Usage Guard integration.

## v0.4.2 to v0.5.0

1. Back up the current configuration and deployment manifests.
2. Add `config_version 1;` (omitting it is accepted and defaults to schema 1).
3. Review `guard_fail_open`; production enforcement deployments should normally use `false`.
4. Review `audit_retry_max_backoff_ms` and remember that v0.5.0 Audit delivery remains in-memory/best-effort.
5. Review `max_inflight_requests` and `max_inflight_upstream_requests`.
6. Deploy the new image and verify `/healthz`, `/readyz`, `/version`, and `/metrics`.
7. Send a known request and retain the returned `X-VCAL-Trace-ID`.
8. If Audit is enabled, verify the corresponding trace and terminal event.

No Redis, Qdrant, or Audit data migration is introduced by these v0.5.0 Firewall changes.

## Rollback

The v0.5.0 configuration additions are backward-compatible when removed. Restore the previous image and previous configuration, then verify health/readiness and a known request. If a rollback follows a failed config reload, note that v0.5.0 keeps the previous valid runtime active when replacement runtime construction fails.
