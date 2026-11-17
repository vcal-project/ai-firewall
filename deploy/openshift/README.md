# AI Cost Firewall on OpenShift

This directory contains the OpenShift-specific deployment assets for AIF v0.8.8.
The AIF image itself remains a generic OCI image and the normal
`docker-compose.yml` deployment remains supported.

The initial design-partner topology is expected to be:

```text
Open WebUI -> AIF -> vLLM chat endpoint -> gpt-oss:20b
                  \
                   -> vLLM embedding endpoint -> Nomic embedding model -> Qdrant
```

The exact vLLM Service names, served model IDs, embedding model/version, and
embedding vector dimension are installation-specific and must be verified in the
MSSP cluster before semantic caching is enabled.

## Security model

The base Deployment is intended to run under OpenShift `restricted-v2` without
requesting `anyuid` or a custom SCC. It deliberately does not set `runAsUser` or
`runAsGroup`, drops all Linux capabilities, disables privilege escalation, uses
a read-only root filesystem, uses `RuntimeDefault` seccomp, and does not mount a
service-account token.

AIF does not write to its application directory. OpenShift may therefore assign
a namespace-specific runtime UID without requiring a writable application
filesystem. Outside OpenShift, the image keeps its normal numeric non-root
fallback user.

## Before applying

Create the Secret first. The example is intentionally not included by the base
Kustomization:

```bash
cp deploy/openshift/examples/secret.example.yaml /tmp/ai-firewall-secret.yaml
# edit /tmp/ai-firewall-secret.yaml
oc apply -f /tmp/ai-firewall-secret.yaml
```

Then review `base/configmap.yaml` and replace at least the cluster-local chat and
embedding vLLM Service names. Apply with:

```bash
oc apply -k deploy/openshift/base
```

## vLLM model discovery

Since v0.8.2, AIF proxies `GET /v1/models` to its configured chat/inference upstream.
This lets Open WebUI discover the model name exposed by the gpt-oss vLLM server.
The embedding endpoint is intentionally separate and is not exposed through that
chat-side discovery route.

Before enabling semantic cache, query the embedding vLLM service directly:

```bash
curl -s http://vllm-embeddings:8000/v1/models | jq
```

Set `AIF_EMBEDDING_MODEL` to the exact served ID. Then issue one embedding request
and set `AIF_QDRANT_VECTOR_SIZE` to the returned embedding vector length. Do not
assume a Nomic dimension from the family name because the exact model and serving
configuration can change the output dimension.

Only after those values are verified should you switch:

```text
AIF_SEMANTIC_CACHE_ENABLED=true
AIF_READINESS_REQUIRES_QDRANT=true
```

## Probes

- `/healthz` is liveness only.
- `/readyz` is traffic readiness and keeps the existing AIF fail-open / Observe
  semantics.
- `/startupz` is deliberately stricter: cache backends configured as required
  for readiness must be available. With the v0.8.8 recovery patches, a
  Redis-backed exact cache in fail-open/Observe mode can recover after Redis is
  restored, including when Redis was unavailable at AIF startup. A Qdrant
  initialization failure may still require a runtime rebuild.

The supplied v0.8.8 base Deployment correctly uses HTTP `/startupz` for startup,
`/readyz` for readiness, and `/healthz` for liveness. Verify the **live** pod's
probe configuration: a manually deployed pod may instead use the CLI
`--healthcheck` for readiness. That CLI option deliberately checks `/healthz`
for backward compatibility, so it must not be used as a dependency-aware
readiness probe. If an exec probe is required, use `--readycheck` (which checks
`/readyz`), but HTTP probes are preferred for OpenShift. `--readycheck`
requires an AIF binary built with Patch 03 (v0.8.8); do not use it with the
older image tag currently pinned by this example. The CLI target address is
`127.0.0.1:8080` unless overridden with `AIF_HEALTHCHECK_ADDR`.

For example, inspect the effective running pod:

```bash
oc -n <namespace> get pod <aif-pod> \
  -o jsonpath='{.spec.containers[0].readinessProbe}{"\n"}{.spec.containers[0].livenessProbe}{"\n"}{.spec.containers[0].startupProbe}{"\n"}'
```

After publishing the v0.8.8 image, verify that the actual Deployment uses that
image (prefer an immutable digest in a controlled pilot), and watch the rollout:

```bash
oc -n <namespace> rollout status deployment/ai-firewall
oc -n <namespace> get deployment ai-firewall \
  -o jsonpath='{.spec.template.spec.containers[0].image}{"\n"}'
oc -n <namespace> get pods -l app.kubernetes.io/name=ai-firewall
```

Do not infer dependency health from `Running` or a liveness-only CLI probe.
Confirm the live `/readyz` HTTP response and the Redis write-health metric
before declaring Redis recovery complete. Redis AOF persistence is unchanged by
this AIF release.

The startup probe gives dependencies up to 60 seconds per process start before
OpenShift restarts the pod. Tune this window for the partner cluster. A strict
Redis startup requirement may cause restarts during a prolonged Redis outage,
even though AIF's fail-open cache can recover if allowed to keep running.

## Initial design-partner posture

Start AIF in `observe`. Exact cache can be enabled immediately. The base example
keeps semantic cache disabled until the Nomic model ID and vector dimension have
been verified. After that, enable it in Observe mode and validate shadow hit/miss
and savings metrics before changing AIF to `enforce`.

