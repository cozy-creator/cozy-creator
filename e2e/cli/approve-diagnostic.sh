#!/usr/bin/env bash
# A single Windows approval observation, using the ordinary CLI install/candidate and web fixtures.
. "$(dirname "$0")/../lib.sh"
parse "$@"
. "$(dirname "$0")/web.sh"
[ "$RUNNER_OS" = Windows ] || fail "this diagnostic requires the Windows CI runner"

cleanup() {
  local result=$?
  mkdir -p "$OUT/cozy-logs"
  (cd ~/.cozy && find . -name '*.log' -size -20M | tar -cf - -T - | (cd "$OUT/cozy-logs" && tar -xf -)) || true
  remove_cozy
  hub_checked
  return "$result"
}
trap cleanup EXIT

step install
install_cozy
run_project "$OUT/project" || fail "cozy project new failed"
step status
cozy --json status > "$OUT/status.json"
step candidates
candidates
step doctor
cozy --json doctor > "$OUT/doctor.json" || check "cozy doctor is not healthy"

step "web-live approval diagnostic"
chrome=$(web_chrome)
[ -x "$chrome" ] || fail "no Google Chrome on this runner"
pw=$OUT/playwright
npm install -s --prefix "$(web_path "$pw")" playwright-core@1.55.0 > "$OUT/web-npm.log" 2>&1 || fail "Playwright installation failed"
web_proof approval-diagnostic "$chrome" "$pw" ''
jq -c '{failures, bounds: .seen.approvalBounds}' "$OUT/web-approval-diagnostic.json"
# The EXIT trap keeps diagnostic evidence and removes the runner-local install.
[ ! -s "$OUT/failures.txt" ]
