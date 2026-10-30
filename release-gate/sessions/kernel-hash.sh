# kernel-hash (proof-only, never merge): kernels/kernel_hash.py on one rental under two Runtimes, the release and the
# candidate whose kernel pins moved: each third-party kernel's output digest, op by op, on the same card and inputs.
# Equal digests are equal bytes; an op that does not repeat its own bytes is compared by its error instead. params:
#   runtimes   {"a": ..., "b": ...}: a release the machine fetches (`0.35.6`), or a cozy-runtime dev-build's manylinux
#              x86_64 wheel (`run:<run id>/<commit>`)
#   tensorfs   the TensorFS both run with (0.17.13)
#   traffic_gb GB the provider bills (8); need_s seconds the guard must buy (2400)
. "$(dirname "$0")/lib.sh"
RUNTIMES=$(param runtimes '{}') DIR=$(dirname "$0")/kernels
[ -f "$DIR/kernel_hash.py" ] || fail "no kernels/kernel_hash.py"
declare -A WANT
for arm in a b; do
  src=$(jq -r --arg a "$arm" '.[$a] // empty' <<<"$RUNTIMES")
  if [[ $src =~ ^run:([0-9]+)/([0-9a-f]{40})$ ]]; then
    mkdir -p "$OUT/wheel-$arm"
    gh run download "${BASH_REMATCH[1]}" -R cozy-creator/cozy-runtime -n "cozy-runtime-${BASH_REMATCH[2]}" -D "$OUT/wheel-$arm" >/dev/null 2>&1 ||
      fail "arm $arm: no artifact in run ${BASH_REMATCH[1]}"
    WANT[$arm]=$(ls "$OUT"/wheel-$arm/cozy_runtime-*manylinux*x86_64.whl 2>/dev/null | head -1)
    [ -f "${WANT[$arm]}" ] || fail "arm $arm: no manylinux x86_64 wheel"
  elif [[ $src =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]]; then
    WANT[$arm]=$src
  else
    fail "arm $arm: runtime '$src'"
  fi
  say "arm $arm: Runtime $(basename "${WANT[$arm]}")"
done
quote "$(param traffic_gb 8)" "$(param need_s 2400)"
rent
cd "$DIR" || fail "no kernels/"
for arm in a b; do
  step "update-$arm" machine update "$NAME" --runtime "${WANT[$arm]}" --tensorfs "$(param tensorfs 0.17.13)" -y || fail "update to arm $arm"
  cozy machine show "$NAME" 2>&1 | grep -E '^(runtime|tensorfs|GPU) ' | tee -a "$R"
  step "hash-$arm" run ./kernel_hash.py --gpus=1 --on "$NAME" || say "WARNING hash-$arm: $(tail -c 600 "$OUT/hash-$arm.txt" | tr '\n' ' ')"
  n=$(runno "hash-$arm"); [ -z "$n" ] || cozy run show "$n" --once --json > "$OUT/hash-$arm.json" 2>&1
  jq -r '.result.value' "$OUT/hash-$arm.json" > "$OUT/report-$arm.json" 2>/dev/null || fail "arm $arm returned no report"
  say "arm $arm: $(jq -c '{gpu, sm, cozy_runtime, seconds, ops: (.ops | length), errors: [.ops | to_entries[] | select(.value.error) | .key]}' "$OUT/report-$arm.json")"
done
for r in "${RIDS[@]}"; do cozy machine end "$r" -y >> "$OUT/end.txt" 2>&1 || cozy machine end "$r" -y --force >> "$OUT/end.txt" 2>&1; done
say "END requested after the last run"
jq -n --slurpfile a "$OUT/report-a.json" --slurpfile b "$OUT/report-b.json" '
  $a[0].ops as $x | $b[0].ops as $y |
  [($x | keys[]) as $k | {op: $k, a: $x[$k], b: ($y[$k] // {error: "absent"})} |
   .verdict = (if (.a.error and .b.error) then "unavailable" elif (.a.error // .b.error) then "error"
               elif (.a.repeats and .b.repeats) then (if .a.sha == .b.sha then "identical" else "DIFFERS" end)
               else "nondeterministic" end)]' > "$OUT/compare.json"
jq -r '.[] | "\(.verdict)\t\(.op)\t\(.a.sha // .a.error)\t\(.b.sha // .b.error)\trel_l2 \(.a.rel_l2 // "-") -> \(.b.rel_l2 // "-")"' "$OUT/compare.json" | tee -a "$R"
say "verdicts: $(jq -c 'group_by(.verdict) | map({(.[0].verdict): length}) | add' "$OUT/compare.json")"
jq -e 'all(.[]; .verdict == "identical" or .verdict == "unavailable") and any(.[]; .verdict == "identical")' "$OUT/compare.json" >/dev/null ||
  fail "not every op is byte-identical (see compare.json)"
say "PASS"
