# A child call's own LoRA stack per slot (cl-687): six SDXL calls of one parent, each with its ModelChoice over the
# request's --lora, must record their stacks, differ, and hit the memo only on the same stack. Sourced by the runtime suite.
# With a --packages checkout it calls that checkout's sdxl (a path dependency, local-sources.sh), not the Hub's last release.
CLS=$HERE/suites/execution/child-lora-stacks
[ -z "$PKGS" ] || { pkg sdxl generate > /dev/null; CLS=$("$HERE/local-sources.sh" "$CLS" "$OUT/child-lora-stacks-src" "$PKGS"); }
cell child-lora-stacks "$CLS/compare" prompt="PEBalloonStyle, $OBJECT on a pier at dusk" \
  seed:=$((R + 341)) --lora proomptengineer/balloon-lora-sdxl
if [ -f child-lora-stacks.json ]; then
  stacks=$(run_get "$OUT" child-lora-stacks '[.calls[]? | if .memoized then "memo" else
    [.measurements.models[]?.adapters[]? | "\(.model | split("/") | last | sub("-lora-sdxl$"; "")) \(.strength * 1000 | round / 1000)"] | join("+") end] | join(" | ")')
  digests=$(run_get "$OUT" child-lora-stacks '[.result.calls[]?.digest] | "\(.[0:4] | unique | length) \(.[0] == .[5])"')
  landed=$(run_get "$OUT" child-lora-stacks '[.output[]? | select(.url)] | length')
  # The request's own strength for --lora without one: the LoRA's recommended strength since xs-183 (balloon's is 0.9).
  r=$(run_get "$OUT" child-lora-stacks '[.request.input.models[]?.lora[]?.strength][0] // 1 | . * 1000 | round / 1000')
  want="balloon $r |  | balloon $r+ikea 0.8 | balloon 0.5 | balloon $r | memo"
  [ "$stacks" = "$want" ] && [ "$digests" = "4 true" ] && [ "$landed" = 5 ] \
    && expect child-lora-stacks PASS "stacks [$stacks]; calls 0-3 four images, the repeat memoized; 5 landed" \
    || expect child-lora-stacks FAIL "stacks [$stacks], want [$want]; distinct/repeat [$digests], want [4 true]; landed ${landed:-0} of 5"
fi
