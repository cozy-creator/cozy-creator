# kernel-hash (proof-only, never merge): kernels/kernel_hash.py on one rental under two Runtimes, the release and the
# candidate whose kernel pins moved: each third-party kernel's output digest, op by op, on the same card and inputs.
# Equal digests are equal bytes; an op that does not repeat its own bytes is compared by its error instead
# (kernels/hash.sh). params:
#   runtimes   {"a": ..., "b": ...}: a release the machine fetches (`0.35.6`), or a cozy-runtime dev-build's manylinux
#              x86_64 wheel (`run:<run id>/<commit>`)
#   tensorfs   the TensorFS both run with (0.17.13)
#   traffic_gb GB the provider bills (8); need_s seconds the guard must buy (2400)
. "$(dirname "$0")/lib.sh"
. "$(dirname "$0")/kernels/hash.sh"
hash_arms "$(param runtimes '{}')" > "$OUT/arms.txt" || fail "$(cat "$OUT/arms.txt")"
say "arms: a ${HASH_WANT[a]##*/}, b ${HASH_WANT[b]##*/}"
quote "$(param traffic_gb 8)" "$(param need_s 2400)"
rent
for arm in a b; do hash_run "$arm" "$(param tensorfs 0.17.13)" || fail "arm $arm returned no report"; done
for r in "${RIDS[@]}"; do cozy machine end "$r" -y >> "$OUT/end.txt" 2>&1 || cozy machine end "$r" -y --force >> "$OUT/end.txt" 2>&1; done
say "END requested after the last run"
hash_compare || fail "not every op is byte-identical (see compare.json)"
say "PASS"
