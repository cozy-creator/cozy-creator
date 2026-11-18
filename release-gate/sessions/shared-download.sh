# shared-download (cl-1205), Paul 11-17, watching kirill fetch MiniMax H3 for three jobs that all need it: "This view is also misleading;
# it looks like we're downloading it 3 times, but we're actually just downloading it once. It should show the download occurring, and
# then it should also show the 3 jobs that are like 'waiting for download...' and they don't need to independently report the progress
# in theirs as well". One rental ($SKU), its TensorD this runner's cozy (`machine update --tensord`, exact-need.sh's way); three boxes of
# `tool` rendered at once, each needing `model`, which the fresh pod doesn't hold. e2e/cli/web/shared-download.spec.js drives the page and
# judges it (one download listed, naming the three runs; each run names it and says no bytes of its own; the tab, drawer and boxes say
# the wait and lead to the download), with screenshots, paper and dark. Meanwhile `cozy run ls` and `cozy machine show` are kept each
# second (run-ls.txt, machine-show.txt), and each run's events (`cozy --json run watch`) with its machine's downloads, as the release
# gate's progress cells keep them: progress/check.py judges them, the RC's and this branch's (progress-rc.txt, progress.txt).
# Without `rent`, $0: the checks, the quote, the guard.
# params:
#   tool    the boxes' tool (default sdxl/image)
#   model   the repository it reads (default stabilityai/stable-diffusion-xl-base-1.0)
#   fields  the boxes' other fields, a JSON object (default {"seed": 7})
. "$(dirname "$0")/lib.sh"
HERE=$(cd "$(dirname "$0")" && pwd)
ROOT=$(cd "$HERE/../.." && pwd)
WEB=$ROOT/e2e/cli/web
TOOL=$(param tool sdxl/image) MODEL=$(param model stabilityai/stable-diffusion-xl-base-1.0) FIELDS=$(param fields '{"seed": 7}')
for tool in jq node npm google-chrome python3; do command -v "$tool" > /dev/null || fail "no $tool"; done
[ -f "$WEB/shared-download.spec.js" ] && [ -f "$ROOT/release-gate/suites/progress/check.py" ] || fail "no shared-download.spec.js or progress/check.py"
jq -e 'type == "object"' <<<"$FIELDS" > /dev/null || fail "fields $FIELDS"
PW=$OUT/playwright-test
npm install -s --prefix "$PW" @playwright/test@1.55.0 > "$OUT/npm.log" 2>&1 || fail "npm install @playwright/test: $(tail -c 300 "$OUT/npm.log")"
V=$(cozy --version | awk '{print $2}')
quote 1800
rent
step update machine update "$NAME" --tensord "$(command -v cozy)" -y || fail "update"
v=$(cozy machine show "$NAME" --json | jq -r .status.version)
[ "$v" = "$V" ] || fail "the pod runs TensorD $v, not the session's $V"
say "$NAME runs this runner's TensorD $v; $MODEL on it: $(cozy --json model ls --on "$NAME" 2> /dev/null | jq -r --arg m "$MODEL" '[.models[]? | select(.model == $m)] | length') release(s)"

# What a person sees each second while the boxes render: the runs (as the spec's project names them) and the machine.
G=$OUT/gate; mkdir -p "$G"; : > "$G/cells.tsv"
looks() {
  local p=''
  while :; do
    [ -n "$p" ] || p=$(ls -d "$OUT"/web/projects/*shared-download* 2> /dev/null | head -1)
    { date -u +%T; [ -z "$p" ] || (cd "$p" && cozy run ls 2>&1); } >> "$OUT/run-ls.txt"
    { date -u +%T; cozy machine show "$NAME" 2>&1; } >> "$OUT/machine-show.txt"
    cozy --json machine show "$NAME" 2> /dev/null | jq -c --argjson at "$(date +%s%3N)" 'select((.status.activity // []) | length > 0)
      | {at: $at, activity: .status.activity}' >> "$G/machine.ndjson"
    sleep 1
  done
}
looks & BG="$BG $!"
SHARED_MACHINE=$NAME SHARED_TOOL=$TOOL SHARED_MODEL=$MODEL SHARED_FIELDS=$FIELDS FLOW_SPEC=shared-download.spec.js \
  HOLDER=$HOME/.cozy/calcifer/calcifer.lock.holder COZY=$(command -v cozy) CHROME=$(command -v google-chrome) OUT=$OUT \
  NODE_PATH=$PW/node_modules node "$PW/node_modules/@playwright/test/cli.js" test -c "$WEB/flow.config.js" > "$OUT/flow.log" 2>&1 & node=$!
BG="$BG $node"
# Each run's events as the progress cells keep them, once the spec's project has its runs.
followed=''
while kill -0 "$node" 2> /dev/null; do
  p=$(ls -d "$OUT"/web/projects/*shared-download* 2> /dev/null | head -1)
  if [ -n "$p" ]; then
    for run in $( (cd "$p" && cozy --json run ls 2> /dev/null) | jq -r '.runs[]? | select(.kind == "call") | .name'); do
      [[ " $followed " != *" $run "* ]] || continue
      followed="$followed $run"
      (cd "$p" && cozy --json run watch "$run" > "$G/$run-cold.ndjson" 2>&1) &
      printf '%s\t%s\t%s\n' "$run-cold" "$TOOL" "$run" >> "$G/cells.tsv"
    done
  fi
  sleep 2
done
wait "$node"; code=$?
for cell in $(cut -f1 "$G/cells.tsv"); do cp "$G/machine.ndjson" "$G/$cell.machine.ndjson" 2> /dev/null; done
tail -40 "$OUT/flow.log" | cut -c1-300 | sed 's/^/    /' >> "$R"
[ "$code" = 0 ] || { RC=1; say "the spec failed ($code): flow.log"; }
# The progress cells' verdict on these runs: this branch's judge, and the RC's (its judge and its page's stage map, as the RC has them).
python3 -I "$ROOT/release-gate/suites/progress/check.py" "$G" "$SKU" > "$OUT/progress.txt" 2>&1; green=$?
RCT=$OUT/rc-judge
if git -C "$ROOT" fetch -q --depth=1 origin rc/0.27.0 2> "$OUT/rc-fetch.err"; then
  for f in release-gate/suites/progress/check.py release-gate/runs.py e2e/cli/stages.py web/app/src/panels/runs/stages.json; do
    mkdir -p "$RCT/$(dirname "$f")" && git -C "$ROOT" show "FETCH_HEAD:$f" > "$RCT/$f" || say "the RC's $f wasn't read"
  done
  python3 -I "$RCT/release-gate/suites/progress/check.py" "$G" "$SKU" > "$OUT/progress-rc.txt" 2>&1
fi
say "progress check, this branch's: $(tail -1 "$OUT/progress.txt"); the RC's: $(tail -1 "$OUT/progress-rc.txt" 2> /dev/null || echo "not read: $(head -c 200 "$OUT/rc-fetch.err")")"
grep -E '^(FAIL|note)' "$OUT/progress.txt" | sed 's/^/  /' | tee -a "$R"
[ "$green" = 0 ] || RC=1
say "shared-download: $([ "$RC" = 0 ] && echo PASS || echo FAIL)"
exit "$RC"
