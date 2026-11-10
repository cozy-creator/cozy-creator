#!/usr/bin/env bash
# Disposable diagnostic: same 5000-asset case, no preceding broad-suite warmup.
set -euo pipefail
source .creator/e2e/lib.sh
parse --out "$RUNNER_TEMP/scale-diag" --tensord "$RUNNER_TEMP/candidates/cozy"
source .creator/e2e/cli/commands.sh
source .creator/e2e/cli/project.sh
source .creator/e2e/cli/web.sh
install_cozy
run_project "$OUT/project"
user_services
cozy --json status > "$OUT/status.json"
candidates
cozy --json doctor > "$OUT/doctor.json"
pw=$OUT/playwright
mkdir -p "$pw"
pnpm --dir "$pw" add --ignore-scripts playwright-core@1.55.0 > "$OUT/playwright-install.log" 2>&1
cp "$RUNNER_TEMP/diagnostic-provenance.json" "$OUT/diagnostic-provenance.json"
web_proof scale "$(web_chrome)" "$pw" ''
finish mac-scale-diagnostic
