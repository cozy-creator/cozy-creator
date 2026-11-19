# lane-fit (xs-213 phase 3, selection rule 8): a 4-bit floor's model resolves by fit, never refused for it. The release
# coordinator's ruling (11-18): "the rule picks the best-ranked lane whose bytes + KV + state + step room fit capacity, else
# the best lane"; capacity is "total or software cap, never free VRAM"; Paul: "every model runs in 6 GB, slow is fine".
# Two probes on one card of 16 GB or more, the session's cozy as its TensorD, each run until its model's header lands, its
# lane read from `machine show` and printed with the Hub's reasons, then canceled:
#   fits   Qwen3.5-9B: its best lane (int8) fits the card and is chosen, nothing passed over.
#   paged  the 27B default under a 6 GiB `--gpu-memory-limit`: no lane fits, and the best (int8) still serves, its reason
#          saying it runs paged. Red before rule 8: TensorD sent the free bytes, and the Hub served the first in the 4-bit
#          order (w4a8 on a card without FP4 tensor cores, nvfp4 on one with them).
# The Hub the session reads must serve rule 8 (tensorhub `Select` by `capacity`): prod once it is deployed, else
# HUB=isolated built from that branch, with both models seeded.
# params:
#   probes   [{"name", "target", "model", "want", "paged" (its reason must say so), "cap" (`--gpu-memory-limit`, `<n>GiB`),
#            "args" ([`cozy run` inputs and flags]; `{ref}` is release-gate/ref), "tree" (a packages_refs name: `./<target>`
#            runs from that checkout)}]; none: the two above
#   runtime  the pod's Runtime: a wheel source as lib.sh `wheel_from` takes; none: the pod's own
. "$(dirname "$0")/lib.sh"
Q='Name three things a lighthouse keeper does at dusk, in one sentence.'
PROBES=$(param probes '')
[ -n "$PROBES" ] || PROBES=$(jq -nc --arg q "$Q" '[
  {name: "fits", target: "qwen3/ask", model: "qwen/qwen3.5-9b", want: "int8", paged: false,
   args: ["--model", "model=qwen/qwen3.5-9b@0.2.0", ("prompt=" + $q), "max_tokens:=64"]},
  {name: "paged", target: "qwen3/ask", model: "llmfan46/qwen3.8-27b-uncensored-heretic-native-mtp-preserved", want: "int8",
   paged: true, cap: "6GiB", args: [("prompt=" + $q), "max_tokens:=64"]}]')
