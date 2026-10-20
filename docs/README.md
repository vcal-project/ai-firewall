# AI Cost Firewall Documentation

This directory contains version-specific technical documentation for [AI Cost Firewall](../README.md) (AIF).
Start with the [Quick Start](quickstart.md) or browse by subject below. For guided product documentation,
visit the [AIF documentation portal](https://ai-firewall.docs.vcal-project.com/).

## Get started

- [Quick Start](quickstart.md) — install, configure an upstream, and verify a first request.
- [Deployment](deployment.md) — production deployment and OpenShift considerations.
- [Upgrading](upgrading.md) — release-to-release upgrade notes and rollback checks.
- [Provider compatibility](provider-compatibility.md) — supported OpenAI-compatible patterns and limits.

## Understand AIF

- [Architecture](architecture.md) — components, boundaries, and interactions.
- [How it works](how-it-works.md) — request lifecycle, exact/semantic caching, and cache eligibility.
- [Evaluation / Observe Mode](evaluation-mode.md) — isolated shadow caches and prospective savings.
- [Assessment Context](assessment-context.md) — effective configuration identity and assessment
  reproducibility.
- [Controlled streaming](controlled-streaming.md) — complete-response approval and SSE replay.
- [Structured content](structured-content.md) — JSON message content, guard inspection coverage, and
  privacy-aware caching.

## Configure and operate

- [Configuration reference](config-reference.md) — directives, defaults, and validation.
- [Operations](operation.md) — probes, failure policies, reloads, and graceful shutdown.
- [Metrics and costs](metrics-and-costs.md) — actual versus hypothetical savings and dashboards.
- [Evidence and Audit](audit-integration.md) — evidence schema, delivery, and limitations.
- [Troubleshooting](troubleshooting.md) — symptom-oriented diagnostics.
- [FAQ](faq.md) — brief answers with links to authoritative guides.

## Other repository references

- [Deployment examples](../deploy/examples/) and [OpenShift baseline](../deploy/openshift/)
- [Benchmark methodology](../BENCHMARKS.md) and [Evaluation validation](../EVALUATION_VALIDATION.md)
- [Releases](https://github.com/vcal-project/ai-firewall/releases) and [Contributing](../CONTRIBUTING.md)

**Documentation policy:** each topic has one primary guide. This repository's reference files describe the
AIF release checked out locally; consult release notes when upgrading. The optional VCAL Guard, Audit, and
Compliance modules are separate products.
