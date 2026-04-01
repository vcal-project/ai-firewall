# Operational Behavior

This section describes how AI Cost Firewall behaves at runtime: health checks, graceful shutdown, and live configuration reload.

---

## Health & Readiness Endpoints

AI Cost Firewall exposes two standard endpoints:

### `/healthz` — Liveness

Indicates that the process is running.

- Returns: `200 OK`
- Does not depend on downstream services
- Used by orchestrators to detect crashes

```
curl -i http://localhost:8080/healthz
```

---

### `/readyz` — Readiness

Indicates whether the service is ready to accept new requests.

- Returns:
  - `200 OK` → ready
  - `503 Service Unavailable` → not accepting traffic
- Used by load balancers and orchestrators

```
curl -i http://localhost:8080/readyz
```

---

### Behavior summary

| State                 | `/healthz` | `/readyz` |
|----------------------|-----------|----------|
| Normal operation     | 200       | 200      |
| During shutdown      | 200       | 503      |
| Process crashed      | ❌        | ❌       |

---

## Graceful Shutdown

AI Cost Firewall supports graceful shutdown on:

- `SIGTERM`
- `SIGINT`

### Shutdown sequence

1. Receive signal
2. `/readyz` returns `503`
3. New requests are rejected
4. In-flight requests complete
5. Process exits after timeout (~10s)

---

## Request Rejection During Shutdown

```
{
  "error": {
    "code": 503,
    "message": "server is shutting down",
    "type": "service_unavailable"
  }
}
```

---

## Configuration Reload (SIGHUP)

Reload without restart:

```
kill -HUP <pid>
```

### Behavior

- Config reloaded and validated
- Runtime rebuilt (Redis, Qdrant, embeddings, upstream)
- Atomic swap
- No downtime

---

## Metrics

- `aif_inflight_requests`
- `aif_shutdown_in_progress`
- `aif_shutdown_rejections_total`

---

## Notes

- No automatic file watching
- Reload via SIGHUP only
- Long requests may be cut after timeout
