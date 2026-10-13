# AI Cost Firewall — Docker Quick Start

A single noninteractive Linux installer for **AIF Core v0.8.5**, Redis, Qdrant, Prometheus, Grafana and the three AIF dashboards. The v0.8.4 Quick Start was validated on Ubuntu and Rocky Linux 9.8/10.2 VMs (SELinux Enforcing); repeat a clean-VM smoke test for v0.8.5 before advertising this release. Provider connectivity and real LLM traffic must be configured/tested separately.

## Prerequisites

Linux x86_64 (also accepts aarch64; not yet VM-tested), Docker Engine with accessible daemon, Docker Compose plugin, `bash`, `curl`, `python3`, `tar`, `sha256sum`, `openssl`, `setfacl` (package `acl`). Ports 8080, 9090 and 3000 must be free. The installer does **not** install Docker or modify SELinux.

## Install

After uploading `aif-quickstart-v0.8.5.tar.gz` and its `.sha256` file to the **v0.8.5 GitHub Release** and publishing an appropriate installer URL:

```bash
curl -fsSL https://vcal-project.com/install-aif.sh | bash
```

Until that URL is configured, use the canonical source after reviewing it:

```bash
curl -fsSLo install-aif.sh https://raw.githubusercontent.com/vcal-project/ai-firewall/main/deploy/quickstart/install.sh
bash install-aif.sh
```

For an *unpublished* bundle already copied to the VM:

```bash
AIF_BUNDLE_DIR="$HOME/aif-release-test" bash install.sh
```

The script verifies the archive checksum, generates a local Grafana administrator password and starts all five containers. It refuses to overwrite an existing installation. The checksum and archive served from the same release establish integrity, not independent proof of publisher identity; signed releases are recommended for production distribution.

## After installation

The host installation is `~/ai-firewall`. The services listen only on loopback:

| Service | Local address |
|---|---|
| AIF | http://127.0.0.1:8080 |
| Prometheus | http://127.0.0.1:9090 |
| Grafana | http://127.0.0.1:3000 |

**Provider configuration is mandatory for real requests.** Edit `~/ai-firewall/configs/ai-firewall.conf` to set valid upstream and embedding endpoints (and API keys if required), then run:

```bash
cd ~/ai-firewall && docker compose restart firewall
```

Observe Mode is the default and forwards all chat requests to the upstream; semantic evaluation requires embeddings. `/healthz` and `/readyz` do not certify provider credentials. Access localhost-only Grafana on a remote VM through SSH forwarding, e.g. `ssh -L 3000:127.0.0.1:3000 user@VM_IP`.

The installed `.env` contains **only** `AIF_VERSION` and a **randomly generated** `GRAFANA_ADMIN_PASSWORD`. **Never commit real credentials** to `.env`, the template, the Git repository or the release archive. API keys for the upstream/embedding services must be supplied privately after installation. If real credentials were included in an earlier archive or commit, rotate them.

## Operations

```bash
cd ~/ai-firewall
docker compose ps
docker compose logs --tail=100 firewall
docker compose down      # leaves named volumes intact
```

For a *disposable test deployment only*, `docker compose down -v` permanently deletes Redis/Qdrant/Grafana data. Never run this against customer data without an explicit backup and approval. Re-running the installer with an existing path makes no changes.

## Build release assets

From the AIF repository root:

```bash
bash deploy/quickstart/build-release.sh
```

This generates `dist/aif-quickstart-v0.8.5.tar.gz` and its `.sha256`. Review both before attaching them to the AIF GitHub release. The installer is served separately from the public repository/website.
