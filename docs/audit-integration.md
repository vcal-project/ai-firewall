# Evidence Events and VCAL Audit

AIF emits structured operational evidence locally and can optionally deliver it to separately licensed
**VCAL Audit**. Audit is **not required** to run AIF Core. This guide describes trace semantics, delivery
behavior, and operational risks.

## Evidence contract

AIF uses event type `vcal.evidence.event`, with documented `schema_version: 1.1`. Each received request
trace has a stable `trace_id` that correlates validation, cache lookups, upstream calls, guard outcomes,
response processing, and exactly one terminal `request.completed` or `request.failed` event.

Guard evidence contains operational metadata, **not raw prompt or assistant-response content**. In Observe
Mode, evidence separates the hypothetical cache decision from the live action using `enforcement_mode`,
`decision`, `would_action`, and `applied_action`.

Example conceptual Observe cache-hit attributes:

```json
{
  "enforcement_mode": "observe",
  "decision": "exact_hit",
  "would_action": "serve_exact_cache",
  "applied_action": "call_upstream"
}
```

See [Evaluation Mode](evaluation-mode.md) for the isolation and accounting semantics. Changes to the
evidence schema should be tracked separately from AIF product version changes.

## Local evidence and optional delivery

For local structured evidence logging, configure:

```text
RUST_LOG=info,vcal_evidence=info
```

To enable the optional buffered HTTP delivery path (adjust to your network and secret handling):

```conf
audit_enabled true;
audit_url http://vcal-audit:8092;
audit_api_key replace-with-shared-audit-token;
audit_queue_capacity 10000;
audit_batch_size 100;
audit_flush_interval_ms 1000;
audit_timeout_seconds 5;
audit_retry_max_attempts 5;
audit_retry_initial_backoff_ms 250;
```

The sender queues event records in memory, groups them by batch size or flush interval, posts batches to the
Audit endpoint, and retries failures using configured backoff. AIF's request path normally does not wait for
durable Audit persistence.

### Durability and failure policy

**AIF-to-Audit delivery is best effort**, not a persistent local spool or end-to-end durability guarantee.
Evidence can be lost if the bounded queue fills, retries are exhausted, or the process terminates before
queued events flush. AIF attempts a best-effort flush during graceful shutdown.

VCAL Audit provides authenticated ingestion, persistent event storage, trace reconstruction, NDJSON export,
and integrity verification **after successful ingestion**. A successful AIF chat response does not prove its
evidence reached Audit.

## Operational verification

1. Ensure AIF and Audit share resolvable routing/network access; the Audit endpoint must accept the configured token.
2. Check Audit service health and its ingest API behavior; an HTTP 401 typically means an API key mismatch.
3. Generate a known request, record its trace ID, and confirm the event appears in Audit along with a single terminal event.
4. Monitor delivery retries, queue fullness, dropped batches, and Audit availability. The sender does not replay batches discarded after retry exhaustion.
5. Allow a graceful shutdown interval to flush available queued records when stopping AIF.

Consult [Troubleshooting](troubleshooting.md#vcal-audit-delivery-problems) for common failure symptoms, and
[Operations](operation.md) for shutdown and logging.
