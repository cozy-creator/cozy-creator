# load-ahead (cl-1123 C, Paul 11-17: "if the target machine GPU is idle, like load the model weights into memory"): one rental,
# and a Qwen Image 2.1 box this runner's Calcifer predicts onto it (`PUT /v1/prepare`). Its machine reports the box's request
# `loaded` in its Status; the box's render then takes the executor the load left, and its run's own report has no executor start,
# construction or loading stage and reads nothing from the store in its first step, beside a cold call of the same function on the
# same machine. Then a load is made to yield: the executors ended, the box's request loads again, and a different call (SDXL) sent
# while it loads goes at once; TensorD's log says how long the GPU took to come back. The idle clock is read before and after a
# load: a load never moves it. Each arm starts with TensorD restarted (`cozy machine update --mode dedicated`, the mode it has: no
# verb ends an executor alone), which ends every executor; its disk stays, and Calcifer sends its lists again to the service that
# started. params:
#   tool       the box's tool and the function the cold call runs (default qwen-image-2/image)
#   turbo, upsampling  the request's Turbo and prompt enhancer (default false: none of its on-demand models). True: the load builds
#              each one the request's rules say it reads, and its first step builds nothing (cl-1123 C)
#   tree       a packages_refs checkout whose qwen-image-2 runs, its rules with it (`-f packages_refs=pk=<ref>`, tree pk): this computer
#              a dev home, its first-party packages from the checkout, as needs-ahead's is ("": the Hub's release)
#   update     `cozy machine update` flags for the pod beside its TensorD: a dev build's Runtime and TensorFS, each
#              `artifact:<run id>:<artifact>[:<repo>]`, fetched before renting ([]: its image's)
#   yield      false: no SDXL arm (default true)
#   other      the call that takes the GPU during a load (default sdxl/generate)
#   wait_s     how long the box's request may take to be `loaded` (default 600)
# Its other call's weights come from the Hub: `floor_mb_s: 0` keeps a slow link from ending it (44049593387: SDXL at 25 MB/s).
. "$(dirname "$0")/lib.sh"
TOOL=$(param tool qwen-image-2/image) OTHER=$(param other sdxl/generate) WAIT=$(param wait_s 600) BEGAN=$(date +%s)
TURBO=$(param turbo false) UPSAMPLING=$(param upsampling false) TREE=$(param tree '') YIELD=$(param yield true)
[[ $TURBO =~ ^(true|false)$ && $UPSAMPLING =~ ^(true|false)$ && $YIELD =~ ^(true|false)$ ]] || fail "turbo, upsampling and yield are true or false"
PROMPT="a red lighthouse on a cliff at dusk, waves below, photo"
FIELDS="turbo:=$TURBO prompt_upsampling:=$UPSAMPLING megapixels:=1 aspect_ratio=1:1"
# artifact:<run id>:<artifact>[:<repo>] fetched into <dir>: the file in it matching <pattern>.
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
  # A dev home, its first-party packages from the checkout; Calcifer started again on it (needs-ahead.sh).
  cfg=$HOME/.cozy/config.toml
  mkdir -p "${cfg%/*}" && touch "$cfg" || fail "no $cfg"
  { echo 'dev = true'; grep -v '^dev *=' "$cfg" || true; } > "$cfg.new" && mv "$cfg.new" "$cfg" || fail "dev = true not written"
  awk -v source="$SOURCE" '
    function ours() { print "diagnostics = true"; print "packages = \"" source "\""; put = 1 }
    /^(diagnostics|packages) *=/ { next } { print } /^\[calcifer\]/ { ours() } END { if (!put) { printf "\n[calcifer]\n"; ours() } }' \
    "$cfg" > "$cfg.next" && mv "$cfg.next" "$cfg" || fail "[calcifer] not written"
  cozy restart > "$OUT/restart.txt" 2>&1 || fail "cozy restart: $(tail -c 300 "$OUT/restart.txt")"
  kernels "$SOURCE/qwen-image-2"
  say "a dev home, qwen-image-2 $(grep -m1 '^version' "$SOURCE/qwen-image-2/pyproject.toml") from the checkout $TREE"
fi
quote "$(param need_s 3600)"
rent
step tensord machine update "$NAME" --tensord "$(command -v cozy)" "${UPDATE[@]}" -y || fail "the pod refused the session's software"
WANT=$(cozy --version | awk '{print $2}')
read -r WANT_RT WANT_FS < <(cozy machine show "$NAME" --json | jq -r '"\(.status.runtime // "") \(.status.tensorfs // "")"')
# ours <step>: the pod runs the session's TensorD, Runtime and TensorFS before <step> (versions.txt); one moved off them is updated
# again, three times at most (A/B's rentals were moved back to the open RC's TensorD mid-session).
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

