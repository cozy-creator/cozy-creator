# kernel-hash (proof-only, never merge): kernels/kernel_hash.py on one rental under two Runtimes, the release and the
# candidate whose kernel pins moved: each third-party kernel's output digest, op by op, on the same card and inputs.
# Equal digests are equal bytes; an op that does not repeat its own bytes is compared by its error instead
# (kernels/hash.sh). params:
#   runtimes   {"a": ..., "b": ..., …}: a release the machine fetches (`0.35.6`), or a cozy-runtime dev-build's manylinux
#              x86_64 wheel (`run:<run id>/<commit>`); run in name order on the one pod
#   mode       hash (compare a against b op by op) or order (each arm a request after the one before it: PASS when
#              every arm reports, none ended by a device fault) (hash)
#   tensorfs   the TensorFS every arm runs with (0.17.13)
#   traffic_gb GB the provider bills (8); need_s seconds the guard must buy (2400)
. "$(dirname "$0")/lib.sh"
. "$(dirname "$0")/kernels/hash.sh"
hash_arms "$(param runtimes '{}')" > "$OUT/arms.txt" || fail "$(cat "$OUT/arms.txt")"
MODE=$(param mode hash)
[[ $MODE == hash || $MODE == order ]] || fail "mode"
for arm in "${HASH_ARMS[@]}"; do say "arm $arm: ${HASH_WANT[$arm]##*/}"; done
quote "$(param traffic_gb 8)" "$(param need_s 2400)"
rent
faults=()
for arm in "${HASH_ARMS[@]}"; do
  hash_run "$arm" "$(param tensorfs 0.17.13)" && continue
  [ "$MODE" = order ] || fail "arm $arm returned no report"
  faults+=("$arm"); say "arm $arm: no report ($(jq -r '.reason // .status' "$OUT/hash-$arm.json" 2>/dev/null | head -c 300))"
done
for r in "${RIDS[@]}"; do cozy machine end "$r" -y >> "$OUT/end.txt" 2>&1 || cozy machine end "$r" -y --force >> "$OUT/end.txt" 2>&1; done
say "END requested after the last run"
if [ "$MODE" = order ]; then
  [ "${#faults[@]}" = 0 ] || fail "arms without a report: ${faults[*]}"
else
  hash_compare || fail "not every op is byte-identical (see compare.json)"
fi
say "PASS"
