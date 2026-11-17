# load-ahead (cl-1123 C, Paul 11-17: "if the target machine GPU is idle, like load the model weights into memory"): one rental,
# and a Qwen Image 2.1 box this runner's Calcifer predicts onto it (`PUT /v1/prepare`). Its machine reports the box's request
# `loaded` in its Status; the box's render then takes the executor the load left, and its run's own report has no executor start,
# construction or loading stage and reads nothing from the store in its first step, beside a cold call of the same function on the
# same machine. Then a load is made to yield: the executors ended, the box's request loads again, and a different call (SDXL) sent
# while it loads goes at once; TensorD's log says how long the GPU took to come back. The idle clock is read before and after a
# load: a load never moves it. Each arm starts with the pod's executors ended over SSH (no verb ends one); its disk stays.
# Enhancer and Turbo off: they are on-demand slots, built at a call's first read. params:
#   tool      the box's tool and the function the cold call runs (default qwen-image-2/image)
#   other     the call that takes the GPU during a load (default sdxl/generate)
#   wait_s    how long the box's request may take to be `loaded` (default 1200)
. "$(dirname "$0")/lib.sh"
TOOL=$(param tool qwen-image-2/image) OTHER=$(param other sdxl/generate) WAIT=$(param wait_s 1200)
PROMPT="a red lighthouse on a cliff at dusk, waves below, photo"
FIELDS='turbo:=false prompt_upsampling:=false megapixels:=1 aspect_ratio=1:1'
[ -f ~/.ssh/id_ed25519.pub ] || ssh-keygen -q -t ed25519 -N '' -f ~/.ssh/id_ed25519 || fail "ssh-keygen"
RENT_ARGS=--ssh
quote "$(param need_s 3600)"
rent
pod() { timeout "${T:-120}" cozy machine ssh "$NAME" "$@"; }
for i in $(seq 1 20); do pod true < /dev/null > /dev/null 2>&1 && break; [ "$i" = 20 ] && fail "no SSH into $NAME"; sleep 15; done
step tensord machine update "$NAME" --tensord "$(command -v cozy)" -y || fail "the pod refused the session's TensorD"
v=$(cozy machine show "$NAME" --json | jq -r .status.version)
[ "$v" = "$(cozy --version | awk '{print $2}')" ] || fail "the pod runs TensorD $v, not the session's"
say "pod TensorD $v, Runtime $(cozy machine show "$NAME" --json | jq -r '.status.runtime // empty')"

# The pod's Status as Calcifer relays it, each second: each predicted request's readiness and the idle clock.
( while :; do
    printf '{"at_ms":%s,"status":%s}\n' "$(date +%s%3N)" \
      "$(cozy machine show "$NAME" --json 2> /dev/null | jq -c '{ahead: (.status.ahead // []), idle_deadline_ms: .status.idle_deadline_ms}' 2> /dev/null || echo null)" >> "$OUT/status.ndjson"
    sleep 1
  done ) & BG=$!
# stage: the newest readiness stage the pod says for any predicted request ("" for none).
stage() { tail -1 "$OUT/status.ndjson" | jq -r '[.status.ahead[]? | .readiness.stage // empty] | first // ""'; }
# until <stage> <seconds>: the pod says it, else fail.
until_stage() {
  local t=0
  until [ "$(stage)" = "$1" ]; do
    sleep 1; t=$((t + 1)); [ "$t" -lt "$2" ] || fail "no predicted request was $1 within $2 s: $(tail -1 "$OUT/status.ndjson" | cut -c1-400)"
  done
  say "a predicted request is $1 ($t s)"
}
# unload: every executor on the pod ended (its disk stays), and its GPU's memory back.
unload() {
  T=60 pod "python3 -c 'import glob, os, signal
for d in glob.glob(\"/proc/[0-9]*\"):
    try:
        if b\"cozy_runtime.internal.executor\" in open(d + \"/cmdline\", \"rb\").read(): os.kill(int(d[6:]), signal.SIGKILL); print(d[6:])
    except OSError: pass'" < /dev/null >> "$OUT/unload.txt" 2>&1
  local used
  for _ in $(seq 1 60); do
    used=$(T=30 pod "nvidia-smi --query-gpu=memory.used --format=csv,noheader,nounits" < /dev/null 2> /dev/null | head -1 | tr -dc 0-9)
    [ -n "$used" ] && [ "$used" -lt 1500 ] && { say "executors ended: GPU 0 holds $used MiB"; return; }
    sleep 2
  done
  fail "the GPU kept $used MiB after its executors ended"
}
# report <name> <run>: the run's own record, and its report's stages and first steps.
report() {
  (cd "$RUN_PROJECT" && cozy --json run watch "$2" --once --full) > "$OUT/run-$1.json" 2>&1 || fail "$1: run $2 unread"
  jq -c --arg arm "$1" '{arm: $arm, run: .number, status, execution_ms,
    stages: [.report.measured[0].stages[]? | {(.name): (.ms | floor)}] | add,
    first_steps: [.report.measured[0].steps[]? | select(.step <= 2) | {step, from_store: .store, resident: (.hit + .kept)}]}' \
    "$OUT/run-$1.json" | tee -a "$R" > "$OUT/report-$1.json"
}