`AIF_ALLOW_UNKNOWN_MODELS_PASS_THROUGH=true` is intentional for local vLLM model
IDs. If a stable local model ID and cost model are defined later, move to an
explicit AIF model-price configuration and disable unknown-model pass-through.

## NetworkPolicy and ServiceMonitor

Files under `examples/` are not applied automatically. Adapt their selectors to
the MSSP cluster before enabling them. In particular, do not add a default-deny
egress policy until DNS, Redis, Qdrant, the embedding vLLM service, the chat vLLM
service, Audit, and any guard-service egress paths have all been enumerated.


## Private HTTPS endpoints using OpenShift service CA

AIF's upstream chat (including controlled streaming) and embedding HTTP clients
support optional, **additive** PEM trust roots. The built-in TLS verification,
including hostname/SAN checks, remains enabled. This is independent of Observe
versus Enforce mode. Do not use `danger_accept_invalid_certs` or an HTTP
fallback to work around internal certificate trust.

OpenShift can inject its service signing CA into a ConfigMap in the **AIF
namespace**. For example, in namespace `aif-pilot`:

```bash
oc -n aif-pilot annotate configmap aif-service-ca \
  service.beta.openshift.io/inject-cabundle=true --overwrite
oc -n aif-pilot get configmap aif-service-ca -o jsonpath='{.data.service-ca\.crt}' | head -c 80
```

Create the `aif-service-ca` ConfigMap before annotating it if it does not
exist. Mount its `service-ca.crt` key read-only at
`/etc/aif-certs/service-ca.crt` in the **AIF container**. The supplied
OpenShift baseline is illustrative; it does **not** automatically create or
mount this ConfigMap. Ensure that the arbitrary OpenShift runtime UID can read
the mounted file. Mount this trust bundle in addition to the application's
normal configuration; do not overwrite the system trust directory.

With nginx-style `ai-firewall.conf`:

```nginx
upstream_ca_bundle /etc/aif-certs/service-ca.crt;
embedding_ca_bundle /etc/aif-certs/service-ca.crt;
```

Only when running **without** the configuration file, equivalent environment
settings are `AIF_UPSTREAM_CA_BUNDLE` and `AIF_EMBEDDING_CA_BUNDLE`. With the
current `reqwest`/WebPKI setup, `SSL_CERT_FILE` alone does not add trust to
these clients. All configured CA paths must be readable and contain valid PEM
certificates when AIF starts. Defaults remain unchanged if directives are
omitted. Custom CA roots apply to the chat and embedding clients only, not
Audit/Guard/evidence clients, Redis, Qdrant, or incoming AIF server TLS.

1. From the same network context as the AIF pod, verify the provider with
   `curl --cacert /etc/aif-certs/service-ca.crt https://<service>.<namespace>.svc.cluster.local:<port>/v1/models`
   (or another supported endpoint). **Do not use `-k`.** The hostname must
   match the service certificate's DNS SAN.
2. Restart the AIF pod after changing either CA path or CA contents, including
   OpenShift service CA rotation. SIGHUP cannot replace these trust roots.
3. Check INFO logs for `loaded custom upstream CA roots for buffered and streaming clients`
   and `loaded custom CA roots for embedding client`.
4. Exercise non-streaming and `stream=true` chat through AIF. For embeddings,
   send a fresh prompt with semantic evaluation enabled and confirm a completed
   semantic lookup rather than an `upstream_tls_error`; a cache hit or disabled
   semantic evaluation does **not** prove embedding connectivity.
5. Negative tests: an untrusted root or hostname mismatch must still fail TLS
   verification (do not perform destructive negative tests against production).

For more detail, see [CA configuration and diagnostics](../../docs/private-ca-tls.md).


### Redis recovery diagnostics (v0.8.8)

When Redis exact caching is configured, `/metrics` exposes these low-cardinality
series in addition to `aif_dependency_failures_total`:

| Metric | Meaning |
| --- | --- |
| `aif_redis_write_healthy` | Most recent periodic `SETEX` write-health result (`1` healthy; `0` unhealthy; `-1` unknown/not yet probed). It is **not** a substitute for `/readyz`. |
| `aif_redis_write_probes_total{result}` | Periodic write probes, with fixed result classes: `success`, `timeout`, `persistence_rejected` (Redis `MISCONF`), `write_rejected` (Redis `READONLY`), `connection_error`, `redis_error`, `client_unavailable`. |
| `aif_redis_reconnect_attempts_total{result}` | Connection-manager replacement attempts, with fixed outcomes `success`, `error`, `timeout`. Success does **not** prove Redis writes work. |
| `aif_redis_write_transitions_total{state}` | Monitor-observed transitions to `available` or `unavailable`; transitions are not counted on every failed probe. |

The write-health monitor probes at five-second intervals. AOF write failures
can return `MISCONF` while Redis still responds to `PING`; inspect the
`persistence_rejected` series, Redis AOF logs and `INFO persistence` together.
A write-health recovery log entry follows a successful `SETEX`, not merely a
connection-manager refresh. Metric labels must not include Redis URLs, keys,
error text, tenant identifiers, or other unbounded values. Redis AOF is kept
**enabled** for the pilot and future privacy mapping requirements.
