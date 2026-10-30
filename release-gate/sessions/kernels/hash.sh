# Sourced by kernel-hash.sh and bench.sh (proof-only): kernel_hash.py under two Runtimes on $NAME, compared op by op.
# hash_arms <{"a": ..., "b": ...}>: each arm's Runtime, before renting: a release the machine fetches (`0.35.6`) or a
# cozy-runtime dev-build's manylinux x86_64 wheel (`run:<run id>/<commit>`), downloaded here; sets HASH_WANT.
declare -A HASH_WANT
hash_arms() {
  local arm src
  for arm in a b; do
    src=$(jq -r --arg a "$arm" '.[$a] // empty' <<<"$1")
    if [[ $src =~ ^run:([0-9]+)/([0-9a-f]{40})$ ]]; then
      mkdir -p "$OUT/wheel-$arm"
      gh run download "${BASH_REMATCH[1]}" -R cozy-creator/cozy-runtime -n "cozy-runtime-${BASH_REMATCH[2]}" -D "$OUT/wheel-$arm" >/dev/null 2>&1 ||
        { echo "arm $arm: no artifact in run ${BASH_REMATCH[1]}"; return 1; }
      HASH_WANT[$arm]=$(ls "$OUT"/wheel-$arm/cozy_runtime-*manylinux*x86_64.whl 2>/dev/null | head -1)
      [ -f "${HASH_WANT[$arm]}" ] || { echo "arm $arm: no manylinux x86_64 wheel"; return 1; }
    elif [[ $src =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]]; then
      HASH_WANT[$arm]=$src
    else
      echo "arm $arm: runtime '$src'"; return 1
    fi
  done
}
# hash_run <arm> <tensorfs>: the pod on that arm's Runtime, then kernel_hash.py; its report in $OUT/report-<arm>.json.
hash_run() {
  local arm=$1 n
  step "update-$arm" machine update "$NAME" --runtime "${HASH_WANT[$arm]}" --tensorfs "$2" -y || return 1
  cozy machine show "$NAME" 2>&1 | grep -E '^(runtime|tensorfs|GPU) ' | tee -a "$R"
  (cd "$(dirname "${BASH_SOURCE[0]}")" && step "hash-$arm" run ./kernel_hash.py --gpus=1 --on "$NAME") ||
    say "WARNING hash-$arm: $(tail -c 600 "$OUT/hash-$arm.txt" | tr '\n' ' ')"
  n=$(runno "hash-$arm"); [ -n "$n" ] || return 1
  cozy run show "$n" --once --json > "$OUT/hash-$arm.json" 2>&1
  jq -r '.result.value' "$OUT/hash-$arm.json" > "$OUT/report-$arm.json" 2>/dev/null && jq -e .ops "$OUT/report-$arm.json" >/dev/null || return 1
  say "arm $arm: $(jq -c '{gpu, sm, cozy_runtime, seconds, ops: (.ops | length), errors: [.ops | to_entries[] | select(.value.error) | .key]}' "$OUT/report-$arm.json")"
}
# hash_compare: $OUT/compare.json and a verdict a line; true when every op both arms ran gave the same bytes.
hash_compare() {
  jq -n --slurpfile a "$OUT/report-a.json" --slurpfile b "$OUT/report-b.json" '
    $a[0].ops as $x | $b[0].ops as $y |
    [($x | keys[]) as $k | {op: $k, a: $x[$k], b: ($y[$k] // {error: "absent"})} |
     .verdict = (if (.a.error and .b.error) then "unavailable" elif (.a.error // .b.error) then "error"
                 elif (.a.repeats and .b.repeats) then (if .a.sha == .b.sha then "identical" else "DIFFERS" end)
                 else "nondeterministic" end)]' > "$OUT/compare.json" || return 1
  jq -r '.[] | "\(.verdict)\t\(.op)\t\(.a.sha // .a.error)\t\(.b.sha // .b.error)\trel_l2 \(.a.rel_l2 // "-") -> \(.b.rel_l2 // "-")"' "$OUT/compare.json" | tee -a "$R"
  say "verdicts: $(jq -c 'group_by(.verdict) | map({(.[0].verdict): length}) | add' "$OUT/compare.json")"
  jq -e 'all(.[]; .verdict == "identical" or .verdict == "unavailable") and any(.[]; .verdict == "identical")' "$OUT/compare.json" >/dev/null
}
