# Sourced by kernel-hash.sh and bench.sh (proof-only): kernel_hash.py under two Runtimes on $NAME, compared op by op.
# hash_arms <{"a": ..., "b": ..., …}>: each arm's Runtime, before renting: a release the machine fetches (`0.35.6`) or a
# cozy-runtime dev-build's manylinux x86_64 wheel (`run:<run id>/<commit>`), downloaded here; sets HASH_WANT and
# HASH_ARMS (the arms in name order: a runs first, after the TensorFS pairing).
declare -A HASH_WANT
HASH_ARMS=()
hash_arms() {
  local arm src
  mapfile -t HASH_ARMS < <(jq -r 'keys[]' <<<"$1")
  [ "${HASH_ARMS[0]:-}" = a ] || { echo "no arm a"; return 1; }
  for arm in "${HASH_ARMS[@]}"; do
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
# Arm a pairs the TensorFS release; arm b keeps it (a dev Runtime with a TensorFS version resolves it on PyPI alone).
hash_run() {
  local arm=$1 n
  if [ "$arm" = a ]; then
    # The TensorFS release pairs with a released Runtime; a dev Runtime then keeps it (with a version it resolves PyPI alone).
    step pair machine update "$NAME" --runtime "${HASH_PAIR:-0.35.6}" --tensorfs "$2" -y || return 1
  fi
  [ "${HASH_WANT[$arm]}" = "${HASH_PAIR:-0.35.6}" ] && [ "$arm" = a ] ||
    step "update-$arm" machine update "$NAME" --runtime "${HASH_WANT[$arm]}" -y || return 1
  cozy machine show "$NAME" 2>&1 | grep -E '^(runtime|tensorfs|GPU) ' | tee -a "$R"
  # A fresh pod's first environment install prints nothing while it downloads: 30 min of silence, not lib.sh's 15.
  say "step hash-$arm: cozy run ./kernel_hash.py --gpus=1 --on $NAME"
  (cd "$(dirname "${BASH_SOURCE[0]}")" && exec cozy run ./kernel_hash.py --gpus=1 --on "$NAME" ${HASH_ARGS:-}) > "$OUT/hash-$arm.txt" 2>&1 &
  local p=$! last=-1 still=0 size
  while kill -0 "$p" 2>/dev/null; do
    sleep 30; size=$(stat -c %s "$OUT/hash-$arm.txt")
    if [ "$size" = "$last" ]; then still=$((still + 30)); else still=0 last=$size; fi
    [ "$still" -lt 1800 ] || { kill "$p"; say "hash-$arm: no output for 30 min"; break; }
  done
  wait "$p" || say "WARNING hash-$arm: $(tr '\r' '\n' < "$OUT/hash-$arm.txt" | tail -c 600 | tr '\n' ' ')"
  local install; install=$(grep -oE 'run [0-9]+ · install' "$OUT/hash-$arm.txt" | head -1 | awk '{print $2}')
  [ -z "$install" ] || say "hash-$arm: environment install run $install took $(cozy run show "$install" --once --json 2>/dev/null |
    jq -r '((.settled_at_ms // 0) - (.accepted_at_ms // .created_at_ms // 0)) / 1000 | floor') s"
  n=$(runno "hash-$arm"); [ -n "$n" ] || return 1
  cozy run show "$n" --once --json > "$OUT/hash-$arm.json" 2>&1
  jq -r '.result.value' "$OUT/hash-$arm.json" > "$OUT/report-$arm.json" 2>/dev/null && jq -e .ops "$OUT/report-$arm.json" >/dev/null || return 1
  say "arm $arm: $(jq -c '{gpu, sm, cozy_runtime, seconds, ops: (.ops | length), errors: [.ops | to_entries[] | select(.value.error) | .key]}' "$OUT/report-$arm.json")"
}
# hash_compare: $OUT/compare.json and a verdict a line; true when every op both arms ran gave the same bytes.
hash_compare() {
  jq -e -n --slurpfile a "$OUT/report-a.json" --slurpfile b "$OUT/report-b.json" '$a[0].inputs == $b[0].inputs' >/dev/null ||
    { say "the arms' inputs differ: $(jq -c .inputs "$OUT/report-a.json") / $(jq -c .inputs "$OUT/report-b.json")"; return 1; }
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