# The pod's Status as Calcifer relays it, each second: each predicted request's readiness and the idle clock.
( while :; do
    printf '{"at_ms":%s,"status":%s}\n' "$(date +%s%3N)" \
      "$(cozy machine show "$NAME" --json 2> /dev/null | jq -c '{ahead: (.status.ahead // []), idle_deadline_ms: .status.idle_deadline_ms}' 2> /dev/null || echo null)" >> "$OUT/status.ndjson"
    sleep 1
  done ) & BG=$!
# stage: the newest readiness stage the pod says for any predicted request ("" for none).
stage() { tail -1 "$OUT/status.ndjson" | jq -r '[.status.ahead[]? | .readiness.stage // empty] | first // ""'; }
# collect: the pod's TensorD log and this runner's Calcifer's, since the session began.
collect() {
  cozy machine logs "$NAME" --lines 0 --since "$(( ($(date +%s) - BEGAN) / 60 + 2 ))m" > "$OUT/tensord.log" 2>&1 || say "the pod's TensorD log wasn't read"
  cozy machine logs local calcifer --lines 20000 > "$OUT/calcifer.log" 2>&1 || true
}
# until <stage> <seconds>: the pod says it, else fail.
until_stage() {
  local t=0
  until [ "$(stage)" = "$1" ]; do
    sleep 1; t=$((t + 1))
    [ "$t" -lt "$2" ] || { collect; fail "no predicted request was $1 within $2 s: $(tail -1 "$OUT/status.ndjson" | cut -c1-400)"; }
  done
  say "a predicted request is $1 ($t s)"
}
# unload <name>: TensorD restarted, every executor with it; its disk stays.
unload() {
  step "restart-$1" machine update "$NAME" --mode dedicated -y || fail "the restart: $(tail -c 300 "$OUT/restart-$1.txt")"
  [ "$(cozy machine show "$NAME" --json | jq -r .status.phase)" = ready ] || fail "$NAME isn't ready after its restart"
  ours "restart-$1"
}
# report <name> <run>: the run's own record, and its report's stages and first steps.
report() {
  (cd "$RUN_PROJECT" && cozy --json run watch "$2" --once --full) > "$OUT/run-$1.json" 2>&1 || fail "$1: run $2 unread"
  jq -c --arg arm "$1" '{arm: $arm, run: .number, status, execution_ms, accepted_to_started_ms: (.started_at_ms - .accepted_at_ms),
    stages: [.report.measured[0].stages[]? | {(.name): (.ms | floor)}] | add}' \
    "$OUT/run-$1.json" | tee -a "$R" > "$OUT/report-$1.json"
}

# Its weights onto the pod by one call of it, which downloads them: every arm after reads them from its disk.
ours fetch
step fetch run "$TOOL" --on "$NAME" instructions="$PROMPT" $FIELDS seed:=1 || fail "the first call"
unload cold
# The cold call: its weights on disk, no executor.
ours cold
step cold run "$TOOL" --on "$NAME" instructions="$PROMPT" $FIELDS seed:=2 || fail "the cold call"
report cold "$(runno cold)"
unload predicted

# The prediction: the project open, as a page shows it (Calcifer guesses only for a project a page shows), and a box with the
# tool put on its canvas. Calcifer guesses its render onto the pod.
python3 "$RUN_PAGE" show "$RUN_PROJECT" > "$OUT/shown.txt" 2>&1 & BG="$BG $!"
for _ in $(seq 1 30); do grep -q '"held"' "$OUT/shown.txt" && break; sleep 1; done
grep -q '"held": \["' "$OUT/shown.txt" || fail "the project wasn't shown: $(tail -c 300 "$OUT/shown.txt")"
say "the project is shown, as an open page shows it: $(head -c 200 "$OUT/shown.txt")"
ours predict
ASK=$(date +%s%3N)
run_page add q --tool "$TOOL" --fields "$(jq -nc --arg p "$PROMPT" --argjson turbo "$TURBO" --argjson up "$UPSAMPLING" \
  '{instructions: $p, turbo: $turbo, prompt_upsampling: $up, megapixels: 1, aspect_ratio: "1:1", seed: 3}')" > "$OUT/box.json" 2>&1 ||
  fail "the box: $(tail -c 300 "$OUT/box.json")"
