# AI Cost Firewall — Quick Start

AI Cost Firewall (AIF) is a **self-hosted, OpenAI-compatible control point for production AI**. The
open-source core uses **Redis exact caching** and **Qdrant semantic caching**, with Prometheus metrics
and Grafana dashboards. It has **no software license fee**; separately licensed VCAL modules for privacy,
security, organizational usage policy, Audit, and Compliance are optional.

**Observe first, then enforce when the evidence supports it.** This guide takes you from installation
to a first observed request. AIF runs in infrastructure you control; no VCAL-operated SaaS is in the
request path.

## Option A — One-command Linux installer (Observe mode)

On Linux (x86_64 or ARM64), install Docker Engine and the Docker Compose plugin first. The installer also
requires Python 3, curl, OpenSSL, tar, SHA-256 tools, POSIX ACL support, and internet access to GitHub and
Docker Hub; it does **not** install Docker. See the [installer prerequisites](../deploy/quickstart/README.md).

```bash
curl -fsSL https://vcal-project.com/install-aif.sh | bash
```

The installer pulls versioned images for AIF, Redis, Qdrant, Prometheus, and Grafana, and provisions the
**Overview, Diagnostics, and Evaluation** dashboards. The project reports tests on Ubuntu and Rocky Linux
9/10 (x86_64); other supported architectures are not necessarily tested on those distributions. Review
the [installer script](https://github.com/vcal-project/ai-firewall/blob/main/deploy/quickstart/install.sh)
before running a remotely retrieved command, especially on a production host.

Configure your upstream and embedding endpoint/model in `~/ai-firewall/configs/ai-firewall.conf`, then
restart AIF:

```bash
cd ~/ai-firewall
docker compose restart firewall
```

The installer starts in **Observe mode**. Requests allowed by enabled controls still reach the upstream
model: AIF uses isolated evaluation caches to estimate **potential** avoided calls, tokens, and costs
without serving cached responses. This mode does **not** change optional Guard policies.

A healthy AIF container does *not* prove that the configured upstream model is reachable. The installer
binds AIF, Prometheus, and Grafana to local-only ports. Open Grafana at `http://127.0.0.1:3000` on the
installation host (or through an SSH tunnel); its generated admin password is in `~/ai-firewall/.env`.

## Option B — Docker Compose from the repository (Enforce mode by default)

### Prerequisites

You need Docker Engine, the Docker Compose plugin, and a reachable OpenAI-compatible chat endpoint. To
enable semantic caching, you also need a working compatible embedding endpoint/model with the dimension
configured to match Qdrant.

```bash
docker --version
docker compose version
git clone https://github.com/vcal-project/ai-firewall.git
cd ai-firewall
cp configs/ai-firewall.conf.example configs/ai-firewall.conf
```

Edit `configs/ai-firewall.conf` and replace example upstream credentials and endpoints. Treat API keys as
secrets and do not commit the configured file. For local endpoints, use addresses reachable **from the
firewall container**, not necessarily the host's `localhost`.

```bash
docker compose pull
docker compose up -d
```

The root Compose stack runs AIF, Redis, Qdrant, Prometheus, and Grafana. The actual mode follows
`aif_enforcement_mode` in the deployed configuration; its normal runtime default is **Enforce mode**
(`enforce`). Unlike the one-line installer, this path does not automatically switch AIF to Observe mode.
To evaluate cache reuse before enforcement, explicitly configure `aif_enforcement_mode observe;`.

### Alternative example deployments

[Deployment examples](../deploy/examples/) cover cloud OpenAI, local Ollama, OpenRouter, hybrid embeddings,
and a local full stack. Follow the README in a chosen example directory; paths and model names vary by
example. For provider URL and credential rules, see [Provider Compatibility](provider-compatibility.md).

## Verify startup and upstream connectivity

Run from a host that can reach port 8080:

```bash
curl -f http://localhost:8080/healthz
curl -f http://localhost:8080/startupz
curl -f http://localhost:8080/readyz
curl -s http://localhost:8080/version
curl -s http://localhost:8080/assessment-context
```

Successful probes normally return `OK`, `started`, and `ready`. `/startupz` may be stricter than `/readyz`
for initialized dependencies. `/assessment-context` is a sanitized configuration snapshot, not proof that
inference works. Where supported by the chat provider, AIF also proxies `GET /v1/models`.

Verify the upstream **with a real chat request** (replace the model with the exact ID served by your
provider):

```bash
curl -sS http://localhost:8080/v1/chat/completions \
  -H 'Content-Type: application/json' \
  -d '{"model":"YOUR_CHAT_MODEL","messages":[{"role":"user","content":"Explain Redis briefly."}]}'
```

Repeat the same request. In **Enforce mode** (`enforce`), an eligible repeat may be served from Redis
exact cache. In **Observe mode** (`observe`), the allowed request still reaches upstream and isolated
evaluation metrics record **potential** reuse. Cache eligibility, guard decisions, model parameters, and
errors all affect the outcome.

To intentionally bypass cache for a request:

```bash
curl -sS http://localhost:8080/v1/chat/completions \
  -H 'Content-Type: application/json' \
  -H 'X-AIF-Cache-Bypass: true' \
  -d '{"model":"YOUR_CHAT_MODEL","messages":[{"role":"user","content":"Do not cache this request."}]}'
```

A bypass skips exact/semantic cache lookup **and** store, including isolated evaluation-cache operations
in Observe mode.

## Check logs and dashboards

```bash
docker compose logs --tail=100 firewall
curl -sS http://localhost:8080/metrics | head
```

The `/metrics` endpoint can be protected by configuration. Grafana dashboards show traffic, cache
effectiveness, runtime diagnostics, and costs. **Potential Observe-mode savings** are kept separate from
**actual Enforce-mode cache outcomes** (whose avoided costs are still estimates based on configured prices).
See [Metrics and Costs](metrics-and-costs.md).

## Next steps

- [Evaluation Mode](evaluation-mode.md) — assess potential cache savings without serving cached completions.
- [How It Works](how-it-works.md) — understand cache keys and eligibility.
- [Configuration Reference](config-reference.md) — tune providers, timeouts, cache lifetimes, and guards.
- [Operations](operation.md) / [Troubleshooting](troubleshooting.md) — production probes and failure
  diagnosis.
- [Deployment](deployment.md) — OpenShift/Kubernetes baseline and runtime security.
- [AIF documentation portal](https://ai-firewall.docs.vcal-project.com/) — guided product and deployment documentation.
- [Build/test from source](../README.md) — Rust workflow.
