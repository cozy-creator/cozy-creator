# engine-recapture (xs-213): the Runtime's `bench.engine_recapture` (fixtures/kernel-ready) on one rental of $SKU, on each
# Runtime of `runtimes` in turn, each mode a real `cozy run … --on <pod>`: a small random-weight Qwen3.5 through the
# engine's decoder, its first call eager while a primed probe opens (its capture refused, `Building`), the same call after
# the probe lands (`lands`) or fails (`fails`) captured with the same tokens.
# `wants`, one word a Runtime (space-separated, in `runtimes`' order), says each mode's verdict there: `lands=PASS,fails=PASS`
# for the change, a red twin's FAIL where the change it lacks is what that mode proves.
# params: tensorfs, runtimes (required: dev-build runs), wants (required), seconds (1500), skus and quoted (preflight.sh).
. "$(dirname "$0")/lib.sh"
HERE=$(cd "$(dirname "$0")" && pwd)
FIX=$HERE/fixtures/ggml-decode
DRIVE=$HERE/fixtures/kernel-ready/drive
read -r -a RUNS <<<"$(param runtimes '')"
read -r -a WANTS <<<"$(param wants '')"
[ "${#RUNS[@]}" -ge 1 ] && [ "${#RUNS[@]}" = "${#WANTS[@]}" ] || fail "runtimes and wants: one want a Runtime"
TENSORFS=$(wheel_from "$OUT/wheels/tensorfs" "$(param tensorfs '')") && [ -n "$TENSORFS" ] || fail "no TensorFS wheel"
WHEELS=()
for i in "${!RUNS[@]}"; do
  w=$(wheel_from "$OUT/wheels/runtime-$i" "${RUNS[$i]}") && [ -n "$w" ] || fail "no Runtime wheel from run ${RUNS[$i]}"
  WHEELS+=("$w")
done
RUNTIME=${WHEELS[0]}
say "tensorfs ${TENSORFS##*/} · runtimes $(for w in "${WHEELS[@]}"; do printf '%s ' "${w##*/}"; done)· cozy $(cozy --version)"
. "$FIX/preflight.sh"
drive_check "$DRIVE" kernel_ready_drive
drive_lock "$DRIVE"
pick_offer
quote "$(param seconds 1500)"
offer_rent
cozy machine show "$NAME" --json > "$OUT/show.json" 2>&1
say "on: $(jq -r '[.status.gpus[] | "\(.name), driver \(.driver // "?")"] | join(", ")' "$OUT/show.json")"
step tensord machine update "$NAME" --tensord "$(command -v cozy)" -y || fail "the pod's TensorD"
(while sleep 480; do cozy machine keepalive "$NAME" > /dev/null 2>&1; done) & BG="$BG $!"
FAILED=()
for i in "${!RUNS[@]}"; do
  role=change; [ "$i" = 0 ] || role=red-$i
  step "candidate-$i" machine update "$NAME" --tensorfs "$TENSORFS" --runtime "${WHEELS[$i]}" -y || fail "$NAME on ${WHEELS[$i]##*/}"
  pod_install "stage-$i" "$DRIVE"
  say "$role: ${WHEELS[$i]##*/} (run ${RUNS[$i]}), wants ${WANTS[$i]}"
  for m in lands fails; do
    n=$role-$m want=$(tr ',' '\n' <<<"${WANTS[$i]}" | sed -n "s/^$m=//p")
    [ -n "$want" ] || continue
    if step "$n" run "$DRIVE/engine_recapture" mode="$m" --on "$NAME"; then
      on_candidates "$n"
      k=$(runno "$n"); (cd "$RUN_PROJECT" && cozy --json --full run watch "$k" --once) > "$OUT/$n.json" 2> /dev/null
      got=PASS said=$(jq -c '.result.detail // "" | fromjson? | del(.calls) // .' "$OUT/$n.json" 2> /dev/null | cut -c1-600)
    else
      got=FAIL said=$(grep -ao 'RuntimeError: [^"]*' "$OUT/$n.txt" | tail -1 | cut -c1-600)
    fi
    say "$n: $got (wanted $want) · ${said:-?}"
    [ "$got" = "$want" ] || FAILED+=("$n")
  done
done
[ "${#FAILED[@]}" = 0 ] || fail "failed: ${FAILED[*]}"
say "PASS: every Runtime's modes as wanted"
