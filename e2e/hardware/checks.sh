# Sourced by the existing skew fixture, after it grants its private account admin.
hardware_checks() (
  set -euo pipefail
  . "$E2E/test-hub.sh"
  require_test_hub "${COZY_HUB_URL:?}"
  local proof=$OUT/hardware package=ci-hardware-$R fixture
  fixture=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)/probe
  mkdir -p "$proof"
  cp -r "$fixture" "$proof/$package"
  sed -i "s/^name = \"ci-hardware-probe\"/name = \"$package\"/" "$proof/$package/pyproject.toml"
  # This catalog is created only through the verified fixture, never copied from production.
  (cd "$proof/$package" && cozy --json package publish) > "$proof/publish.json"
  cozy --json package install "$WHO/$package@0.1.0" --on local > "$proof/install.json"
  run_cell "$proof" cpu "$WHO/$package/cpu" value:=19 --on local
  run_cell "$proof" gpu "$WHO/$package/gpu" value:=19 --on local
  python3 "$(dirname "$fixture")/verify.py" "$proof"
  cozy --json package delete "$WHO/$package" -y > "$proof/delete.json"
)
hardware_checks
hardware_status=$?
if [ "$hardware_status" = 0 ]; then expect "hardware CUDA" PASS "published fixture package, CPU and real CUDA arithmetic/output verified"
else expect "hardware CUDA" FAIL "the fixture CUDA probe failed; see hardware receipts"; fi