say "box q added: Calcifer predicts its render onto $NAME"
until_stage loaded "$WAIT"
say "q loaded $(( ($(date +%s%3N) - ASK) / 1000 )) s after the box was added"
# Its render takes the executor the load left.
ours render
python3 "$RUN_PAGE" render "$RUN_PROJECT" --on "$NAME" q > "$OUT/render.txt" 2>&1 || fail "the render: $(tail -c 400 "$OUT/render.txt")"
(cd "$RUN_PROJECT" && cozy --json run ls --all --limit 10) > "$OUT/runs.json" 2>&1
WARM=$(jq -r --argjson a "$ASK" '[.runs[]? | select(.created_at_ms >= $a)] | max_by(.created_at_ms) | .number // empty' "$OUT/runs.json")
[ -n "$WARM" ] || fail "the render's run: $(head -c 300 "$OUT/runs.json")"
report predicted "$WARM"

ARMS="cold predicted" SENT=$(date +%s%3N)
if [ "$YIELD" = true ]; then
  # The yield: SDXL's weights onto the pod, then TensorD restarted. Calcifer sends the box's request to the service that started,
  # which plans it again and loads it (the GPU is idle); while it does, SDXL is sent.
  ours other-fetch
  step other-fetch run "$OTHER" --on "$NAME" prompt="a brass compass on a walnut desk" seed:=4 || fail "the other call's fetch"
  unload yield
  until_stage loading 300
  ours other
  SENT=$(date +%s%3N)
  step other run "$OTHER" --on "$NAME" prompt="a brass compass on a walnut desk" seed:=5 || fail "the other call"
  report other "$(runno other)"
  # Then the GPU is idle again: it loads again, and the idle clock that began when SDXL ended stays where it was.
  until_stage loaded 600
  ARMS="$ARMS other"
fi
ours end
kill $BG 2> /dev/null; BG=''

collect
sed 's/^/version before /' "$OUT/versions.txt" >> "$R"
grep -aE "load ahead|queue: goes after" "$OUT/tensord.log" | cut -c1-300 | tee -a "$R" > "$OUT/load-ahead.log"
python3 - "$OUT" "$SENT" $ARMS <<'PY' | tee -a "$R" > "$OUT/summary.json"
import json, sys
out, sent, names = sys.argv[1], int(sys.argv[2]), sys.argv[3:]
rows = [json.loads(line) for line in open(f"{out}/status.ndjson") if line.strip().endswith("}")]
rows = [row for row in rows if row.get("status")]
seen, last = [], None
for row in rows:
    now = [(a.get("id"), (a.get("readiness") or {}).get("stage", a.get("state"))) for a in row["status"]["ahead"]]
    if now != last:
        seen.append({"at_ms": row["at_ms"], "ahead": now, "idle_deadline_ms": row["status"].get("idle_deadline_ms")})
        last = now
arms = {arm: json.load(open(f"{out}/report-{arm}.json")) for arm in names}
after = [row["status"].get("idle_deadline_ms") for row in rows if row["at_ms"] > sent]
print(json.dumps({"readiness": seen, "arms": arms, "idle_deadlines_after_other": sorted(set(d for d in after if d))}, indent=1))
PY
# Its first step (the run's first stage) starts with nothing to start, build or load: no executor start, no construction, no
# on-demand model built (`loading <slot>`), nothing loaded inside that stage. What varena pages in later, a model it let go to make
# room for another's working memory on a full card, is said, not judged (44060442909: the transformer at Turbo's first step).
jq -r '[.report.measured[0].stages[]?.name]' "$OUT/run-predicted.json" > "$OUT/stages-predicted.json"
first=$(jq -r 'map(select(test("^(executor start|construction)$|^loading ") | not)) | first // ""' "$OUT/stages-predicted.json")
built=$(jq -r 'map(select(test("^(executor start|construction)$|^loading "))) | join(", ")' "$OUT/stages-predicted.json")
inside=$(jq -r --arg f "$first" 'map(select(startswith($f + " / loading"))) | join(", ")' "$OUT/stages-predicted.json")
later=$(jq -r --arg f "$first" 'map(select(contains(" / loading") and (startswith($f + " /") | not))) | join(", ")' "$OUT/stages-predicted.json")
say "the predicted render's first step: $first; started or built: ${built:-nothing}; loaded in it: ${inside:-nothing}; paged in later: ${later:-nothing}"
[ -z "$built" ] && [ -z "$inside" ] || { say "FAIL: the predicted render's first step started an executor, or built or loaded a model"; RC=1; }
[ "$YIELD" = false ] || grep -q "load ahead: ended for a run" "$OUT/load-ahead.log" || { say "FAIL: no load yielded to SDXL"; RC=1; }
yielded=''; [ "$YIELD" = false ] || yielded='; the load yielded to SDXL'
[ "$RC" = 0 ] && say "PASS: loaded ahead (turbo $TURBO, upsampling $UPSAMPLING); the render's first step built and loaded nothing$yielded"
