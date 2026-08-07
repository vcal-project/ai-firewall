# Upgrading AI Cost Firewall

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
