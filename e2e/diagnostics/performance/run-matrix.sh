#!/usr/bin/env bash
# Disposable diagnostic only; complete pinned frontends on the same observed native backend.
set -euo pipefail
source .creator/e2e/lib.sh
parse --out "$RUNNER_TEMP/perf-setup" --tensord "$RUNNER_TEMP/candidates/cozy"
source .creator/e2e/cli/commands.sh
source .creator/e2e/cli/project.sh
source .creator/e2e/cli/web.sh
install_cozy
run_project "$OUT/project"
user_services
cozy --json status > "$OUT/status.json"
candidates
cozy --json doctor > "$OUT/doctor.json"
pw=$RUNNER_TEMP/perf-playwright
mkdir -p "$pw"
pnpm --dir "$pw" add --ignore-scripts playwright-core@1.55.0 > "$OUT/playwright-install.log" 2>&1
export HOLDER="$HOME/.cozy/calcifer/calcifer.lock.holder" COZY CHROME
CHROME=$(web_chrome)
export PLAYWRIGHT="$pw/node_modules/playwright-core"
export PROFILE_HARNESS_ROOT="$PWD/.creator" PROFILE_OUTPUT_ROOT="$RUNNER_TEMP/perf-matrix"
export PROFILE_CURRENT_ROOT="$RUNNER_TEMP/frontend-current" PROFILE_BASELINE_ROOT="$RUNNER_TEMP/frontend-baseline"
node .diag/e2e/diagnostics/performance/run-matrix.cjs || check 'diagnostic matrix retained original failures or incomplete lifecycle; see case receipts'
finish mac-scale-performance-diagnostic
