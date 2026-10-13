#!/usr/bin/env bash
# AI Cost Firewall Core Quick Start — VCAL Labs, Inc.
# Requires Linux, Docker Engine, Docker Compose plugin, Python 3 and ACL utilities.
set -Eeuo pipefail
umask 077

readonly AIF_VERSION='v0.8.5'
readonly RELEASE_BASE="https://github.com/vcal-project/ai-firewall/releases/download/${AIF_VERSION}"
readonly ARCHIVE="aif-quickstart-${AIF_VERSION}.tar.gz"
readonly INSTALL_DIR="${HOME:?HOME is required}/ai-firewall"
# Unpublished release-candidate testing only. Omit for normal public installation.
readonly AIF_BUNDLE_DIR="${AIF_BUNDLE_DIR:-}"
TMP_DIR=''

info() { printf '[aif] %s\n' "$*"; }
fail() { printf '[aif] ERROR: %s\n' "$*" >&2; exit 1; }
cleanup() { if [[ -n "$TMP_DIR" && -d "$TMP_DIR" ]]; then rm -rf -- "$TMP_DIR"; fi; }
trap cleanup EXIT
trap 'fail "Installation failed near line $LINENO. Check the error above."' ERR

banner() {
  cat <<'BANNER'

============================================================
                   AI COST FIREWALL
                   by VCAL Labs, Inc.
============================================================
  Docker Quick Start  |  AIF + Redis + Qdrant + Monitoring
BANNER
  printf '  Release: %s\n\n' "$AIF_VERSION"
}

[[ "$(uname -s)" == Linux ]] || fail 'Linux is required.'
case "$(uname -m)" in x86_64|aarch64) ;; *) fail 'Supported architectures: x86_64 and aarch64.' ;; esac
for binary in curl docker sha256sum tar openssl python3 setfacl; do
  command -v "$binary" >/dev/null 2>&1 || fail "Missing prerequisite: $binary"
done
docker compose version >/dev/null 2>&1 || fail 'Docker Compose plugin is required.'
docker info >/dev/null 2>&1 || fail 'Docker daemon unavailable. Check service and user permissions.'

banner
if [[ -e "$INSTALL_DIR" || -L "$INSTALL_DIR" ]]; then
  info "Existing installation or path detected at $INSTALL_DIR; no changes made."
  info "Inspect it with: cd '$INSTALL_DIR' && docker compose ps"
  exit 0
fi

# Limit surprise conflicts: Quick Start publishes only loopback ports.
if command -v ss >/dev/null 2>&1; then
  for port in 8080 9090 3000; do
    if ss -H -ltn "sport = :$port" 2>/dev/null | grep -q .; then
      fail "TCP port $port is already listening. No changes made."
    fi
  done
else
  info 'WARNING: ss is unavailable; host port conflicts will be detected by Docker.'
fi

TMP_DIR=$(mktemp -d)
if [[ -n "$AIF_BUNDLE_DIR" ]]; then
  info "Using local release candidate from $AIF_BUNDLE_DIR"
  [[ -f "$AIF_BUNDLE_DIR/$ARCHIVE" && -f "$AIF_BUNDLE_DIR/$ARCHIVE.sha256" ]] || \
    fail 'Local bundle or checksum missing.'
  cp -- "$AIF_BUNDLE_DIR/$ARCHIVE" "$TMP_DIR/$ARCHIVE"
  cp -- "$AIF_BUNDLE_DIR/$ARCHIVE.sha256" "$TMP_DIR/$ARCHIVE.sha256"
else
  info "Downloading versioned deployment files for $AIF_VERSION..."
  curl -fsSL --retry 3 --output "$TMP_DIR/$ARCHIVE" "$RELEASE_BASE/$ARCHIVE"
  curl -fsSL --retry 3 --output "$TMP_DIR/$ARCHIVE.sha256" "$RELEASE_BASE/$ARCHIVE.sha256"
fi

(
  cd "$TMP_DIR"
  sha256sum --check --status -- "$ARCHIVE.sha256"
) || fail 'Release archive checksum verification failed.'

