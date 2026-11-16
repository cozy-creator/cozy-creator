# lane-floor (cl-1076, se-143): each model resolves to the lane it must on a card that runs it, however little of the card
# is free. Paul, 11-06 (xs-154 Q1): at the default 8-bit floor "the 8-bit lane always serves; memory never lowers it". Paul,
# 11-16, on an RTX 5090 that held H3's nvfp4 lane: "we should be downloading the fp8 paths"; and (se-143) "make sure all
# models are int8 convrot lanes by default. All of them, including text encoders, and prompt enhancers and such." TensorD
# asked the Hub with the card's free bytes, so a slot whose 8-bit lane outgrew them was served 4-bit. One card with FP4
# tensor cores, the session's cozy as its TensorD: each probe's `target` runs until its `model`'s header lands, the lane is
# read from `machine show`, and the run is canceled. Red before a fix (nvfp4), green after.
# params:
#   probes   [{"target", "model", "want", "args" ([`cozy run` inputs and flags]; `{ref}` is release-gate/ref), "tree" (a
#            packages_refs name: `./<target>` runs from that checkout)}]; none: one probe of `target`, `model` and `want`
#   target   the default probe's function (minimax-h3/generate), with an H3 request
#   model    the model whose lane it judges (minimaxai/minimax-h3)
#   want     the lane it must resolve to (int8)
#   runtime  the pod's Runtime: a wheel source as lib.sh `wheel_from` takes (a cozy-runtime dev-build run id: its wheel
#            carries the kernels a machine requires); none: the pod's own
. "$(dirname "$0")/lib.sh"
T=$(param target minimax-h3/generate) M=$(param model minimaxai/minimax-h3) WANT=$(param want int8) PROBES=$(param probes '')
jq -n --argjson s "$RANDOM$RANDOM" '{beats: [{description: "A lighthouse on a sea cliff at dusk, waves breaking below.", duration: 5}], seed: $s}' \
  > "$OUT/request.json"
[ -n "$PROBES" ] || PROBES=$(jq -nc --arg t "$T" --arg m "$M" --arg w "$WANT" --arg r "$OUT/request.json" \
  '[{target: $t, model: $m, want: $w, args: ["--input", $r]}]')
N=$(jq length <<<"$PROBES")
[ "$N" -gt 0 ] || fail "no probes"
for ((i = 0; i < N; i++)); do
  jq -e --argjson i "$i" '.[$i] | (.target | type == "string") and (.model | type == "string") and (.want | type == "string")
    and ((.args // []) | type == "array")' <<<"$PROBES" > /dev/null || fail "probe $i needs target, model and want"
  tree=$(jq -r --argjson i "$i" '.[$i].tree // ""' <<<"$PROBES")
  [ -z "$tree" ] || [ -d "$PACKAGES/$tree" ] || fail "probe $i: no packages checkout $tree"
done
RUNTIME='' SRC=$(param runtime '')
if [ -n "$SRC" ]; then RUNTIME=$(wheel_from "$OUT/wheels/runtime" "$SRC") && [ -n "$RUNTIME" ] || fail "no Runtime wheel from $SRC"; fi
quote $((1200 + 600 * N))
rent

jq -e '.status.gpus | length == 1 and (.[0].family | ltrimstr("sm") | tonumber) >= 100' "$OUT/show.json" > /dev/null ||
  fail "not one card with FP4 tensor cores: $(jq -c '[.status.gpus[] | {name, family}]' "$OUT/show.json")"
CARD=$(jq -r '.status.gpus[0] | "\(.name) (\(.family), \(.memory_bytes / 1e9 | floor) GB)"' "$OUT/show.json")
step update machine update "$NAME" --tensord "$(command -v cozy)" -y || fail "update"
[ -z "$RUNTIME" ] || step runtime machine update "$NAME" --runtime "$RUNTIME" -y || fail "the pod's Runtime"

# probe <i>: the probe's target run until its model's header lands; prints the lane it resolved to.
probe() {
  local p t m dir follower lane='' t0 run
  p=$(jq -c --argjson i "$1" '.[$i]' <<<"$PROBES"); t=$(jq -r .target <<<"$p"); m=$(jq -r .model <<<"$p")
  local args=()
  mapfile -t args < <(jq -r '.args // [] | .[]' <<<"$p" | sed "s|{ref}|$REF|g")
  dir=$PWD
  if [ -n "$(jq -r '.tree // ""' <<<"$p")" ]; then dir=$PACKAGES/$(jq -r .tree <<<"$p"); t=./$t; fi
  (cd "$dir" && cozy run -p "$RUN_PROJECT" "$t" "${args[@]}" --gpus=1 --on "$NAME") > "$OUT/run-$1.txt" 2>&1 & follower=$!
  t0=$(date +%s)
  while [ -z "$lane" ] && [ $(($(date +%s) - t0)) -lt 1200 ]; do
    sleep 5
    lane=$(cozy machine show "$NAME" --json 2> /dev/null | jq -r --arg m "$m" '[.status.models[]? | select(.repository == $m) | .checkpoints[].lane] | first // empty')
    kill -0 "$follower" 2> /dev/null || [ -n "$lane" ] || break
  done
  run=$(awk '$1=="run" && $2 ~ /^[0-9]+$/ {print $2; exit}' "$OUT/run-$1.txt")
  if [ -n "$run" ]; then
    (cd "$RUN_PROJECT" && cozy run cancel "$run" -y) >> "$OUT/cancel.txt" 2>&1
    (cd "$RUN_PROJECT" && cozy run watch "$run" --once --json --full) > "$OUT/run-$1.json" 2>&1
  fi
  kill "$follower" 2> /dev/null; wait "$follower" 2> /dev/null
  say "probe $1 $t: run ${run:-none}: $m resolved to ${lane:-nothing} on $CARD"
  say "probe $1 warnings: $(jq -c '[.. | objects | select(has("message")) | .message | select(type == "string" and test("lane|fits"))] | unique' "$OUT/run-$1.json" 2> /dev/null)"
  [ -n "$lane" ] || say "probe $1: $m's header never landed: $(tail -3 "$OUT/run-$1.txt" | cut -c1-300)"
  echo "${lane:-nothing}" > "$OUT/lane-$1.txt"
}

failed=()
for ((i = 0; i < N; i++)); do
  probe "$i"
  m=$(jq -r --argjson i "$i" '.[$i].model' <<<"$PROBES"); want=$(jq -r --argjson i "$i" '.[$i].want' <<<"$PROBES")
  [ "$(cat "$OUT/lane-$i.txt")" = "$want" ] || failed+=("$m:$(cat "$OUT/lane-$i.txt")≠$want")
done
cozy machine show "$NAME" --json > "$OUT/models.json" 2>&1
[ "${#failed[@]}" = 0 ] || fail "${failed[*]}"
say "PASS: $N probes resolved as wanted"
