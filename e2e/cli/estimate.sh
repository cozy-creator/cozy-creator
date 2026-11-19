# estimate_plan (cl-1286; Paul, 11-18: "we give it a prediction, and it says 'I need x models, and inference will take y'"): a
# function's own size variables, as its plan reads a request, in its run's record; one that gives no
# value left out and said once; a function without `estimate=` as before; a broken expression refused as cozy reads the package, in
# one line, and a run of it planned as one without, said once; and the plan of real H3 and Qwen Image 2 requests (their packages checkout).
# The plan's time is the platform's own row pick (`References::estimate`, #3983's review): the row a formula reads and the default's,
# one; on a machine with no GPU a formula's time counts only where it read a CPU row (sized's, which reads none, gives none there).
estimate_plan() {
step "estimate plan (cl-1286): a request's size by its function's own variables, in its run's record"
local s=$here/estimate/sized size='{"seconds": 7, "megapixels": 0.9216, "steps": 30}' code
local plan=$OUT/needs-build/debug/examples/needs-plan
# The evaluator Calcifer's quote and TensorD's plan share (crates/needs), as its example, over the interfaces cozy reads.
(cd "$here/../.." && nice -n 19 cargo build --locked -q -p needs --example needs-plan --target-dir "$OUT/needs-build") > "$OUT/needs-build.log" 2>&1 \
  || { check "estimate: needs-plan didn't build: $(tail -c 300 "$OUT/needs-build.log")"; plan=''; }
run_cell "$OUT" est-sized "$s/sized" duration:=7 width:=1280 height:=720 turbo:=false --on local \
  || check "estimate: sized didn't complete: $(run_get "$OUT" est-sized .reason.message)"
run_show "$OUT" est-sized "$(cat "$OUT/est-sized.number")"
echo "estimate: sized's record: $(jq -c '{status, size: .measurements.size, said: [.. | strings | select(test("size variable"))]}' "$OUT/est-sized.json" 2> /dev/null | head -c 900)"
J --argjson s "$size" '.measurements.size == $s' "$OUT/est-sized.json" || check "estimate: sized's record keeps no size $size"
J '[.. | strings | select(test("its size variable frames gives no value"))] | length >= 1' "$OUT/est-sized.json" \
  || check "estimate: sized's record doesn't say frames gave no value"

step "estimate plan: a function with no estimate= is sized and recorded as before"
run_cell "$OUT" est-plain "$here/weightless/reverse" text=sized --on local || check "estimate: reverse didn't complete"
run_show "$OUT" est-plain "$(cat "$OUT/est-plain.number")"
J '.measurements.size == null' "$OUT/est-plain.json" || check "estimate: reverse's record has a size: $(jq -c .measurements.size "$OUT/est-plain.json")"

step "estimate plan: a broken expression refused as cozy reads the package, in one line; a run of it says so once and runs as before"
code=0
"$RUN_CLI" run "$here/estimate/broken/broken" --describe > "$OUT/est-broken.out" 2> "$OUT/est-broken.err" || code=$?
echo "estimate: broken's describe exited $code: $(grep -m1 'estimate_invalid' "$OUT/est-broken.err" "$OUT/est-broken.out")"
[ "$code" != 0 ] && grep -q 'request.durtion' "$OUT/est-broken.err" "$OUT/est-broken.out" || check "estimate: broken's describe wasn't refused for request.durtion"
run_cell "$OUT" est-broken-run "$here/estimate/broken/broken" duration:=3 --on local || check "estimate: a run of broken didn't complete"
run_show "$OUT" est-broken-run "$(cat "$OUT/est-broken-run.number")"
echo "estimate: broken's run: $(jq -c '{status, size: .measurements.size, said: [.. | strings | select(test("estimate can.t be read"))]}' "$OUT/est-broken-run.json" 2> /dev/null | head -c 600)"
J '.measurements.size == null and ([.. | strings | select(test("its estimate can.t be read"))] | length >= 1)' "$OUT/est-broken-run.json" \
  || check "estimate: broken's run doesn't say its estimate can't be read, or keeps a size"

if [ -n "$plan" ]; then
  step "estimate plan: a formula that reads no row times a card with a GPU, never a machine without one"
  "$RUN_CLI" --json --full run "$s/sized" --describe > "$OUT/est-sized.describe.json" 2> "$OUT/est-sized.describe.err" \
    || check "estimate: cozy couldn't read sized: $(tail -c 300 "$OUT/est-sized.describe.err")"
  jq .interface "$OUT/est-sized.describe.json" > "$OUT/est-sized.interface.json"
  echo '[]' > "$OUT/est-norows.json"
  echo '{"gpu": "NVIDIA GeForce RTX 4090", "gpus": 1, "memory_bytes": 25757220864}' > "$OUT/est-gpu.json"
  echo '{"memory_bytes": 16772423680}' > "$OUT/est-cpu.json"
  for c in gpu cpu; do
    echo '{"duration": 7, "width": 1280, "height": 720, "turbo": false}' | "$plan" "$OUT/est-sized.interface.json" e2e-sized/sized "$OUT/est-norows.json" "$OUT/est-$c.json" \
      > "$OUT/est-sized-$c.plan.json" 2>&1 || check "estimate: sized's plan on $c failed: $(tail -c 300 "$OUT/est-sized-$c.plan.json")"
    echo "estimate: sized on $c: $(jq -c .estimate "$OUT/est-sized-$c.plan.json" 2> /dev/null)"
  done
  J '.estimate.ms == 26250 and .estimate.basis == "reference"' "$OUT/est-sized-gpu.plan.json" || check "estimate: sized's formula didn't time an RTX 4090 at 26,250 ms"
  J '.estimate.ms == null and ([.estimate.unanswered[]? | .[1] | test("no GPU")] | any)' "$OUT/est-sized-cpu.plan.json" \
    || check "estimate: sized's formula, which reads no row, timed a machine with no GPU"
fi

[ -n "${PACKAGES:-}" ] && [ -n "$plan" ] || { echo "estimate: no packages checkout or no needs-plan: H3 and Qwen Image 2 not planned"; return 0; }
step "estimate plan: real H3 and Qwen Image 2 requests, planned from the interfaces cozy read"
local p f rows card='{"gpu": "NVIDIA GeForce RTX 4090", "gpus": 1, "memory_bytes": 25757220864}' req want
echo "$card" > "$OUT/est-card.json"
for p in "minimax-h3 generate" "qwen-image-2 image"; do
  set -- $p
  "$RUN_CLI" --json --full run "$PACKAGES/$1/$2" --describe > "$OUT/est-$1.describe.json" 2> "$OUT/est-$1.describe.err" \
    || { check "estimate: cozy couldn't read $1/$2: $(tail -c 300 "$OUT/est-$1.describe.err")"; continue; }
  jq .interface "$OUT/est-$1.describe.json" > "$OUT/est-$1.interface.json"
  case $1 in
    minimax-h3) req='{"beats": [{"duration": 15}, {"duration": 15}], "turbo": true}' want='{"beats": 2, "seconds": 30, "segments": 2, "steps": 8}' ;;
    # gpuless-place's Qwen box (its stand-g quote is this plan's); its 9 words where it counts them (cl-1286, packages #1063).
    *) req='{"instructions": "A red bicycle against a white wall at night.", "prompt_upsampling": false}' want='{"megapixels": 2, "prompt_upsampling": 0, "steps": 4}' ;;
  esac
  echo "$req" | "$plan" "$OUT/est-$1.interface.json" "$1/$2" "$PACKAGES/$1/benchmarks.json" "$OUT/est-card.json" > "$OUT/est-$1.plan.json" 2>&1 \
    || check "estimate: $1/$2's plan failed: $(tail -c 300 "$OUT/est-$1.plan.json")"
  echo "estimate: $1/$2 of $req on an RTX 4090: $(cat "$OUT/est-$1.plan.json")"
  J --argjson w "$want" '(.size | del(.prompt_words)) == $w and (.size.prompt_words // 9) == 9 and .faults == [] and (.estimate.ms > 0) and .estimate.basis == "reference" and .row.basis == "reference"' "$OUT/est-$1.plan.json" \
    || check "estimate: $1/$2's plan isn't size $want with a time from an RTX 4090 row"
