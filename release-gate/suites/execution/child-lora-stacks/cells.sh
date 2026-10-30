# A child call's own LoRA stack per slot (cl-687): six SDXL calls of one parent, each with its ModelChoice over the
# request's --lora, must record their stacks, differ, and hit the memo only on the same stack. Sourced by the runtime suite.
cell child-lora-stacks "$HERE/suites/execution/child-lora-stacks/compare" prompt="PEBalloonStyle, $OBJECT on a pier at dusk" \
  seed:=$((R + 341)) --lora proomptengineer/balloon-lora-sdxl
if [ -f child-lora-stacks.json ]; then
  stacks=$(run_get "$OUT" child-lora-stacks '[.calls[]? | if .memoized then "memo" else
    [.measurements.models[]?.adapters[]? | "\(.model | split("/") | last | sub("-lora-sdxl$"; "")) \(.strength * 1000 | round / 1000)"] | join("+") end] | join(" | ")')
  digests=$(run_get "$OUT" child-lora-stacks '[.result.calls[]?.digest] | "\(.[0:4] | unique | length) \(.[0] == .[5])"')
  landed=$(run_get "$OUT" child-lora-stacks '[.output[]?.path] | length')
  want="balloon 1 |  | balloon 1+ikea 0.8 | balloon 0.5 | balloon 1 | memo"
  [ "$stacks" = "$want" ] && [ "$digests" = "4 true" ] && [ "$landed" = 5 ] \
    && expect child-lora-stacks PASS "stacks [$stacks]; calls 0-3 four images, the repeat memoized; 5 landed" \
    || expect child-lora-stacks FAIL "stacks [$stacks], want [$want]; distinct/repeat [$digests], want [4 true]; landed ${landed:-0} of 5"
fi
