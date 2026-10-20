# AI Cost Firewall — Quick Start

AI Cost Firewall (AIF) is a self-hosted OpenAI-compatible gateway with exact caching (Redis/Valkey),
semantic caching (Qdrant), and Prometheus/Grafana observability. Security, Privacy, Usage, and Audit modules
are **optional** and are not needed to try core AIF.

This guide gets a gateway running and verifies a request. For architecture and operational details, follow
the links at the end rather than configuring everything on the first run.

## Option A — One-command Linux installer

On a supported Linux host with Docker Engine and the Docker Compose plugin installed:

```bash
curl -fsSL https://vcal-project.com/install-aif.sh | bash
```

The installer starts AIF, Redis, Qdrant, Prometheus, and Grafana and provisions dashboards. The tested
platforms described by the project are Ubuntu and Rocky Linux 9/10 (x86_64). Review the [installer
documentation](../deploy/quickstart/README.md) before running a remotely retrieved script, especially on a
production host.

Configure your upstream and embedding endpoint/model in `~/ai-firewall/configs/ai-firewall.conf`, then
restart AIF:

```bash
cd ~/ai-firewall
docker compose restart firewall
```

The installer defaults to **Observe Mode**. A working gateway container does *not* prove that the configured
upstream model is reachable. In Observe Mode, live requests still reach upstream; cache savings are
hypothetical. Grafana is available on the host at `http://127.0.0.1:3000`; the locally generated admin
password is stored in `~/ai-firewall/.env`.

## Option B — Docker Compose from the repository

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

The root Compose stack runs AIF, Redis, Qdrant, Prometheus, and Grafana. The effective enforcement mode is
determined by the config you deploy; the standard configuration's default is `enforce`. This differs from
the one-command installer's Observe Mode default.

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

Repeat the same request. In `enforce`, an eligible second request may be served from exact cache. In
`observe`, it still reaches upstream and only the isolated evaluation metrics record potential reuse.
Cacheability, guard decisions, model parameters, and errors all affect this outcome.

To intentionally bypass cache for a request:

```bash
curl -sS http://localhost:8080/v1/chat/completions \
  -H 'Content-Type: application/json' \
  -H 'X-AIF-Cache-Bypass: true' \
  -d '{"model":"YOUR_CHAT_MODEL","messages":[{"role":"user","content":"Do not cache this request."}]}'
```

A bypass skips exact/semantic lookup **and** store, including shadow operations in Observe Mode.

## Check logs and dashboards

```bash
docker compose logs --tail=100 firewall
curl -sS http://localhost:8080/metrics | head
```

The `/metrics` endpoint can be protected by configuration. Grafana dashboards show traffic, cache
effectiveness, and net savings; [Metrics and Costs](metrics-and-costs.md) explains the distinction between
actual and hypothetical savings.

## Next steps

- [Evaluation Mode](evaluation-mode.md) — assess cache opportunity without serving cached completions.
- [How It Works](how-it-works.md) — understand cache keys and eligibility.
- [Configuration Reference](config-reference.md) — tune providers, timeouts, cache lifetimes, and guards.
- [Operations](operation.md) / [Troubleshooting](troubleshooting.md) — production probes and failure
  diagnosis.
- [Deployment](deployment.md) — OpenShift/Kubernetes baseline and runtime security.
- [Build/test from source](../README.md) — Rust workflow.