done

# Qwen Image 2's prompt words (cl-1286; packages #1063: its rewrite ≈ 9.1 s × words^0.29 on an RTX 4090): a 9-word and a 210-word box at
# its defaults quoted from their measured rows (20.6 s and 45.95 s, the mean of two seeds each), and the 210-word box after a 9-word run
# on that card quoted as its formula scales it, 2.23 times, never in proportion to its words (23 times).
jq -e '[.entrypoints[]? | select(.name == "image") | .estimate.size.prompt_words] | any' "$OUT/est-qwen-image-2.interface.json" > /dev/null 2>&1 \
  || { echo "estimate: this Qwen Image 2 counts no prompt words"; return 0; }
step "estimate plan: Qwen Image 2's short and long prompt from their rows; a long one after a short run, as its formula scales it"
local short='{"instructions": "A red bicycle against a white wall at night."}' long
long=$(python3 -c 'import json; print(json.dumps({"instructions": " ".join(["word"] * 210)}))')
local q=("$OUT/est-qwen-image-2.interface.json" qwen-image-2/image "$PACKAGES/qwen-image-2/benchmarks.json" "$OUT/est-card.json")
echo "$short" | "$plan" "${q[@]}" > "$OUT/est-qwen-short.plan.json" 2>&1 || check "estimate: Qwen's short prompt didn't plan: $(tail -c 300 "$OUT/est-qwen-short.plan.json")"
echo "$long" | "$plan" "${q[@]}" > "$OUT/est-qwen-long.plan.json" 2>&1 || check "estimate: Qwen's long prompt didn't plan: $(tail -c 300 "$OUT/est-qwen-long.plan.json")"
jq -c '[{size: .size, ms: 20600}]' "$OUT/est-qwen-short.plan.json" > "$OUT/est-qwen-runs.json"
echo "$long" | "$plan" "${q[@]}" "$OUT/est-qwen-runs.json" > "$OUT/est-qwen-past.plan.json" 2>&1 || check "estimate: Qwen's long prompt after a short run didn't plan: $(tail -c 300 "$OUT/est-qwen-past.plan.json")"
for k in short long past; do echo "estimate: Qwen $k: $(jq -c '{size, estimate: .estimate | {ms, basis}, past}' "$OUT/est-qwen-$k.plan.json" 2> /dev/null)"; done
J '.size.prompt_words == 9 and (.estimate.ms - 20600 | fabs) < 206' "$OUT/est-qwen-short.plan.json" || check "estimate: Qwen's 9-word box isn't quoted 20.6 s from its row"
J '.size.prompt_words == 210 and (.estimate.ms - 45950 | fabs) < 460' "$OUT/est-qwen-long.plan.json" || check "estimate: Qwen's 210-word box isn't quoted 45.95 s from its row"
J '.past.basis == "measured" and (.past.ms / 20600 - 2.23 | fabs) < 0.03' "$OUT/est-qwen-past.plan.json" \
  || check "estimate: Qwen's 210-word box after a 9-word run of 20.6 s isn't 2.23 times it, as its formula scales: $(jq -c .past "$OUT/est-qwen-past.plan.json" 2> /dev/null)"
}

