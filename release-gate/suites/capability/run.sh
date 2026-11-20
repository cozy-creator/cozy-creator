#!/usr/bin/env bash
# Suite `capability`: one cell, `long-project-run`. e2e/skew/run.sh --admin --command OUT COZY RUNTIME TENSORFS, signed in to CI's
# throwaway Hub as its admin (a fresh account's grant doesn't cover making a private model: 44662230988). Escaped in 0.26.0-0.27.1 (TensorD hub.rs): a capability's renewal looked its runs up outside their private scope, so it never
# found a project's run on this computer's machine. Redeemed once as the run began, its token was never renewed: past the Hub's 15
# minutes every read under it was refused, a private LoRA's weights mid-render or the upload at its end (the coordinator 11-20, of
# 44636417703: "A private LoRA or model in a local render over 15 minutes failing mid-run is a user-facing loss in the release").
# The cell: a private two-stack model (the tensorfs suite's multi-conversion `source`) uploaded from a project's run, then quantized
# to int8 in that project on `local` by a copy of the package whose recipe waits WAIT_S first, saying so each minute. The run reads
# its private source at once, and its lane's commit goes to the Hub as it ends, past the token's life: refused CREDENTIAL_REQUIRED
# while the renewal can't find the run, landed once it renews it.
set -uo pipefail
OUT=$1 COZY_BIN=$2
HERE=$(cd "$(dirname "$0")/../../.." && pwd)  # the checkout
WAIT_S=1020  # 17 min: past the Hub's 15 min token (internal/authn accessTokenDuration), renewed at two thirds of it
mkdir -p "$OUT" && cd "$OUT" && OUT=$PWD || exit 2
: > expect.tsv
log() { echo "$(date -u +%H:%M:%SZ) $*" | tee -a "$OUT/gate.log"; }
expect() { printf '%s\t%s\t%s\n' "$1" "$2" "$(tr '\n\t\r' '   ' <<< "$3" | cut -c1-400)" >> expect.tsv; log "$1: $2 $3"; }
verdict() {
  local fails passes
  fails=$(grep -c $'\tFAIL\t' expect.tsv) passes=$(grep -c $'\tPASS\t' expect.tsv)
  grep $'\tFAIL\t' expect.tsv | cut -f1,3 | sed 's/^/FAIL /' | tee verdict.txt
  if [ "$fails" = 0 ] && [ "$passes" -gt 0 ]; then echo "VERDICT PASS ($passes cells)" | tee -a verdict.txt; exit 0; fi
  echo "VERDICT FAIL ($fails of $((fails + passes)) cells)" | tee -a verdict.txt; exit 1
}
export PATH=$(dirname "$COZY_BIN"):$PATH
. "$HERE/e2e/test-hub.sh"
require_test_hub "${COZY_HUB_URL:-}" 2> hub-check.err || { expect long-project-run FAIL "not the throwaway Hub ${COZY_HUB_URL:-unset}: $(cat hub-check.err)"; verdict; }
J() { jq -e "$@" > /dev/null; }
account=$(cozy --json whoami | jq -r '.account // empty')
[ -n "$account" ] || { expect long-project-run FAIL "not signed in: $(cozy --json whoami 2>&1 | head -c 300)"; verdict; }
seed=$((RANDOM % 9000 + 1000)) model=$account/capability-long-$((RANDOM % 9000 + 1000))
pkg=$OUT/ci-multi-conversion slow=$OUT/ci-multi-conversion-slow project=$OUT/project
cp -r "$HERE/release-gate/suites/tensorfs/multi-conversion" "$pkg" && cp -r "$pkg" "$slow" || { expect long-project-run FAIL "no copy of the multi-conversion package"; verdict; }
# The slow copy: its recipe waits WAIT_S before it encodes, a minute at a time, each said.
python3 -I - "$slow/ci_multi_conversion/__init__.py" "$WAIT_S" <<'PY' || { expect long-project-run FAIL "the slow copy wasn't written"; verdict; }
import sys
path, wait = sys.argv[1], int(sys.argv[2])
text = open(path).read()
head = "def quantize(ctx: Context, tel: Telemetry, source: Weights[TwoStacks], output: str) -> ModelArtifact:\n"
assert text.count(head) == 1, "no quantize recipe to slow"
text = text.replace(head, head + f"    import time\n    for minute in range({wait} // 60):\n        tel.log(\"waiting past the run capability's token\", minute=minute)\n        time.sleep(60)\n    time.sleep({wait} % 60)\n")
open(path, "w").write(text)
PY
cozy --json new --unencrypted "$project" > new.json 2>&1 || { expect long-project-run FAIL "cozy new: $(tail -c 300 new.json)"; verdict; }
in_project() { (cd "$project" && cozy "$@"); }
# The private bf16 source, made in the project and uploaded from its run.
in_project --json run "$pkg/source" "seed:=$seed" --on local > source.ndjson 2> source.err \
  || { expect long-project-run FAIL "the source run failed: $(tail -c 300 source.ndjson) $(tail -c 200 source.err)"; verdict; }
made=$(grep '^{' source.ndjson | jq -rs '[.. | objects | select(.name? and (.name | test("^[A-Z0-9]+-[0-9]+$")) and .status? == "completed") | .name] | last // empty')
in_project --json model upload "run:$made#model" "$model" --lane bf16 --on local -y > upload.ndjson 2> upload.err \
  || { expect long-project-run FAIL "uploading run $made's model as $model failed: $(tail -c 300 upload.ndjson) $(tail -c 200 upload.err)"; verdict; }
bf16=$(grep '^{' upload.ndjson | jq -rs --arg m "$model" '[.. | objects | select(.published?.destination? == $m) | .published.commit] | last // empty')
[ -n "$bf16" ] || { expect long-project-run FAIL "the upload named no commit of $model: $(tail -c 400 upload.ndjson)"; verdict; }
log "long-project-run: $model@$bf16 (private bf16); its int8 lane in the project on local, the recipe waiting ${WAIT_S}s first"
t0=$SECONDS
in_project --json model quantize "$model@$bf16" --lane int8 --package "$slow" --on local -y > quantize.ndjson 2> quantize.err; code=$?
took=$((SECONDS - t0))
int8=$(grep '^{' quantize.ndjson | jq -rs '[.. | objects | select(.name? == "int8" and (.checkpoint?.revision? | type == "string")) | .checkpoint.revision] | last // empty')
why=$(grep '^{' quantize.ndjson | jq -rs '[.. | objects | select(.reason?.message?) | "\(.reason.code): \(.reason.message)"] | last // empty')
if [ "$code" = 0 ] && [ -n "$int8" ] && [ "$took" -ge "$WAIT_S" ]; then
  expect long-project-run PASS "a project's run on local ${took}s long (past the 15 min token) committed $model@$int8 (int8) to the Hub under its run capability"
else
  expect long-project-run FAIL "a project's run on local ${took}s long: exit $code, no int8 commit: ${why:-$(tail -c 300 quantize.ndjson) $(tail -c 200 quantize.err)}"
fi
cozy --json logout > logout.json 2>&1 || true
verdict
