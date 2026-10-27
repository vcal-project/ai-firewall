# AI Cost Firewall

![Rust](https://img.shields.io/badge/Rust-stable-orange)
![License](https://img.shields.io/github/license/vcal-project/ai-firewall)
![GitHub Release](https://img.shields.io/github/v/release/vcal-project/ai-firewall)
![Docker Pulls](https://img.shields.io/docker/pulls/vcalproject/ai-firewall)
![Status](https://img.shields.io/badge/status-production--ready-brightgreen)

**Control what reaches your AI models — and what doesn’t need to.**

AI Cost Firewall (AIF) is a **self-hosted, OpenAI-compatible control point** in front of production AI workloads. It uses **Redis exact caching** and **Qdrant semantic caching** to identify repeated or similar requests, while providing metrics and structured evidence for cost and operational visibility.

Start in **Observe mode** to estimate potential cache savings on real traffic without changing application responses. Switch to **Enforce mode** when the evidence supports serving eligible cache hits.

The **AIF core gateway is open source and has no software license fee**. Optional VCAL modules add privacy, security, organizational usage-policy, Audit, and Compliance capabilities. No VCAL-operated SaaS is part of the request path.

<p align="center">
  <img src="assets/ai-cost-firewall-overview.gif" alt="AI Cost Firewall and optional VCAL modules" width="900">
</p>

Developed and maintained by **VCAL Labs, Inc.** · [Website](https://vcal-project.com/)· [Documentation portal](https://ai-firewall.docs.vcal-project.com/) · [Releases](https://github.com/vcal-project/ai-firewall/releases)

## Quick Start

On Linux (x86_64 or ARM64), with Docker Engine and Docker Compose already installed:

```bash
curl -fsSL https://vcal-project.com/install-aif.sh | bash
```

The installer pulls versioned images for **AIF, Redis, Qdrant, Prometheus, and Grafana** and provisions the Overview, Diagnostics, and Evaluation dashboards. It also requires Python 3 and standard utilities; see the installer guide for the complete prerequisites. It has been tested on Ubuntu and Rocky Linux 9/10 (x86_64).

1. Set a valid OpenAI-compatible upstream URL and authentication (if required) in `~/ai-firewall/configs/ai-firewall.conf`. Configure a compatible embedding endpoint/model for semantic-cache evaluation.
2. Restart AIF:

   ```bash
   cd ~/ai-firewall && docker compose restart firewall
   ```
3. Open Grafana at `http://127.0.0.1:3000` **on the installation host** (or connect through an SSH tunnel). The installer binds AIF, Prometheus, and Grafana to local-only ports. The generated Grafana admin password is stored in `~/ai-firewall/.env`.

**The one-line installer starts in Observe mode.** Every request still reaches the upstream model; AIF evaluates potential cache reuse without serving cached responses. Healthy containers do not by themselves prove provider connectivity.

Read the [installer prerequisites and troubleshooting](deploy/quickstart/README.md), or follow the [manual Docker Compose guide](docs/quickstart.md). Review a remote installation script before running it.

## Why AI Cost Firewall?

Repeated and semantically similar LLM requests can waste tokens and incur avoidable provider latency. In **Enforce mode**, AIF can serve eligible responses from cache instead of calling the upstream model.

```text
Application → AI Cost Firewall → Redis exact cache → Qdrant semantic cache
                           └── on miss/bypass → OpenAI-compatible LLM
```

In **Observe mode**, AIF uses isolated evaluation caches to identify potential cache hits and estimate avoidable calls, tokens, and gross/net cost without substituting cached answers for live upstream responses.

[How AIF works](docs/how-it-works.md) · [Architecture](docs/architecture.md)

## Key Capabilities

- **Two-layer caching:** Redis exact cache and Qdrant semantic cache with configurable cache controls.
- **OpenAI-compatible APIs:** chat completions, `GET /v1/models` proxying, and controlled SSE delivery for streaming chat completions.
- **Observe before Enforce:** measure potential savings with isolated evaluation caches, then serve eligible cache hits when you choose to enforce.
- **Cost and operational visibility:** Prometheus metrics, Grafana dashboards, per-model cost estimates, cache diagnostics, and evidence events.
- **Operational controls:** configuration validation, readiness/startup/liveness probes, configurable fail-open/fail-closed behavior, request limits, and graceful shutdown.
- **Deployment flexibility:** Docker Compose examples and an OpenShift `restricted-v2` deployment baseline.
- **Private-provider HTTPS:** optional CA bundles for upstream LLM and embedding endpoints, including controlled streaming, without disabling certificate or hostname verification.

**Controlled streaming is buffered:** AIF assembles and checks the full provider response before replaying it as SSE. It does **not** forward model tokens immediately as they arrive. [Details](docs/controlled-streaming.md).

## Observe mode vs. Enforce mode

| Mode | Cache decision | Client response | Savings figures |
|---|---|---|---|
| `observe` | Isolated evaluation caches | Live upstream response | **Potential** cache hits, calls/tokens avoided, and estimated savings |
| `enforce` | Production exact/semantic caches | Eligible cache hit, otherwise upstream | **Actual** cache outcomes with estimated avoided cost |

```conf
# Installer default: observe; normal runtime default: enforce.
aif_enforcement_mode observe;
```

AIF Observe mode evaluates **caching only**. It does not change the configured policy or enforcement behavior of optional VCAL Guards. For assessment reproducibility, AIF provides the sanitized `GET /assessment-context` endpoint and a configuration identity metric.

[Evaluation Mode](docs/evaluation-mode.md) · [Assessment Context](docs/assessment-context.md) · [Evaluation validation](EVALUATION_VALIDATION.md)

## Optional VCAL modules

AIF can orchestrate separately licensed VCAL modules. **None is required for the open-source AIF gateway.**

| Module | Function |
|---|---|
| Privacy Guard | Detect, redact, or anonymize sensitive data before upstream calls and optionally restore approved placeholders |
| Security Guard | Detect and block prompt injection, jailbreak attempts, and other supported text-based LLM security risks |
| Usage Guard | Allow, warn, or block requests under organizational AI usage policies |
| Audit | Retain structured evidence and reconstruct request traces |
| Compliance | Evaluate retained evidence and support governance reporting |

Guards can be enabled individually or together. Inspection coverage depends on each module and adapter; do not assume every Guard inspects every structured content shape. [Guard operation](docs/operation.md) · [Audit integration](docs/audit-integration.md).

**VCAL Console** separates actual production outcomes from hypothetical Observe-mode results. It is included with an active annual **VCAL Production Support** subscription for AIF or an applicable VCAL module license; no separate Console license is required.

**VCAL Semantic Cache Server** is a separate, standalone HTTP semantic-cache service for other applications and RAG pipelines. AIF uses Redis and Qdrant; **no native AIF adapter** for the Semantic Cache Server is currently available.

## API and Provider Compatibility

AIF expects **OpenAI-compatible** chat and embedding endpoints. Common patterns include OpenAI, Ollama, LM Studio, vLLM, LiteLLM, and OpenRouter. The chat and embedding endpoints can be configured separately.

Current gateway routes include `POST /v1/chat/completions` and `GET /v1/models` (discovery through the configured chat upstream). Provider SSE support is needed for `stream=true`, not for normal JSON responses.

**Limitations:** AIF is not a universal provider adapter. Native Anthropic, Gemini, Mistral, and Cohere APIs are not directly supported; use an appropriate OpenAI-compatible compatibility layer. Structured message arrays have specific cache and guard-inspection limits. AIF does **not** fetch or inspect referenced documents, file attachments, or opaque non-text parts.

[Provider Compatibility](docs/provider-compatibility.md) · [Structured Content and Privacy-aware Caching](docs/structured-content.md)

## Dashboards and Evidence

The Docker deployment includes **Overview, Diagnostics, and Evaluation** Grafana dashboards for cost, cache effectiveness, runtime diagnostics, and potential savings. Observe-mode estimates are **not** counted as actual production cache savings.

[![AIF Cost Savings Overview dashboard](assets/grafana/ai-firewall-overview-021.png)](assets/grafana/ai-firewall-overview-021.png)

*Historical demo screenshot from a controlled local workload with simulated upstream; not a production performance guarantee.*

AIF also emits structured evidence (`schema_version: 1.1`), with optional asynchronous delivery to VCAL Audit. Evidence metadata does not include prompt or response content.

[Metrics and Dashboards](docs/metrics-and-costs.md) · [Audit Integration](docs/audit-integration.md)

## Deployment and Configuration

Run AIF on premises, in a private cloud, or in other infrastructure you control. Keep your OpenAI-compatible clients, providers, and operational tooling.

| Start here | Description |
|---|---|
| [One-line installer](deploy/quickstart/README.md) | Five-container stack with monitoring |
| [Manual installation](docs/quickstart.md) | Docker Compose and source-based setup |
| [Deployment examples](deploy/examples/) | Cloud, local Ollama, hybrid, and OpenRouter configurations |
| [OpenShift baseline](deploy/openshift/) | Security-restricted OpenShift manifests |
| [Private CA and TLS](docs/private-ca-tls.md) | Custom certificate trust for internal HTTPS model and embedding endpoints |
| [Configuration Reference](docs/config-reference.md) | Nginx-style directives and examples |
| [Operations](docs/operation.md) | Probes, reload, fail-open behavior, and runtime controls |
| [Troubleshooting](docs/troubleshooting.md) | Provider and cache diagnostics |

### Private HTTPS endpoints

For OpenShift service certificates or other private PKI, mount a PEM CA bundle inside the AIF container and configure trust independently for the upstream LLM and embedding provider:

```conf
upstream_ca_bundle /etc/aif-certs/service-ca.crt;
embedding_ca_bundle /etc/aif-certs/service-ca.crt;
```

Both settings are optional. They add CA certificates to the existing trust roots; TLS certificate-chain and hostname verification remain enabled for normal and streaming requests. AIF rejects an unreadable or invalid configured CA bundle during initialization. Restart the AIF container after rotating CA files. Setting `SSL_CERT_FILE` alone is not a substitute for these directives. **Observe mode still requires a working HTTPS connection to the live upstream.**

See [Private CA and TLS](docs/private-ca-tls.md) for volume mounting, OpenShift service CA injection, and verification steps.

Validate a repository-based deployment with:

```bash
curl http://localhost:8080/healthz
curl http://localhost:8080/readyz
curl http://localhost:8080/version
```

For provider connectivity, also test the configured upstream or its model-discovery endpoint; health checks alone are not sufficient.

## Benchmarks and Validation

Historical controlled tests with a **local simulated upstream** demonstrated cache effectiveness and gateway throughput. Results depend on test environment and workload; they are **not production capacity guarantees**.

[Benchmark report](BENCHMARKS.md) · [Observe/Enforce validation](EVALUATION_VALIDATION.md)

## Further Documentation

| Topic | Guide |
|---|---|
| Architecture and request flow | [Architecture](docs/architecture.md) · [How It Works](docs/how-it-works.md) |
| Observe mode and assessments | [Evaluation Mode](docs/evaluation-mode.md) · [Assessment Context](docs/assessment-context.md) |
| Streaming and content | [Controlled Streaming](docs/controlled-streaming.md) · [Structured Content](docs/structured-content.md) |
| Private HTTPS providers | [Private CA and TLS](docs/private-ca-tls.md) · [Provider Compatibility](docs/provider-compatibility.md) |
| Monitoring | [Metrics and Costs](docs/metrics-and-costs.md) |
| Audit and evidence | [Audit Integration](docs/audit-integration.md) |
| Questions and examples | [FAQ](docs/faq.md) · [Examples](deploy/examples/) |

Browse the [AIF documentation portal](https://ai-firewall.docs.vcal-project.com/) for deployment guides and additional product guidance.

## Build, Test, and Contribute

```bash
git clone https://github.com/vcal-project/ai-firewall.git
cd ai-firewall
cargo build --release
cargo test
```

Contributions in documentation, provider compatibility, deployments, observability, testing, and performance are welcome. See [CONTRIBUTING.md](CONTRIBUTING.md).

## Releases and License

See [GitHub Releases](https://github.com/vcal-project/ai-firewall/releases) for version-specific features, fixes, and compatibility notes.

Licensed under the **Apache License 2.0**. See [LICENSE](LICENSE).
