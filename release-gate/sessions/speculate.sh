# speculate (cl-1123 D, Paul 11-17: "or even start running the run, as if the current stuff entered into the text-box were the final
# request, and cancel it and re-submit it if the input changes"): one rental, a Qwen Image 2.1 box this runner's Calcifer predicts onto
# it, its default request (Turbo and prompt upsampling on). Once loaded and its draft still 3 s, the machine runs it ahead; Status says
# `running` n/m, then `done`. Each step is checked in the machine's own log and the runs' records:
#   1. done:      the box's render adopts the finished run: its take lands about as fast as its file does.
#   2. edit:      the box edited while its draft runs: that run ends ("its draft changed") and the new draft runs.
#   3. in flight: the box rendered while its draft runs: the render adopts the run going.
#   4. preempt:   another model's call (SDXL) sent while a draft runs: it gets the GPU at once (under 50 ms), the run told to stop.
#   5. no trace:  the project lists only the session's own five runs, and no speculative run moved the idle deadline.
# The pod runs the session's TensorD, Runtime and TensorFS, checked before every step (load-ahead.sh). params:
#   tree     a packages_refs checkout whose qwen-image-2 runs, its rules with it (`-f packages_refs=pk=<ref>`, tree pk): a dev home
#   update   `cozy machine update` flags beside its TensorD: a dev build's Runtime and TensorFS, `artifact:<run id>:<artifact>[:<repo>]`
#   other    the call that takes the GPU during a draft's run (default sdxl/generate)
# Weights come from the Hub: `floor_mb_s: 0` keeps a slow link from ending it. Each render takes its box's draft seed, so the box's
# next draft (another seed) runs ahead at once: each edit waits for the new draft `loaded`, then `running`.
. "$(dirname "$0")/lib.sh"
TOOL=qwen-image-2/image OTHER=$(param other sdxl/generate) TREE=$(param tree '') BEGAN=$(date +%s)
PROMPT="a red lighthouse on a cliff at dusk, waves below, photo"
fetched() {
  IFS=: read -r _ rid name repo <<<"$1"; mkdir -p "$2"
  gh run download "$rid" -R "cozy-creator/${repo:-cozy-creator}" -n "$name" -D "$2" > /dev/null 2>&1 || return 1
  find "$2" -type f | grep -m1 -E "$3"
}
mapfile -t UPDATE < <(param update '[]' | jq -r '.[]')
for j in "${!UPDATE[@]}"; do
  v=${UPDATE[$j]}; [[ $v == artifact:* ]] || continue
  UPDATE[$j]=$(fetched "$v" "$OUT/update/$j" 'manylinux.*x86_64.*\.whl$') || fail "$v has no Linux x86_64 wheel"
done
if [ -n "$TREE" ]; then
  [ -f "$PACKAGES/$TREE/qwen-image-2/pyproject.toml" ] || fail "no qwen-image-2 in the packages checkout $TREE"
  SOURCE=$(cd "$PACKAGES/$TREE" && pwd)
  cfg=$HOME/.cozy/config.toml
  mkdir -p "${cfg%/*}" && touch "$cfg" || fail "no $cfg"
  { echo 'dev = true'; grep -v '^dev *=' "$cfg" || true; } > "$cfg.new" && mv "$cfg.new" "$cfg" || fail "dev = true not written"
  awk -v source="$SOURCE" '
    function ours() { print "diagnostics = true"; print "packages = \"" source "\""; put = 1 }
    /^(diagnostics|packages) *=/ { next } { print } /^\[calcifer\]/ { ours() } END { if (!put) { printf "\n[calcifer]\n"; ours() } }' \
    "$cfg" > "$cfg.next" && mv "$cfg.next" "$cfg" || fail "[calcifer] not written"
  cozy restart > "$OUT/restart.txt" 2>&1 || fail "cozy restart: $(tail -c 300 "$OUT/restart.txt")"
  kernels "$SOURCE/qwen-image-2"
fi
quote "$(param need_s 3000)"
rent
step tensord machine update "$NAME" --tensord "$(command -v cozy)" "${UPDATE[@]}" -y || fail "the pod refused the session's software"
WANT=$(cozy --version | awk '{print $2}')
read -r WANT_RT WANT_FS < <(cozy machine show "$NAME" --json | jq -r '"\(.status.runtime // "") \(.status.tensorfs // "")"')
ours() {
  local v t
  for t in 1 2 3; do
    v=$(cozy machine show "$NAME" --json 2> /dev/null | jq -r '"\(.status.version // "") \(.status.runtime // "") \(.status.tensorfs // "")"')
    [ "$v" != "$WANT $WANT_RT $WANT_FS" ] || { echo "$(date -u +%T) $1 $v" >> "$OUT/versions.txt"; return 0; }
    say "$1: the pod runs ${v:-?}, not the session's $WANT $WANT_RT $WANT_FS (try $t): updated again"
    step "update-$1-$t" machine update "$NAME" --tensord "$(command -v cozy)" "${UPDATE[@]}" -y
  done
  fail "$1: the pod kept moving off the session's software (${v:-?})"
}
ours start
say "pod TensorD $WANT, Runtime $WANT_RT, TensorFS $WANT_FS"
( while :; do
    printf '{"at_ms":%s,"status":%s}\n' "$(date +%s%3N)" \
      "$(cozy machine show "$NAME" --json 2> /dev/null | jq -c '{ahead: (.status.ahead // []), idle_deadline_ms: .status.idle_deadline_ms}' 2> /dev/null || echo null)" >> "$OUT/status.ndjson"
    sleep 1
  done ) & BG=$!
