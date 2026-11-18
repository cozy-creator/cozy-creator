# estimate_plan (cl-1286; Paul, 11-18: "we give it a prediction, and it says 'I need x models, and inference will take y'"): a
# function's own size variables, as its plan reads a request, in `cozy run --estimate` and in its run's record; one that gives no
# value left out and said once; a function without `estimate=` as before; a broken expression refused as cozy reads the package, in
# one line; and the plan of real H3 and Qwen Image 2 requests (their packages checkout).
estimate_plan() {
step "estimate plan (cl-1286): a request's size by its function's own variables, in its plan and its run's record"
local s=$here/estimate/sized size='{"seconds": 7, "megapixels": 0.9216, "steps": 30}' code
local ask=(duration:=7 width:=1280 height:=720 turbo:=false --on local)
"$RUN_CLI" --json run "$s/sized" "${ask[@]}" --estimate > "$OUT/est-plan.json" 2> "$OUT/est-plan.err" \
  || check "estimate: --estimate of sized failed: $(tail -c 300 "$OUT/est-plan.err")"
echo "estimate: sized's plan: $(head -c 600 "$OUT/est-plan.json")"
J --argjson s "$size" '[.. | objects | select(has("machine") and has("fits"))] | any(.size == $s and .formula_ms == 26250)' "$OUT/est-plan.json" \
  || check "estimate: sized's plan isn't size $size and its formula's 26250 ms"
run_cell "$OUT" est-sized "$s/sized" "${ask[@]}" || check "estimate: sized didn't complete: $(run_get "$OUT" est-sized .reason.message)"
run_show "$OUT" est-sized "$(cat "$OUT/est-sized.number")"
echo "estimate: sized's record: $(jq -c '{status, size: .measurements.size, said: [.. | strings | select(test("size variable"))]}' "$OUT/est-sized.json" 2> /dev/null | head -c 900)"
J --argjson s "$size" '.measurements.size == $s' "$OUT/est-sized.json" || check "estimate: sized's record keeps no size $size"
J '[.. | strings | select(test("its size variable frames gives no value"))] | length >= 1' "$OUT/est-sized.json" \
  || check "estimate: sized's record doesn't say frames gave no value"

step "estimate plan: a function with no estimate= is sized and recorded as before"
run_cell "$OUT" est-plain "$here/weightless/reverse" text=sized --on local || check "estimate: reverse didn't complete"
run_show "$OUT" est-plain "$(cat "$OUT/est-plain.number")"
J '.measurements.size == null' "$OUT/est-plain.json" || check "estimate: reverse's record has a size: $(jq -c .measurements.size "$OUT/est-plain.json")"

step "estimate plan: a broken expression refused as cozy reads the package, in one line"
"$RUN_CLI" run "$here/estimate/broken/broken" duration:=3 --on local > "$OUT/est-broken.out" 2> "$OUT/est-broken.err"; code=$?
echo "estimate: broken exited $code: $(cat "$OUT/est-broken.err" | head -c 600)"
[ "$code" != 0 ] && grep -q 'request.durtion' "$OUT/est-broken.err" || check "estimate: broken wasn't refused for request.durtion"

[ -n "${PACKAGES:-}" ] || { echo "estimate: no packages checkout: H3 and Qwen Image 2 not planned"; return 0; }
step "estimate plan: real H3 and Qwen Image 2 requests"
"$RUN_CLI" --json run "$PACKAGES/minimax-h3/generate" 'beats:=[{"duration": 10}, {"duration": 14}]' turbo:=true --on local --estimate \
  > "$OUT/est-h3.json" 2> "$OUT/est-h3.err" || check "estimate: H3's --estimate failed: $(tail -c 300 "$OUT/est-h3.err")"
echo "estimate: H3's plan: $(head -c 600 "$OUT/est-h3.json")"
J '[.. | objects | select(has("fits"))] | any(.size == {"seconds": 24, "steps": 8})' "$OUT/est-h3.json" \
  || check "estimate: H3's plan isn't seconds 24, steps 8"
"$RUN_CLI" --json run "$PACKAGES/qwen-image-2/image" instructions="a lighthouse at dusk" megapixels:=4 turbo:=false --on local --estimate \
  > "$OUT/est-qwen.json" 2> "$OUT/est-qwen.err" || check "estimate: Qwen's --estimate failed: $(tail -c 300 "$OUT/est-qwen.err")"
echo "estimate: Qwen Image 2's plan: $(head -c 600 "$OUT/est-qwen.json")"
J '[.. | objects | select(has("fits"))] | any(.size == {"megapixels": 4, "steps": 40})' "$OUT/est-qwen.json" \
  || check "estimate: Qwen's plan isn't megapixels 4, steps 40"
}
