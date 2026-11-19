# launcher-probe (xs-213): on one rental of $SKU whose driver is `driver` (a prefix, 615.; any other ends the rental
# at once), which of the engine's launcher paths the driver runs and which crash it, each in its own process. It
# fails unless the real 9B's bucket ran launched, its tokens equal to the replayed path's and every clear position
# agreeing (the launcher's body lowered to kernels, cozy-runtime #1860's way):
#   engine-loop-<section>  the varena suite's engine-loop, one section a process: loop, polled, attention, ggml,
#                          copies (the WHILE-node launcher and its host ring), replayed (a captured step replayed from
#                          the host, no WHILE node); a section that crashes its process says its exit (-11: SIGSEGV);
#   bucket-replayed        the real 9B's four rows through the decoder on the host-replayed path (`launcher=False`),
#                          against Transformers' teacher-forced forward;
#   bucket                 the same on the launcher's WHILE node, last (xs-213: it crashed driver 615.71.09).
# It fails only where it can't tell: a section that crashes is what it reports, not a failure of its own.
# params: tensorfs, runtime (required), driver (615.), seconds (1200), skus and quoted (preflight.sh).
. "$(dirname "$0")/lib.sh"
HERE=$(cd "$(dirname "$0")" && pwd)
FIX=$HERE/fixtures/ggml-decode
VARENA=$(cd "$HERE/../suites/varena/drive" && pwd)
DRIVER=$(param driver 615.)
Q8=qwen/qwen3.5-9b@sha256:c1059b7b191259ecd579df4640b6abd4e419aa25761135f0bd12c8e4b040393c
TENSORFS=$(wheel_from "$OUT/wheels/tensorfs" "$(param tensorfs '')") && [ -n "$TENSORFS" ] || fail "no TensorFS wheel"
RUNTIME=$(wheel_from "$OUT/wheels/runtime" "$(param runtime '')") && [ -n "$RUNTIME" ] || fail "no Runtime wheel"
say "tensorfs ${TENSORFS##*/} · runtime ${RUNTIME##*/} · cozy $(cozy --version) · wanted driver ${DRIVER}*"
. "$FIX/preflight.sh"
drive_check "$FIX/drive" ggml_logits
drive_lock "$FIX/drive"
drive_check "$VARENA" varena_drive
drive_lock "$VARENA"
pick_offer
quote "$(param seconds 1200)"
offer_rent
cozy machine show "$NAME" --json > "$OUT/show.json" 2>&1
driver=$(jq -r '[.status.gpus[]?.driver] | first // ""' "$OUT/show.json")
say "on: $(jq -r '[.status.gpus[] | .name] | join(", ")' "$OUT/show.json") · driver ${driver:-unknown}"
[[ $driver == "$DRIVER"* ]] || fail "driver ${driver:-unknown}, not ${DRIVER}*: nothing to probe here (the rental ends now)"
step tensord machine update "$NAME" --tensord "$(command -v cozy)" -y || fail "the pod's TensorD"
step candidate machine update "$NAME" --tensorfs "$TENSORFS" --runtime "$RUNTIME" -y || fail "the pod's TensorFS and Runtime"
(while sleep 480; do cozy machine keepalive "$NAME" > /dev/null 2>&1; done) & BG="$BG $!"
record() { local n; n=$(runno "$1"); [ -n "$n" ] && (cd "$RUN_PROJECT" && cozy --json --full run watch "$n" --once) > "$OUT/$1.json" 2> /dev/null; }
pod_install stage-varena "$VARENA"
for s in loop polled attention ggml copies replayed; do
  c=engine-loop-$s
  if step "$c" run "$VARENA/core" "case=$c" --on "$NAME"; then
    n=$(runno "$c"); (cd "$RUN_PROJECT" && cozy --json --full run watch "$n" --once) 2> /dev/null | jq -r '.result.detail' > "$OUT/$c-case.json"
    say "$c: ran · $(jq -c '.rows[0] | del(.case, .card)' "$OUT/$c-case.json" 2> /dev/null | cut -c1-600)"
  else
    say "$c: FAILED · exit $(grep -ao '"exit": -\?[0-9]*' "$OUT/$c.txt" | tail -1 | grep -o -- '-\?[0-9]*$') · $(grep -ao 'failed \[[a-z_]*\][^.]*' "$OUT/$c.txt" | tail -1 | cut -c1-300)"
  fi
done
step pull-q8 model install "$Q8" --on "$NAME" || fail "pull q8_0"
for b in bucket-replayed bucket; do
  entry=${b//-/_}
  if step "$b" run "$FIX/drive/$entry" --model "model=$Q8" --on "$NAME"; then
    record "$b"
    jq -r '.result.detail // ""' "$OUT/$b.json" > "$OUT/$b-detail.json"
    say "$b: $(jq -c '{declared, launcher, drafted_launched, plain_launched, drafted_digest, plain_digest, drafted: [.drafted[]? | [.bucket.agreed, .bucket.clear, .alone.agreed, .alone.clear]], plain: [.plain[]? | [.bucket.agreed, .bucket.clear, .alone.agreed, .alone.clear]]}' "$OUT/$b-detail.json" 2> /dev/null)"
  else
    say "$b: FAILED · $(grep -ao 'failed \[[a-z_]*\][^.]*\.[^.]*' "$OUT/$b.txt" | tail -1 | cut -c1-300)"
  fi
done
# The judged path must be the launcher (its check's copies section passing on this driver), and its tokens the
# replayed path's, every clear position agreeing with Transformers.
FAILED=()
jq -e '.launcher and .drafted_launched and .plain_launched' "$OUT/bucket-detail.json" > /dev/null 2>&1 || FAILED+=("not-launched")
jq -e 'all([(.drafted // [])[], (.plain // [])[]][]; .bucket.agreed == .bucket.clear and .alone.agreed == .alone.clear)' \
  "$OUT/bucket-detail.json" > /dev/null 2>&1 || FAILED+=("bucket-forced")
same=$(jq -rn --slurpfile a "$OUT/bucket-detail.json" --slurpfile b "$OUT/bucket-replayed-detail.json" \
  '($a[0].drafted_digest == $b[0].drafted_digest and $a[0].plain_digest == $b[0].plain_digest)' 2> /dev/null)
say "bucket (the judged path) and bucket-replayed made the same tokens: ${same:-?}"
[ "$same" = true ] || FAILED+=("tokens")
[ "${#FAILED[@]}" = 0 ] || fail "failed: ${FAILED[*]}"
say "DONE: each path's outcome on driver $driver above"
