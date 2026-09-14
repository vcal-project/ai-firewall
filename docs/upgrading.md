# Upgrading AI Cost Firewall

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