# estimate_scaled (cl-1286; the packages agent, 11-19: a declared prompt-length variable "would quote a 210-word prompt after a 9-word
# run at 23× instead of about 2.4×"). A function's past runs scale to the size asked by the same exponents its rows do, never in straight
# proportion: the scaled tool's CPU rows measure its time as seconds^0.5, and once it has rendered at 4 s here, the page's estimate of it
# at 16 s is twice that at 4 s, as its rows say ((16/4)^0.5), not four times (rc/0.28.0 scaled its runs in proportion). And a tool no
# benchmarks.json or run times (cl-1279, 11-19: "show the time as unknown") is still rendered, its estimate and its run's placed time
# unknown, never a flat minute.
estimate_scaled() {
step "estimate scaled (cl-1286): a past run scales by its function's exponents, as its rows do; with nothing measured, its time is unknown"
local P=$OUT/scaled tool=$here/estimate/scaled untimed=$OUT/scaled-untimed est long short n watch
rm -rf "$untimed" && cp -r "$tool" "$untimed" && rm -f "$untimed/benchmarks.json"
sed -i 's/^name = "e2e-scaled"$/name = "e2e-untimed"/' "$untimed/pyproject.toml"
cozy --json new --unencrypted "$P" > "$OUT/scaled-new.json" 2>&1 || { check "estimate scaled: new: $(tail -c 300 "$OUT/scaled-new.json")"; return; }
jq -n --arg t "$tool/scaled" --arg u "$untimed/scaled" '[
  {name: "first", tool: $t, kind: "image", fields: {instructions: "Measured here.", seconds: 4}},
  {name: "long", tool: $t, kind: "image", fields: {instructions: "Four times its seconds.", seconds: 16}},
  {name: "short", tool: $t, kind: "image", fields: {instructions: "Its seconds again.", seconds: 4}},
  {name: "untimed", tool: $u, kind: "image", fields: {instructions: "Nothing times this.", seconds: 1}}]' | boxes_in "$P" || return
