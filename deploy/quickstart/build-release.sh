#!/usr/bin/env bash
set -Eeuo pipefail
ROOT=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/../.." && pwd)
SRC="$ROOT/deploy/quickstart"
DIST="$ROOT/dist"
VERSION='v0.8.4'
ARCHIVE="aif-quickstart-${VERSION}.tar.gz"
mkdir -p "$DIST"
files=(
  compose.yaml ai-firewall.conf.template prometheus.yml
  grafana/dashboards/ai-cost-firewall-overview.json
  grafana/dashboards/ai-cost-firewall-diagnostics.json
  grafana/dashboards/aif-evaluation.json
  grafana/provisioning/dashboards.yml
  grafana/provisioning/datasource.yml
)
for f in "${files[@]}"; do [[ -f "$SRC/$f" ]] || { echo "Missing: $f" >&2; exit 1; }; done
bash -n "$SRC/install.sh"
python3 - "$SRC/ai-firewall.conf.template" <<'PY_CHECK'
from pathlib import Path
import sys,re
s=Path(sys.argv[1]).read_text()
for name in ['upstream_api_key','embedding_api_key']:
    m=re.search(rf'^\s*{name}\s+([^;\n]+);',s,re.M)
    if not m or not re.search(r'replace|placeholder|example',m.group(1),re.I):
        raise SystemExit(f'{name} must contain a placeholder, not a real key')
PY_CHECK
(
 cd "$SRC"
 tar -czf "$DIST/$ARCHIVE" "${files[@]}"
)
(
 cd "$DIST"
 sha256sum "$ARCHIVE" > "$ARCHIVE.sha256"
 sha256sum -c "$ARCHIVE.sha256"
 tar -tzf "$ARCHIVE"
)
echo "Release assets ready: $DIST/$ARCHIVE and $DIST/$ARCHIVE.sha256"
