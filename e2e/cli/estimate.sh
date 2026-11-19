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
    # gpuless-place's Qwen box (its stand-g quote is this plan's).
    *) req='{"instructions": "A red bicycle against a white wall at night.", "prompt_upsampling": false}' want='{"megapixels": 2, "prompt_upsampling": 0, "steps": 4}' ;;
  esac
  echo "$req" | "$plan" "$OUT/est-$1.interface.json" "$1/$2" "$PACKAGES/$1/benchmarks.json" "$OUT/est-card.json" > "$OUT/est-$1.plan.json" 2>&1 \
    || check "estimate: $1/$2's plan failed: $(tail -c 300 "$OUT/est-$1.plan.json")"
  echo "estimate: $1/$2 of $req on an RTX 4090: $(cat "$OUT/est-$1.plan.json")"
  J --argjson w "$want" '.size == $w and .faults == [] and (.estimate.ms > 0) and .estimate.basis == "reference" and .row.basis == "reference"' "$OUT/est-$1.plan.json" \
    || check "estimate: $1/$2's plan isn't size $want with a time from an RTX 4090 row"
done
}
