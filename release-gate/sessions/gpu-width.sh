# gpu-width (cl-1187, Paul 11-17: "a machine with 2x GPUs, if one long job is using a single GPU, and there's another free GPU, then
# minimax h3 should use that free GPU (even if it could use both GPUs ideally, rather than waiting for the second GPU to free up)").
# One 2-GPU rental, red then green, each arm's TensorD put on with `machine update --tensord`. H3 Turbo 15 s throughout. Each arm:
#   lone   H3 on the idle machine: both GPUs
#   hold   H3 at --gpus 1, one GPU; once it denoises, `next`, H3 with no count: red waits for 2 GPUs held by hold; green starts at once
#          on the free GPU
# Green then: `first` and `second`, two H3s sent together on the idle machine, each at the width the machine gives it (the second's
# wait says about how long), beside hold and next (two at once, a GPU each): the concurrent-vs-serial numbers for
# design/tensord-scheduler.md. Last, a Qwen Image box Calcifer predicts onto the pod loads ahead; while it loads, an H3 is sent: it
# takes both GPUs, the load yielding to it. params:
#   red       the dev-build run id whose cozy is red's TensorD; green's is the session's cozy (a dev build, pod-session's `cozy`)
#   ahead     false: no load-ahead step (default true)
. "$(dirname "$0")/lib.sh"
RED_RUN=$(param red) AHEAD=$(param ahead true) BEGAN=$(date +%s)
[[ $RED_RUN =~ ^[0-9]+$ ]] || fail "red is not a dev-build run id: '$RED_RUN'"
mkdir -p "$OUT/red-cozy"
gh run download "$RED_RUN" -R cozy-creator/cozy-creator -p 'cozy-*' -D "$OUT/red-cozy" > /dev/null 2>&1 || fail "red's cozy (dev-build $RED_RUN) wasn't fetched"
RED=$(find "$OUT/red-cozy" -type f -name 'cozy-linux-x86_64' | head -1)
[ -n "$RED" ] && chmod +x "$RED" || fail "dev-build $RED_RUN holds no cozy-linux-x86_64"
GREEN=$(command -v cozy)
say "red TensorD $("$RED" --version), green $("$GREEN" --version)"
RENT_ARGS="--gpus 2"
quote 5400
rent

# ours <arm> <bin> <step>: the pod runs the arm's TensorD before <step>; one moved off it is updated again, three times at most.
ours() {
  local want v t
  want=$("$2" --version | awk '{print $2}')
  for t in 1 2 3; do
    v=$(cozy machine show "$NAME" --json 2> /dev/null | jq -r '.status.version // ""')
    [ "$v" != "$want" ] || { echo "$(date -u +%T) $1 $3 $v" >> "$OUT/versions.txt"; return 0; }
    say "$1 $3: the pod runs ${v:-?}, not $want (try $t): updated again"
    step "$1-$3-update-$t" machine update "$NAME" --tensord "$2" -y
  done
  fail "$1 $3: the pod kept moving off its TensorD (${v:-?})"
}
# send <name> [flags]: H3 Turbo 15 s, a new seed, sent in the background.
send() {
  local name=$1; shift
  jq -n --argjson s "$RANDOM$RANDOM" '{summary: {references: [], text: "A steady tracking shot of a courier cycling a cargo bike through a narrow cobbled street after rain, past cafe tables and a flower stall, into a sunlit square."},
    beats: [{duration: 15, description: {references: [], text: "The courier rings the bell at the flower stall and rolls down the slope into the square."}}],
    soundscape: {references: [], text: "Tires hissing on wet cobbles, a bicycle bell, cafe chatter."}, seed: $s}' > "$OUT/$name.req.json"
  [ -n "$(run_submit "$OUT" "$name" run minimax-h3/generate --input "$OUT/$name.req.json" --on "$NAME" "$@")" ] ||
    fail "$name wasn't sent: $(tail -c 300 "$OUT/$name.err")"
  say "$name sent: $(cat "$OUT/$name.id")"
}
# denoising <name>: run <name> reached its first denoise step.
denoising() {
  local i
  for i in $(seq 1800); do
    grep '^{' "$OUT/$1.ndjson" 2> /dev/null | grep -q '"stage":"[^"]*denoise' && return 0
    run_alive "$(cat "$OUT/$1.pid")" || return 1
    sleep 1
  done
  return 1
}
# settled <name>: run <name> completed; its record, and its log as `run watch --full` prints it.
settled() {
  run_wait "$OUT" "$1" || fail "$1 did not end: $(tail -c 300 "$OUT/$1.ndjson")"
  [ "$(run_get "$OUT" "$1" .status)" = completed ] || fail "$1 $(run_get "$OUT" "$1" .status): $(run_get "$OUT" "$1" .reason.message | head -c 300)"
  (cd "$RUN_PROJECT" && cozy run watch "$(cat "$OUT/$1.id")" --once --full) > "$OUT/$1.log" 2>&1 || true
}
# facts <arm> <name>: what run <name> took (its calls' widest group), how long after it was sent it began to denoise, what it waited
# for as it said, and its times.
facts() {
  python3 - "$OUT" "$1" "$2" <<'PY' | tee -a "$R" >> "$OUT/facts.ndjson"
import json, re, sys
out, arm, name = sys.argv[1:]
run = json.load(open(f"{out}/{name}.json"))
log = open(f"{out}/{name}.log", errors="replace").read()
waits = sorted({line.split("stage: ", 1)[1].strip() for line in log.splitlines() if "stage: waiting for" in line})
said = [json.loads(line).get("reason", "") for line in open(f"{out}/{name}.ndjson") if line.startswith("{") and '"run.waiting"' in line]
# `(at 50.1s)`, `(at 3m21.1s)`: since it was sent.
denoise = next((60 * int(m.group(1) or 0) + float(m.group(2)) for m in re.finditer(r"stage: .*denoise.* \(at (?:(\d+)m)?([0-9.]+)s\)", log)), None)
degrees = [c.get("measurements", {}).get("execution", {}).get("degree", 0) for c in run.get("calls", [])]
print(json.dumps({"arm": arm, "run": name, "number": run.get("name"), "width": max(degrees or [0]), "denoise_at_s": denoise,
                  "waits": waits, "said": sorted({s for s in said if s}), "execution_s": round((run.get("execution_ms") or 0) / 1000, 1),
                  "accepted_to_settled_s": round(((run.get("settled_at_ms") or 0) - (run.get("accepted_at_ms") or 0)) / 1000, 1)}))
PY
}