# Its weights onto the pod by one call of it, which downloads them: every arm after reads them from its disk.
step fetch run "$TOOL" --on "$NAME" instructions="$PROMPT" $FIELDS seed:=1 || fail "the first call"
unload
# The cold call: its weights on disk, no executor.
step cold run "$TOOL" --on "$NAME" instructions="$PROMPT" $FIELDS seed:=2 || fail "the cold call"
report cold "$(runno cold)"
unload

# The prediction: a box with the tool, as a person puts one on the canvas. Calcifer guesses its render onto the pod.
ASK=$(date +%s%3N)
run_page add q --tool "$TOOL" --fields "$(jq -nc --arg p "$PROMPT" '{instructions: $p, turbo: false, prompt_upsampling: false,
  megapixels: 1, aspect_ratio: "1:1", seed: 3}')" > "$OUT/box.json" 2>&1 || fail "the box: $(tail -c 300 "$OUT/box.json")"
say "box q added: Calcifer predicts its render onto $NAME"
until_stage loaded "$WAIT"
say "q loaded $(( ($(date +%s%3N) - ASK) / 1000 )) s after the box was added"
# Its render takes the executor the load left.
python3 "$RUN_PAGE" render "$RUN_PROJECT" --on "$NAME" q > "$OUT/render.txt" 2>&1 || fail "the render: $(tail -c 400 "$OUT/render.txt")"
(cd "$RUN_PROJECT" && cozy --json run ls --all --limit 10) > "$OUT/runs.json" 2>&1
WARM=$(jq -r --argjson a "$ASK" '[.runs[]? | select(.created_at_ms >= $a)] | max_by(.created_at_ms) | .number // empty' "$OUT/runs.json")
[ -n "$WARM" ] || fail "the render's run: $(head -c 300 "$OUT/runs.json")"
report predicted "$WARM"

# The yield: SDXL's weights onto the pod, then the executors ended. The box's request loads again at once (the GPU is idle);
# while it does, SDXL is sent.
step other-fetch run "$OTHER" --on "$NAME" prompt="a brass compass on a walnut desk" seed:=4 || fail "the other call's fetch"
unload
until_stage loading 300
SENT=$(date +%s%3N)
step other run "$OTHER" --on "$NAME" prompt="a brass compass on a walnut desk" seed:=5 || fail "the other call"
report other "$(runno other)"
# Then the GPU is idle again: it loads again, and the idle clock that began when SDXL ended stays where it was.
until_stage loaded 600
kill "$BG" 2> /dev/null; BG=''

cozy machine logs "$NAME" --lines 0 --since "$(( ($(date +%s) - ASK / 1000) / 60 + 15 ))m" > "$OUT/tensord.log" 2>&1 || say "the pod's TensorD log wasn't read"
grep -aE "load ahead|queue: goes after" "$OUT/tensord.log" | cut -c1-300 | tee -a "$R" > "$OUT/load-ahead.log"
python3 - "$OUT" "$SENT" <<'PY' | tee -a "$R" > "$OUT/summary.json"
import json, sys
out, sent = sys.argv[1], int(sys.argv[2])
rows = [json.loads(line) for line in open(f"{out}/status.ndjson") if line.strip().endswith("}")]
rows = [row for row in rows if row.get("status")]
seen, last = [], None
for row in rows:
    now = [(a.get("id"), (a.get("readiness") or {}).get("stage", a.get("state"))) for a in row["status"]["ahead"]]
    if now != last:
        seen.append({"at_ms": row["at_ms"], "ahead": now, "idle_deadline_ms": row["status"].get("idle_deadline_ms")})
        last = now
arms = {arm: json.load(open(f"{out}/report-{arm}.json")) for arm in ("cold", "predicted", "other")}
after = [row["status"].get("idle_deadline_ms") for row in rows if row["at_ms"] > sent]
print(json.dumps({"readiness": seen, "arms": arms, "idle_deadlines_after_other": sorted(set(d for d in after if d))}, indent=1))
PY
[ "$(jq '[.stages // {} | keys[] | select(test("executor start|construction|^loading"))] | length' "$OUT/report-predicted.json")" = 0 ] ||
  { say "FAIL: the predicted render's run started an executor or loaded a model"; RC=1; }
grep -q "load ahead: ended for a run" "$OUT/load-ahead.log" || { say "FAIL: no load yielded to SDXL"; RC=1; }
[ "$RC" = 0 ] && say "PASS: loaded ahead; the render loaded nothing; the load yielded to SDXL"
