# Suite `escaped` (gate.sh --suite escaped): every first-party package's cells (products.sh) at a 6 GiB cap, so a
# package that keeps device memory across its steps as plain tensors, outside the memory manager, cannot ship; then a
# cell that failed or was refused memory runs again uncapped, so one that sizes its work from the card cannot either.
# check.py reads each run's `escaped` record (varena's tally of what a model's passes left live, by birth site) and
# fails a cell whose undeclared escaped bytes pass BOUND or grow by more than GROWTH a pass. Cross-step state belongs
# in `author.retained`, which varena spills and refills. A cell parked in known-failing.json for this SKU is skipped.
cap() {
  "$CLI" machine update "$M" --gpu-memory-limit "$1" -y >"cap-$1.log" 2>&1
  local i said want=0
  [ "$1" = off ] || want=$(( ${1%GiB} << 30 ))
  for i in $(seq 12); do
    said=$("$CLI" --json machine show "$M" 2>/dev/null | jq -r '[.status.gpus[]? | .memory_limit_bytes // 0 | tostring] | unique | join(",")')
    [ "$said" = "$want" ] && break
    sleep 5
  done
  log "cap $1 on $M: machine show says ${said:-nothing} bytes"
  [ "$said" = "$want" ] || expect "cap-$1" FAIL "machine show says ${said:-nothing} bytes, not $want"
}
cap 6GiB
for p in "${PRODUCTS[@]}"; do product "$p"; done
cap off
# refused <cell>: its capped run failed or has no record, or varena refused it memory in this request (a split level
# refused for good, a Runtime Retry, a loop's refused pass, an OOM the package returned past).
refused() {
  [ -f "$1.json" ] || return 0
  jq -e '(.status != "completed") or ([.measurements, .calls[]?.measurements] | map(.memory.stats.varena[]?.stats // {})
    | any(((.retries // 0) + (.swallowed_ooms // 0)) > 0 or any(.working_splits[]?; .refused != null)
          or any(.escaped[]?; .refused != null)))' "$1.json" >/dev/null 2>&1
}
# Each such cell again uncapped on the same pod (<cell>-uncapped), which tells a capacity failure from a bug: a package
# that sizes from the physical card, not varena's room, fails capped and passes here. A passing capped cell needs no
# twin. check.py compares success, the capped peak and the kernels, never the bytes.
TWINS=$(cut -f1 cells.tsv | while read -r c; do refused "$c" && echo "$c"; done | paste -sd, -)
log "escaped: uncapped twins for ${TWINS:-no cell}"
if [ -n "$TWINS" ]; then
  TWIN=-uncapped
  for p in "${PRODUCTS[@]}"; do product "$p"; done
  TWIN=
fi