N=$(jq length <<<"$PROBES")
[ "$N" -gt 0 ] || fail "no probes"
for ((i = 0; i < N; i++)); do
  jq -e --argjson i "$i" '.[$i] | ([.name, .target, .model, .want] | all(type == "string")) and ((.args // []) | type == "array")
    and ((.cap // "") | test("^([0-9]+GiB)?$"))' <<<"$PROBES" > /dev/null || fail "probe $i needs name, target, model, want; cap <n>GiB"
  tree=$(jq -r --argjson i "$i" '.[$i].tree // ""' <<<"$PROBES")
  [ -z "$tree" ] || [ -d "$PACKAGES/$tree" ] || fail "probe $i: no packages checkout $tree"
done
RUNTIME='' SRC=$(param runtime '')
if [ -n "$SRC" ]; then RUNTIME=$(wheel_from "$OUT/wheels/runtime" "$SRC") && [ -n "$RUNTIME" ] || fail "no Runtime wheel from $SRC"; fi
quote $((1200 + 600 * N))
rent

jq -e '.status.gpus | length == 1 and (.[0].family | ltrimstr("sm") | tonumber) >= 80 and .[0].memory_bytes >= 16e9' \
  "$OUT/show.json" > /dev/null || fail "not one sm80+ card of 16 GB or more: $(jq -c '[.status.gpus[] | {name, family, memory_bytes}]' "$OUT/show.json")"
CARD=$(jq -r '.status.gpus[0] | "\(.name) (\(.family), \(.memory_bytes / 1e9 | floor) GB)"' "$OUT/show.json")
step update machine update "$NAME" --tensord "$(command -v cozy)" -y || fail "update"
[ -z "$RUNTIME" ] || step runtime machine update "$NAME" --runtime "$RUNTIME" -y || fail "the pod's Runtime"

# probe <i>: its cap set (or lifted), its target run until its model's header lands; writes the lane and the run's reasons.
probe() {
  local p name t m cap follower lane='' t0 run
  p=$(jq -c --argjson i "$1" '.[$i]' <<<"$PROBES"); name=$(jq -r .name <<<"$p"); t=$(jq -r .target <<<"$p")
  m=$(jq -r .model <<<"$p"); cap=$(jq -r '.cap // "off"' <<<"$p")
  step "cap-$name" machine update "$NAME" --gpu-memory-limit "$cap" -y || fail "$name: --gpu-memory-limit $cap"
  local args=()
  mapfile -t args < <(jq -r '.args // [] | .[]' <<<"$p" | sed "s|{ref}|$REF|g")
  if [ -n "$(jq -r '.tree // ""' <<<"$p")" ]; then t=$PACKAGES/$(jq -r .tree <<<"$p")/$t; fi
  (cd "$RUN_PROJECT" && cozy run "$t" "${args[@]}" --gpus=1 --on "$NAME") > "$OUT/run-$name.txt" 2>&1 & follower=$!
  t0=$(date +%s)
  while [ -z "$lane" ] && [ $(($(date +%s) - t0)) -lt 1200 ]; do
    sleep 5
    lane=$(cozy machine show "$NAME" --json 2> /dev/null | jq -r --arg m "$m" '[.status.models[]? | select(.repository == $m) | .checkpoints[].lane] | first // empty')
    kill -0 "$follower" 2> /dev/null || [ -n "$lane" ] || break
  done
  run=$(awk '$1=="run" && $2 ~ /^[0-9]+$/ {print $2; exit}' "$OUT/run-$name.txt")
  if [ -n "$run" ]; then
    (cd "$RUN_PROJECT" && cozy run cancel "$run" -y) >> "$OUT/cancel.txt" 2>&1
    (cd "$RUN_PROJECT" && cozy run watch "$run" --once --json --full) > "$OUT/run-$name.json" 2>&1
  fi
  kill "$follower" 2> /dev/null; wait "$follower" 2> /dev/null
  jq -c '[.. | objects | select(has("message")) | .message | select(type == "string" and test("lane|fits|paged"))] | unique' \
    "$OUT/run-$name.json" > "$OUT/reasons-$name.json" 2> /dev/null || echo '[]' > "$OUT/reasons-$name.json"
  say "probe $name: run ${run:-none}: $m resolved to ${lane:-nothing} on $CARD at cap $cap"
  say "probe $name reasons: $(cat "$OUT/reasons-$name.json")"
  [ -n "$lane" ] || say "probe $name: $m's header never landed: $(tail -3 "$OUT/run-$name.txt" | cut -c1-300)"
  echo "${lane:-nothing}" > "$OUT/lane-$name.txt"
}

failed=()
for ((i = 0; i < N; i++)); do
  probe "$i"
  p=$(jq -c --argjson i "$i" '.[$i]' <<<"$PROBES"); name=$(jq -r .name <<<"$p"); want=$(jq -r .want <<<"$p")
  got=$(cat "$OUT/lane-$name.txt") said=$(jq -r 'any(.[]; test("runs paged"))' "$OUT/reasons-$name.json")
  [ "$got" = "$want" ] || failed+=("$name: $got≠$want")
  [ "$said" = "$(jq -r '.paged // false' <<<"$p")" ] || failed+=("$name: its reasons say runs paged: $said")
done
cozy machine show "$NAME" --json > "$OUT/models.json" 2>&1
[ "${#failed[@]}" = 0 ] || fail "${failed[*]}"
say "PASS: $N probes resolved by fit as wanted"
