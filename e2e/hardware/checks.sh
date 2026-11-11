#!/usr/bin/env bash
# e2e/skew/run.sh --admin --command: OUT INSTALLED_COZY RUNTIME TENSORFS.
set -euo pipefail
[ "$#" = 4 ] || { echo 'usage: checks.sh OUT COZY RUNTIME TENSORFS' >&2; exit 2; }
OUT=$1 COZY=$2 RUNTIME=$3 TENSORFS=$4
E2E=$(cd "$(dirname "$0")/.." && pwd)
. "$E2E/test-hub.sh"
require_test_hub "${COZY_HUB_URL:?}"
mkdir -p "$OUT"
cozy() { "$COZY" "$@"; }
log() { echo "$(date -u +%H:%M:%SZ) hardware: $*"; }
WHO=$(cozy --json whoami | python3 -c 'import json,sys; print(json.load(sys.stdin)["account"])')
package=ci-hardware-$(date +%s)-$RANDOM
fixture=$E2E/hardware/probe
cp -r "$fixture" "$OUT/$package"
sed -i "s/^name = \"ci-hardware-probe\"/name = \"$package\"/" "$OUT/$package/pyproject.toml"
RUN_CLI=$COZY
. "$E2E/run.sh"
run_project "$OUT/project"
log "installing the exact Runtime/TensorFS candidates"
cozy --json machine update local --runtime "$RUNTIME" --tensorfs "$TENSORFS" -y | tee "$OUT/candidate-update.ndjson"
log "publishing and installing the owned probe at the verified fixture"
(cd "$OUT/$package" && cozy --json package publish) > "$OUT/publish.json"
cozy --json package install "$WHO/$package@0.1.0" --on local | tee "$OUT/install.ndjson"
log 'running CPU arithmetic'
run_cell "$OUT" cpu "$WHO/$package/cpu" value:=19 --on local
log 'running physical CUDA arithmetic'
run_cell "$OUT" gpu "$WHO/$package/gpu" value:=19 --on local
log 'running separate operator SDK qualification in the exact installed environment'
operator_python=$(python3 - "$COZY_MACHINE_ROOT" <<'PY'
from pathlib import Path
import sys
root = Path(sys.argv[1]) / 'var/lib/cozy/tensord/generations'
interpreters = sorted(path for path in root.glob('*/env/bin/python') if path.is_file())
assert len(interpreters) == 1, f'expected the sole owned probe environment, got {interpreters}'
print(interpreters[0])
PY
)
sdk_sha=$(jq -r 'select(.type == "run.completed") | .run.result.to.tensorfs_sha256 // empty' "$OUT/candidate-update.ndjson" | tail -1)
"$operator_python" -I "$E2E/hardware/operator_sdk.py" --wheel "$TENSORFS" --expected-sha "$sdk_sha" \
  --out "$OUT/operator-sdk" --gpu-uuid "${CUDA_VISIBLE_DEVICES:?}"
python3 "$E2E/hardware/verify.py" "$OUT" "$RUNTIME" "$TENSORFS"
cozy --json package delete "$WHO/$package" -y > "$OUT/delete.json"
log 'PASS exact candidate CLI/Runtime arithmetic and separate native SDK CUDA qualification'
