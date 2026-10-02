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