arm() {
  local a=$1 bin=$2
  step "$a-tensord" machine update "$NAME" --tensord "$bin" -y || fail "$a: the pod refused its TensorD"
  ours "$a" "$bin" start
  say "$a: TensorD $(cozy machine show "$NAME" --json | jq -r .status.version)"
  send "$a-lone"
  settled "$a-lone"; facts "$a" "$a-lone"
  ours "$a" "$bin" hold
  send "$a-hold" --gpus 1
  denoising "$a-hold" || fail "$a: hold never denoised: $(tail -c 300 "$OUT/$a-hold.ndjson")"
  send "$a-next"
  settled "$a-hold"; settled "$a-next"
  facts "$a" "$a-hold"; facts "$a" "$a-next"
}
arm red "$RED"
arm green "$GREEN"
ours green "$GREEN" pair
send green-first; send green-second
settled green-first; settled green-second
facts green green-first; facts green green-second

if [ "$AHEAD" = true ]; then
  # A Qwen Image box on the page, Calcifer predicting its render onto the pod (load-ahead.sh): its weights come, it loads; TensorD
  # restarted (every executor ends), it loads again, and an H3 sent then takes both GPUs.
  python3 "$RUN_PAGE" show "$RUN_PROJECT" > "$OUT/shown.txt" 2>&1 & BG="$!"
  for _ in $(seq 1 30); do grep -q '"held"' "$OUT/shown.txt" && break; sleep 1; done
  ( while :; do
      printf '{"at_ms":%s,"status":%s}\n' "$(date +%s%3N)" \
        "$(cozy machine show "$NAME" --json 2> /dev/null | jq -c '{ahead: (.status.ahead // [])}' 2> /dev/null || echo null)" >> "$OUT/status.ndjson"
      sleep 1
    done ) & BG="$BG $!"
  stage() { tail -1 "$OUT/status.ndjson" | jq -r '[.status.ahead[]? | .readiness.stage // empty] | first // ""'; }
  until_stage() { local t=0; until [ "$(stage)" = "$1" ]; do sleep 1; t=$((t + 1)); [ "$t" -lt "$2" ] || return 1; done; }
  run_page add q --tool qwen-image-2/image --fields '{"instructions": "a red lighthouse on a cliff at dusk, waves below, photo", "turbo": false, "prompt_upsampling": false, "megapixels": 1, "aspect_ratio": "1:1", "seed": 3}' > "$OUT/box.json" 2>&1 ||
    fail "the box: $(tail -c 300 "$OUT/box.json")"
  if until_stage loaded 1200; then
    step ahead-restart machine update "$NAME" --mode dedicated -y || fail "the restart"
    ours green "$GREEN" ahead
    if until_stage loading 300; then
      say "q loads ahead: H3 sent"
      send green-ahead
      settled green-ahead; facts green green-ahead
    else say "q did not load again within 300 s: no H3 sent during a load"; fi
  else say "q was never loaded ahead within 1200 s"; fi
  kill $BG 2> /dev/null || true
fi

cozy machine logs "$NAME" --lines 0 --since "$(( ($(date +%s) - BEGAN) / 60 + 2 ))m" > "$OUT/tensord.log" 2>&1 || say "the pod's TensorD log wasn't read"
grep -aE "load ahead|queue: |its width" "$OUT/tensord.log" | cut -c1-300 > "$OUT/queue.log" || true
f() { jq -rs --arg r "$1" --arg k "$2" '.[] | select(.run == $r) | .[$k] | tostring' "$OUT/facts.ndjson"; }
say "red next: width $(f red-next width), denoise at $(f red-next denoise_at_s) s, waits $(f red-next waits)"
say "green next: width $(f green-next width), denoise at $(f green-next denoise_at_s) s, waits $(f green-next waits)"
[ "$(f red-next width)" = 2 ] && [[ $(f red-next waits) == *"waiting for 2 GPUs"* ]] || fail "inconclusive: red's next didn't wait for 2 GPUs"
[ "$(f red-lone width)" = 2 ] && [ "$(f green-lone width)" = 2 ] || fail "a lone H3 didn't take both GPUs (red $(f red-lone width), green $(f green-lone width))"
[ "$(f green-next width)" = 1 ] && [[ $(f green-next waits) != *GPU* ]] || fail "green's next didn't start at once on the free GPU"
[[ $(f green-second said) == *"about "*" min"* || $(f green-second said) == *"under a minute"* ]] || say "NOTE: green's second said no time for its wait: $(f green-second said)"
[ "$AHEAD" = false ] || [ -z "$(f green-ahead width)" ] || say "green ahead: width $(f green-ahead width); TensorD: $(grep -a 'load ahead' "$OUT/queue.log" | tail -3 | tr '\n' ' ')"
say "PASS: red's next waited for 2 GPUs held by hold; green's started at once on the free GPU; a lone H3 took both on each"