coming_go first '["local"]'
coming_done first 600
for _ in $(seq 20); do
  project_api "$P" /fit -X POST -H 'Content-Type: application/json' -d '{"names":["long","short"]}' > "$OUT/scaled-fit.json" 2> "$OUT/scaled-fit.err" || true
  long=$(jq -r '.estimates[]? | select(.name == "long" and .basis == "measured") | .run_ms // empty' "$OUT/scaled-fit.json" 2> /dev/null || true)
  short=$(jq -r '.estimates[]? | select(.name == "short" and .basis == "measured") | .run_ms // empty' "$OUT/scaled-fit.json" 2> /dev/null || true)
  [ -n "$long" ] && [ -n "$short" ] && break
  sleep 3
done
echo "estimate scaled: after first's render at 4 s, the estimates: $(jq -c '[.estimates[]? | {name, machine, run_ms, basis}]' "$OUT/scaled-fit.json" 2> /dev/null)"
[ -n "$long" ] && [ -n "$short" ] && [ "$short" -gt 0 ] \
  && python3 -c 'import sys; r = int(sys.argv[1]) / int(sys.argv[2]); print(f"estimate scaled: 16 s over 4 s is {r:.3f}, its rows say 2.000"); sys.exit(abs(r - 2.0) > 0.05)' "$long" "$short" \
  || check "estimate scaled: the estimate at 16 s isn't twice that at 4 s from first's run, as its rows' exponent 0.5 says: long ${long:-none}, short ${short:-none}"

project_api "$P" /fit -X POST -H 'Content-Type: application/json' -d '{"names":["untimed"]}' > "$OUT/scaled-fit-untimed.json" 2> "$OUT/scaled-fit-untimed.err" || true
est=$(jq -c '.estimates[]? | select(.name == "untimed")' "$OUT/scaled-fit-untimed.json" 2> /dev/null || true)
echo "estimate scaled: untimed's estimate ${est:-none}"
J '.machine == "local" and .run_ms == null and .basis == "none"' <<< "${est:-null}" \
  || check "estimate scaled: untimed's estimate isn't on local with its time unknown: ${est:-none} $(tail -c 300 "$OUT/scaled-fit-untimed.err")"
coming_go untimed '["local"]'
coming_done untimed 600
in_project "$P" --json run ls --limit 0 > "$OUT/scaled-runs.json"
n=$(jq -r '[.runs[] | select(.kind == "call" and (.target | test("untimed")))] | first | .number // empty' "$OUT/scaled-runs.json")
[ -n "$n" ] || { check "estimate scaled: untimed's render made no run: $(jq -c '[.runs[] | {number, kind, target, status}]' "$OUT/scaled-runs.json")"; return; }
in_project "$P" --json --full run watch "$n" --once > "$OUT/scaled-untimed.json" 2> /dev/null || true
watch=$(in_project "$P" run watch "$n" --once 2> /dev/null | grep -i 'placed' || true)
echo "estimate scaled: untimed's run $n: $(jq -c '{machine, status, placed}' "$OUT/scaled-untimed.json" 2> /dev/null); cozy says: ${watch:-nothing placed}"
J '.status == "completed" and .machine == "local" and .placed.machine == "local" and .placed.basis == "none" and .placed.finish_at_ms == null' "$OUT/scaled-untimed.json" \
  || check "estimate scaled: untimed's run isn't completed on local, placed there with its time unknown: $(jq -c '{status, machine, placed}' "$OUT/scaled-untimed.json" 2> /dev/null)"
grep -q 'its time unknown' <<< "$watch" || check "estimate scaled: cozy run watch doesn't say untimed's time is unknown: ${watch:-nothing}"
}
