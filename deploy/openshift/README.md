# AI Cost Firewall on OpenShift

This directory contains the OpenShift-specific deployment assets for AIF v0.8.2.
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

AIF v0.8.2 proxies `GET /v1/models` to its configured chat/inference upstream.
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
  for readiness must have initialized successfully in the current process. This
  avoids a startup race where AIF enters fail-open/Observe mode before Redis or
  Qdrant and otherwise keeps a no-op cache until the pod is restarted.

The startup probe gives dependencies up to 60 seconds per process start before
OpenShift restarts the pod. Tune this window for the partner cluster.

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