collect() {
  cozy machine logs "$NAME" --lines 0 --since "$(( ($(date +%s) - BEGAN) / 60 + 2 ))m" > "$OUT/tensord.log" 2>&1 || say "the pod's TensorD log wasn't read"
  cozy machine logs local calcifer --lines 20000 > "$OUT/calcifer.log" 2>&1 || true
}
# item: the box's guess now, "<its prepare item> <its stage>" ("" for none).
item() { tail -1 "$OUT/status.ndjson" | jq -r '[.status.ahead[]? | select(.id | startswith("cozy:prepare:")) | "\(.id) \(.readiness.stage // "")"] | first // ""'; }
# until_stage <stages> <s>: within <s> s, the box's guess is at one of <stages> (a regex: 44175857630's went from loading to running
# between two looks), and is not $WAS, the item it was before its last edit or render.
WAS='' ITEM=''
until_stage() {
  local t=0 now=''
  until now=$(item); [ -n "$now" ] && [ "${now%% *}" != "$WAS" ] && [[ ${now#* } =~ ^($1)$ ]]; do
    sleep 1; t=$((t + 1))
    [ "$t" -lt "$2" ] || { collect; fail "the box's draft wasn't $1 within $2 s: $(tail -1 "$OUT/status.ndjson" | cut -c1-500)"; }
  done
  ITEM=${now%% *}
  say "the box's draft is ${now#* } ($t s; ${ITEM#cozy:prepare:})"
}
# mark <what>: a person's action (marks.txt): the idle deadline may move only at one.
mark() { echo "$(date +%s%3N) $1" >> "$OUT/marks.txt"; }
# render <arm>: the box rendered as the page renders it; its take landed, timed from the ask.
render() {
  local asked; asked=$(date +%s%3N); mark "render $1"
  python3 "$RUN_PAGE" render "$RUN_PROJECT" --on "$NAME" q > "$OUT/render-$1.txt" 2>&1 || fail "$1: the render: $(tail -c 400 "$OUT/render-$1.txt")"
  mark "render $1 landed"
  say "$1: the render's take landed $(( $(date +%s%3N) - asked )) ms after Render"
  # Its draft's seed is taken, so the box is guessed anew: that item, whatever its stage.
  WAS=$ITEM; until_stage '.*' 60
}
edit() { WAS=$ITEM; mark edit; run_page set q --fields "$(jq -nc --arg p "$1" '{instructions: $p}')" > "$OUT/edit.json" 2>&1 || fail "the edit: $(tail -c 300 "$OUT/edit.json")"; say "q edited: $1"; }

# The weights onto the pod (the default request reads the model, its text encoder, expander_t2i and Turbo's LoRA), then SDXL's.
ours fetch
step fetch run "$TOOL" --on "$NAME" instructions="$PROMPT" megapixels:=1 aspect_ratio=1:1 seed:=1 || fail "the first call"
ours other-fetch
step other-fetch run "$OTHER" --on "$NAME" prompt="a brass compass on a walnut desk" seed:=4 || fail "the other call's fetch"
step restart machine update "$NAME" --mode dedicated -y || fail "the restart"
[ "$(cozy machine show "$NAME" --json | jq -r .status.phase)" = ready ] || fail "$NAME isn't ready after its restart"
ours shown
python3 "$RUN_PAGE" show "$RUN_PROJECT" > "$OUT/shown.txt" 2>&1 & BG="$BG $!"
for _ in $(seq 1 30); do grep -q '"held"' "$OUT/shown.txt" && break; sleep 1; done
grep -q '"held": \["' "$OUT/shown.txt" || fail "the project wasn't shown: $(tail -c 300 "$OUT/shown.txt")"

# 1. done: drafted, loaded, run ahead to its end; then Render.
mark draft
run_page add q --tool "$TOOL" --fields "$(jq -nc --arg p "$PROMPT" '{instructions: $p, megapixels: 1, aspect_ratio: "1:1", seed: 3}')" \
  > "$OUT/box.json" 2>&1 || fail "the box: $(tail -c 300 "$OUT/box.json")"
say "box q drafted (Turbo and upsampling on)"
until_stage 'loaded|running|done' 600
until_stage 'running|done' 60
until_stage done 300
ours done
render done

# 2. edit: a new draft runs; edited again while it runs, it ends and the newer runs.
edit "a green lighthouse on a cliff at dawn, mist below, photo"
until_stage 'loaded|running' 120
until_stage running 60
edit "a blue lighthouse in a storm at night, waves breaking, photo"
until_stage 'loaded|running' 120
until_stage running 60

# 3. in flight: rendered while that draft runs.
ours in-flight
render in-flight

# 4. preempt: another draft runs; SDXL sent meanwhile.
edit "a white lighthouse at noon, calm sea, gulls, photo"
until_stage 'loaded|running' 120
until_stage running 60
ours preempt
mark preempt
step preempt run "$OTHER" --on "$NAME" prompt="a brass compass on a walnut desk" seed:=5 || fail "the other call"
mark "preempt done"
ours end
kill $BG 2> /dev/null; BG=''

collect
sed 's/^/version before /' "$OUT/versions.txt" >> "$R"
grep -aE "runs as if it were sent|speculative run|adopts|differs from item|queue: goes after|queue: waiting for a free GPU" "$OUT/tensord.log" | cut -c1-320 | tee -a "$R" > "$OUT/speculate.log"
# The SDXL call's wait for the GPU, from its queue's two lines.
grep -q "adopts the speculative run .*Completed\|adopts the speculative run .*completed" "$OUT/speculate.log" || { say "FAIL: no render adopted a finished run"; RC=1; }
grep -q "adopts the speculative run .*\(running\|starting\|Running\|Starting\)" "$OUT/speculate.log" || { say "FAIL: no render adopted a run going"; RC=1; }
grep -q "speculative run ended for its draft changed" "$OUT/speculate.log" || { say "FAIL: no edit ended a run"; RC=1; }
grep -q "speculative run ended for a run" "$OUT/speculate.log" || { say "FAIL: SDXL didn't end a run"; RC=1; }
python3 - "$OUT/speculate.log" <<'PY' | tee -a "$R"
import re, sys
from datetime import datetime
lines = open(sys.argv[1]).read().splitlines()
def at(line): return datetime.fromisoformat(line.split()[0].rstrip("Z")).timestamp()
ended = [l for l in lines if "speculative run ended for a run" in l]
waits = [l for l in lines if "queue: waiting for a free GPU" in l]
goes = [l for l in lines if "queue: goes after" in l and "free GPU" in l]
if ended and waits:
    w = [l for l in waits if at(l) <= at(ended[-1]) + 1][-1:] or waits[-1:]
    g = [l for l in goes if at(l) >= at(w[0])][:1]
    print(f"preempt: the call waited for a free GPU from {w[0].split()[0]}; the run ended {1000 * (at(ended[-1]) - at(w[0])):.1f} ms later"
          + (f"; the call went {1000 * (at(g[0]) - at(w[0])):.1f} ms after it waited" if g else ""))
PY
# 5. No trace: fetch, other-fetch, two renders and the preempting call are the project's runs; while a draft ran ahead or stood
# done, the idle deadline stayed where the last action left it.
(cd "$RUN_PROJECT" && cozy --json run ls --all --limit 40) > "$OUT/runs.json" 2>&1
say "the project's runs: $(jq -c '[.runs[]? | {number, target}]' "$OUT/runs.json" | cut -c1-400)"
[ "$(jq '[.runs[]?] | length' "$OUT/runs.json")" = 5 ] || { say "FAIL: the project lists other than its 5 runs"; RC=1; }
! grep -q speculative "$OUT/runs.json" || { say "FAIL: a speculative run is in the project"; RC=1; }
python3 - "$OUT" <<'PY' | tee -a "$R" | grep -q '^idle: held' || { say "FAIL: speculation moved the idle deadline"; RC=1; }
import json, sys
out = sys.argv[1]
marks = sorted(int(line.split()[0]) for line in open(f"{out}/marks.txt"))
rows = [json.loads(line) for line in open(f"{out}/status.ndjson") if line.strip().endswith("}")]
rows = [row for row in rows if row.get("status")]
def stage(row):
    return next(((a.get("readiness") or {}).get("stage") for a in row["status"]["ahead"] if a.get("id", "").startswith("cozy:prepare:")), None)
moved, spans, held, busy = [], 0, None, 0
for row in rows:
    since = max([m for m in marks if m <= row["at_ms"]], default=0)
    # From 2 s after an action (its own call or list put landed) until the next one. None is a moment's admission, or a job.
    if stage(row) not in ("running", "done") or row["at_ms"] < since + 2000:
        held, busy = None, 0
        continue
    deadline = row["status"].get("idle_deadline_ms")
    if held is None or held[0] != since:
        held, busy, spans = (since, None), 0, spans + 1
    busy = busy + 1 if deadline is None else 0
    if busy == 3:
        moved.append((row["at_ms"], held[1], "held by a job"))
    if deadline is not None and held[1] is None:
        held = (since, deadline)
    elif deadline is not None and deadline != held[1]:
        moved.append((row["at_ms"], held[1], deadline))
print(f"idle: {'held' if not moved and spans else 'MOVED'} across {spans} spans of running ahead" + (f"; moved {moved[:3]}" if moved else ""))
PY
[ "$RC" = 0 ] && say "PASS: run ahead, adopted done and in flight, ended by an edit, and by another model's call; no trace, idle held"
