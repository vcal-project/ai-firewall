# Evaluation / Observe Mode

AIF includes an AIF-level Evaluation Mode for low-risk production pilots. Assessment Context and deployment hardening complement this mode without changing the Observe/Enforce request-path semantics.

Configure the runtime mode with:

```text
aif_enforcement_mode enforce;
```

or:

```text
aif_enforcement_mode observe;
```

`enforce` is the default and preserves the normal production cache behavior.

In `observe` mode, AI Cost Firewall remains in the live application request/response path but does not serve cached responses to the client. Instead, it evaluates what exact and semantic caching would have done while continuing to call the configured upstream provider.

The core observe-mode guarantees are:

- the live application still receives the upstream response;
- exact-cache evaluation uses an isolated Redis key namespace;
- semantic-cache evaluation uses an isolated Qdrant collection;
- shadow hits record what would have been served without suppressing the real upstream request;
- shadow misses populate only evaluation cache state;
- production cache state is not populated or reused by observe mode;
- cache-bypass requests skip both evaluation lookup and evaluation store;
- Redis, Qdrant, or embedding failures used only for evaluation do not interrupt live application traffic;
- AIF remains ready when optional evaluation dependencies are temporarily unavailable; deployments that mark Redis or Qdrant as required for readiness can use `/startupz` to require successful initialization before the pod is considered started;
- Redis and Qdrant evaluation paths recover after dependency restart;
- controlled streaming and non-streaming requests retain the same transport-independent cache identity.

Evaluation Mode is currently an **AI Cost Firewall cache/optimization feature**. It does not change the configured enforcement behavior of VCAL Security Guard, Privacy Guard, or Usage Guard. Guard-specific `off / observe / enforce` modes are not part of this release.

## Evaluation evidence

Evaluation-aware evidence records the effective mode and separates the hypothetical action from what actually happened to the live request.

Typical cache-hit evidence includes:

```json
{
  "enforcement_mode": "observe",
  "decision": "exact_hit",
  "would_action": "serve_exact_cache",
  "applied_action": "call_upstream"
}
```

A cache bypass remains explicit:

```json
{
  "enforcement_mode": "observe",
  "decision": "cache_bypassed",
  "would_action": "call_upstream",
  "applied_action": "call_upstream"
}
```

This keeps VCAL Audit traces suitable for later pilot assessment without representing hypothetical actions as production actions.

## Evaluation accounting

Observe-mode cache opportunities do **not** increment the normal production cache-hit or savings counters.

Dedicated evaluation metrics are used for:

- requests evaluated;
- would-have exact and semantic hits;
- potentially avoidable upstream calls;
- potentially avoidable tokens;
- estimated gross and net cost avoidance;
- shadow-store outcomes;
- evaluation errors.

This separation prevents hypothetical pilot savings from being mixed with actual production savings.

## Assessment support

Evaluation Mode produces the hypothetical cache/cost telemetry. Assessment Context provides the effective runtime/configuration identity needed to interpret that telemetry reproducibly.

AIF itself does not create or persist assessment reports. A separate assessment/reporting layer can combine:

```text
bounded Evaluation telemetry
+
GET /assessment-context
+
historical aif_runtime_info
```

to verify configuration stability and freeze a bounded Observe-mode assessment.

For configuration stability and the sanitized assessment snapshot, see [Assessment Context and Configuration Identity](assessment-context.md). For the original workload comparison, see [Evaluation Mode Validation](../EVALUATION_VALIDATION.md).
