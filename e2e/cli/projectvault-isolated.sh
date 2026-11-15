#!/usr/bin/env bash
# e2e/skew/run.sh --command: OUT INSTALLED_COZY RUNTIME TENSORFS.
# Qualifies the exact supplied archive binary; installer scripts have their own gate.
set -euo pipefail
[ "$#" = 4 ] || { echo "usage: projectvault-isolated.sh OUT INSTALLED_COZY RUNTIME TENSORFS" >&2; exit 2; }
[ "${GITHUB_ACTIONS:-}" = true ] && [ "${RUNNER_OS:-}" = Linux ] \
  || { echo "the vault archive suite requires a fresh Linux GitHub runner" >&2; exit 2; }
vault_gate_out=$1 vault_gate_binary=$2 vault_gate_runtime=$3 vault_gate_tensorfs=$4
[ -n "$vault_gate_runtime" ] && [ -n "$vault_gate_tensorfs" ] \
  || { echo "the vault archive suite requires the exact Runtime and TensorFS pair" >&2; exit 2; }
[ "$(realpath "$vault_gate_binary")" = "$(realpath "$HOME/.cozy/bin/cozy")" ] \
  || { echo "the fixture binary must be installed in the default home" >&2; exit 2; }
. "$(dirname "$0")/../lib.sh"
parse --out "$vault_gate_out" --installed --tensord "$vault_gate_binary" \
  --runtime "$vault_gate_runtime" --tensorfs "$vault_gate_tensorfs"
. "$E2E/run.sh"
. "$E2E/cli/project.sh"
. "$E2E/cli/projectvault.sh"
RUN_CLI=$COZY
export RUN_CLI COZY
collect_vault_logs() {
  local status=$?
  set +e
  mkdir -p "$OUT/cozy-logs"
  (cd "$HOME/.cozy" && find . -name '*.log' -size -20M | tar -cf - -T - | tar -xf - -C "$OUT/cozy-logs")
  return "$status"
}
trap collect_vault_logs EXIT
user_services
run_project "$OUT/project" || fail "project initialization failed"
# Set the exact SDK pair on first machine startup, before taking its baseline report.
candidates
cozy --json doctor > "$OUT/doctor.json"
jq -n --arg binary "$(cozy --version)" --arg sha256 "$(sha256sum "$COZY" | cut -d ' ' -f 1)" \
  --arg runtime "$vault_gate_runtime" --arg tensorfs "$vault_gate_tensorfs" \
  --arg hub "$COZY_HUB_URL" --arg expected "$COZY_TEST_HUB_ORIGIN" \
  '{cozy:$binary,sha256:$sha256,requested_runtime:$runtime,requested_tensorfs:$tensorfs,hub:$hub,expected_test_origin:$expected}' > "$OUT/cohort.json"
step "encrypted project backend"
projectvault
vault_leftovers
finish "isolated projectvault"