# Accept only the eight expected files. Reject absolute paths, traversal,
# duplicates, links/devices and unexpectedly large input before extraction.
python3 - "$TMP_DIR/$ARCHIVE" <<'PY_CHECK'
import pathlib, sys, tarfile
expected = {
    'compose.yaml', 'ai-firewall.conf.template', 'prometheus.yml',
    'grafana/provisioning/dashboards.yml',
    'grafana/provisioning/datasource.yml',
    'grafana/dashboards/ai-cost-firewall-overview.json',
    'grafana/dashboards/ai-cost-firewall-diagnostics.json',
    'grafana/dashboards/aif-evaluation.json',
}
found = set()
with tarfile.open(sys.argv[1], 'r:gz') as tf:
    for m in tf:
        p = pathlib.PurePosixPath(m.name)
        if p.is_absolute() or '..' in p.parts or not (m.isfile() or m.isdir()):
            raise SystemExit(f'Unsafe member: {m.name}')
        if m.isfile():
            if m.name not in expected or m.name in found or m.size > 2_000_000:
                raise SystemExit(f'Unexpected/duplicate/oversized member: {m.name}')
            found.add(m.name)
        elif str(p).rstrip('/') not in {str(pathlib.PurePosixPath(x).parent) for x in expected} | {'grafana', 'grafana/dashboards', 'grafana/provisioning'}:
            raise SystemExit(f'Unexpected directory: {m.name}')
if found != expected:
    raise SystemExit(f'Missing files: {sorted(expected - found)}')
PY_CHECK

mkdir -p "$TMP_DIR/stage"
tar -xzf "$TMP_DIR/$ARCHIVE" -C "$TMP_DIR/stage" --no-same-owner --no-same-permissions
stage="$TMP_DIR/stage"

# The AIF runtime runs as UID 65532. Keep the config private to the owner
# while granting read access to that exact runtime UID via POSIX ACL.
mkdir -p "$stage/configs"
cp -- "$stage/ai-firewall.conf.template" "$stage/configs/ai-firewall.conf"
cat > "$stage/.env" <<ENV_FILE
AIF_VERSION=${AIF_VERSION}
GRAFANA_ADMIN_PASSWORD=$(openssl rand -hex 24)
ENV_FILE
chmod 755 "$stage" "$stage/grafana" "$stage/grafana/dashboards" "$stage/grafana/provisioning"
chmod 644 "$stage/compose.yaml" "$stage/ai-firewall.conf.template" "$stage/prometheus.yml"
find "$stage/grafana" -type f -exec chmod 644 {} +
chmod 700 "$stage/configs"
chmod 600 "$stage/.env" "$stage/configs/ai-firewall.conf"
setfacl -m u:65532:--x "$stage/configs" || fail 'Could not grant AIF access to config directory.'
setfacl -m u:65532:r-- "$stage/configs/ai-firewall.conf" || fail 'Could not grant AIF read access to config.'

(
  cd "$stage"
  docker compose config --quiet
) || fail 'Docker Compose configuration validation failed.'

# Move to the permanent path only after configuration validation.
# mkdir/copy are never applied to an already existing deployment.
[[ ! -e "$INSTALL_DIR" && ! -L "$INSTALL_DIR" ]] || fail 'Installation path appeared during setup; refusing to overwrite.'
mv -- "$stage" "$INSTALL_DIR"
info "Deployment files installed in $INSTALL_DIR"
info 'Pulling images from Docker Hub and starting all five services...'
if ! (
  cd "$INSTALL_DIR"
  docker compose pull &&
    docker compose up -d --wait --wait-timeout 180 &&
    docker compose ps
); then
  fail "Startup failed. Files preserved in $INSTALL_DIR. Inspect: cd '$INSTALL_DIR' && docker compose ps -a && docker compose logs --tail=100"
fi

cat <<EOF_SUMMARY

============================================================
  INSTALLATION COMPLETE — PROVIDER CONFIGURATION REQUIRED
============================================================
  AIF:        http://127.0.0.1:8080
  Prometheus: http://127.0.0.1:9090
  Grafana:    http://127.0.0.1:3000  (user: admin)

  Grafana password: in $INSTALL_DIR/.env (keep private)
  AIF mode: OBSERVE — all requests still call the upstream LLM.

  ACTION REQUIRED BEFORE REAL LLM REQUESTS
  1. Edit $INSTALL_DIR/configs/ai-firewall.conf
  2. Set a valid upstream LLM endpoint and authentication, if needed.
  3. Set a compatible embedding endpoint/model for semantic evaluation.
  4. Restart: cd '$INSTALL_DIR' && docker compose restart firewall

  Health/readiness indicate running infrastructure, NOT provider validity.
  Keep real provider API keys only in your private local configuration.
============================================================
EOF_SUMMARY
