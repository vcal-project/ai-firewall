# Metrics policy

Prometheus labels must come from bounded vocabularies. Approved examples are endpoint class, cache type, dependency, failure class, guard, stage, action and bounded policy/rule IDs.

Never use trace IDs, user IDs, prompts, responses, cache keys, arbitrary URLs, arbitrary error messages or customer-supplied strings as metric labels.

`aif_dependency_failures_total{dependency,class}` provides a common dependency failure taxonomy. `/readyz` and `aif_readiness_state` represent serving readiness rather than only process liveness.
