# kernel-ready-cpu (xs-213): the Runtime's kernel-readiness exerciser (`bench.kernel_ready`, fixtures/kernel-ready) on this
# runner's own machine (`local`: no GPU, no rental), a real `cozy run … --on local` a mode, on each Runtime of `runtimes` in
# turn. The first is the change, which passes every mode; any after it is its red twin (the change reverted), which must
# fail each mode of `red` and pass the rest.
#   building-error  a build fails before it ever served: a capture asked while it built is refused (`Building`), one asked
#                   after records the fallback, and the epoch moves once, so the step graphs keyed on it record again;
#   late-success, late-error, dropped-slot: two openers of one kernel (internal/bench/kernel_ready.py).
# params: tensorfs, runtimes (required: dev-build runs, space-separated), modes (all four), red (building-error).
. "$(dirname "$0")/lib.sh"
HERE=$(cd "$(dirname "$0")" && pwd)
DRIVE=$HERE/fixtures/kernel-ready/drive
. "$HERE/fixtures/ggml-decode/preflight.sh"  # on_candidates: a run on the candidate wheels, not its lock's
MODES=$(param modes "building-error late-success late-error dropped-slot") RED=$(param red building-error)
read -r -a RUNS <<<"$(param runtimes '')"
[ "${#RUNS[@]}" -ge 1 ] || fail "no runtimes"
TENSORFS=$(wheel_from "$OUT/wheels/tensorfs" "$(param tensorfs '')") && [ -n "$TENSORFS" ] || fail "no TensorFS wheel"
for _ in $(seq 60); do
  cozy --json machine show local > "$OUT/local.json" 2> "$OUT/local.err"
  [ "$(jq -r '.status.phase // empty' "$OUT/local.json" 2> /dev/null)" = ready ] && break
  sleep 5
done
[ "$(jq -r '.status.phase // empty' "$OUT/local.json")" = ready ] || fail "local isn't ready: $(tail -c 300 "$OUT/local.err")"
[ "$(jq '.status.gpus // [] | length' "$OUT/local.json")" = 0 ] || fail "local has a GPU: these modes are the CPU's"
(cd "$RUN_PROJECT" && cozy --json machine use local) > "$OUT/use.json" 2>&1 || fail "machine use local: $(tail -c 300 "$OUT/use.json")"
FAILED=()
for i in "${!RUNS[@]}"; do
  role=change; [ "$i" = 0 ] || role=red-$i
  RUNTIME=$(wheel_from "$OUT/wheels/runtime-$i" "${RUNS[$i]}") && [ -n "$RUNTIME" ] || fail "no Runtime wheel from run ${RUNS[$i]}"
  step "candidate-$i" machine update local --tensorfs "$TENSORFS" --runtime "$RUNTIME" -y || fail "local on ${RUNTIME##*/}"
  say "$role: ${RUNTIME##*/} (run ${RUNS[$i]}) · TensorFS ${TENSORFS##*/}"
  for m in $MODES; do
    n=$role-$m want=PASS
    [ "$role" = change ] || [[ " $RED " != *" $m "* ]] || want=FAIL
    if step "$n" run "$DRIVE/kernel_ready" mode="$m" --on local; then
      on_candidates "$n"
      k=$(runno "$n"); (cd "$RUN_PROJECT" && cozy --json --full run watch "$k" --once) > "$OUT/$n.json" 2> /dev/null
      got=PASS said=$(jq -r '.result.detail // ""' "$OUT/$n.json" 2> /dev/null | cut -c1-500)
    else
      got=FAIL said=$(grep -ao 'RuntimeError: [^"]*' "$OUT/$n.txt" | tail -1 | cut -c1-500)
    fi
    say "$n: $got (wanted $want) · ${said:-?}"
    [ "$got" = "$want" ] || FAILED+=("$n")
  done
done
[ "${#FAILED[@]}" = 0 ] || fail "failed: ${FAILED[*]}"
say "PASS: the change passed every mode; each red twin failed only its red modes"
