# launcher-probe (xs-213): on one rental of $SKU whose driver is `driver` (a prefix, 615.; any other ends the rental
# at once), which of the engine's launcher paths the driver runs and which crash it, each in its own process. It
# fails unless the real 9B ran launched, its tokens equal to the replayed path's and every clear position agreeing (the
# launcher's body lowered to kernels, cozy-runtime #1860's way):
#   engine-loop-<section>  the varena suite's engine-loop, one section a process: loop, polled, attention, ggml,
#                          copies (the WHILE-node launcher and its host ring), replayed (a captured step replayed from
#                          the host, no WHILE node); a section that crashes its process says its exit (-11: SIGSEGV);
#   decode-paths           the real 9B's 512-token decode launched and host-replayed, back to back, warm: launched
#                          must make the replayed tokens and be no more than 5% slower (the decode drive's `engine`);
#   bucket                 its four rows through the decoder on the path the engine judged (the launcher), against
#                          Transformers' teacher-forced reads.
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
    say "$c: FAILED · exit $(grep -ao '"exit": -\?[0-9]*' "$OUT/$c.txt" | tail -1 | grep -o -- '-\?[0-9]*$') · $(grep -ao '"wrong": \[[^]]*\]' "$OUT/$c.txt" | head -1 | cut -c1-400)"
  fi
done
step pull-q8 model install "$Q8" --on "$NAME" || fail "pull q8_0"
FAILED=()
# The same 512-token decode launched and host-replayed, back to back in one process, warm (the decode drive's
# `engine`): launched must make the replayed tokens and be no more than 5% slower.
if step decode-paths run "$FIX/drive/engine" --model "model=$Q8" --on "$NAME"; then
  record decode-paths
  jq -r '.result.detail // ""' "$OUT/decode-paths.json" > "$OUT/decode-paths-detail.json"
  for mode in drafted plain; do
    say "decode $mode: launched $(jq -r --arg m "$mode" '.[$m] | "\(.ms_step) ms a step over \(.steps) steps (launcher \(.launcher))"' "$OUT/decode-paths-detail.json") · replayed $(jq -r --arg m "$mode-replayed" '.[$m] | "\(.ms_step) ms a step, its tokens equal launched for \(.same_as_launched)"' "$OUT/decode-paths-detail.json")"
  done
  jq -e '[.drafted, .plain] | all(.launcher == true)' "$OUT/decode-paths-detail.json" > /dev/null 2>&1 || FAILED+=("decode-not-launched")
  jq -e '[["drafted", "drafted-replayed"], ["plain", "plain-replayed"]] as $pairs | . as $d
    | all($pairs[]; $d[.[1]].same_as_launched == $d[.[0]].tokens and $d[.[0]].ms_step <= 1.05 * $d[.[1]].ms_step)' \
    "$OUT/decode-paths-detail.json" > /dev/null 2>&1 || FAILED+=("decode-paths")
else
  FAILED+=("decode-paths")
fi
# look <step> <began>: what the pod is doing during a long step, bounded: the step's last progress, GPU use, the
# busiest processes and TensorD's newest lines (44643016198's bucket said nothing for 16 min, then the deadline came).
look() {
  local at=$(($(date +%s) - $2)) f
  f=$OUT/$1-look-$at.txt
  timeout 90 cozy machine ssh "$NAME" "nvidia-smi --query-gpu=utilization.gpu,memory.used,power.draw --format=csv,noheader; top -b -n1 -o %CPU -w 220 | sed -n 8,12p" < /dev/null 2>&1 | grep -v '^Warning: Permanently added' > "$f"
  timeout 60 cozy machine logs "$NAME" -n 60 >> "$f" 2>&1
  say "$1 at ${at}s: $(grep -ao 'step [0-9]*/[0-9]*[^·]*' "$OUT/$1.txt" | tail -1) · gpu $(head -1 "$f" | cut -c1-60) · busiest $(sed -n 2p "$f" | awk '{print $9 "% cpu", $12, $13, $14}' | cut -c1-120)"
}
# The real 9B's four rows on the path the engine judged (the launcher, its body lowered to kernels), every clear
# position agreeing with Transformers; its stages are the run's progress, and the pod is looked at every 5 min after 10.
began=$(date +%s)
(sleep 600 && while :; do look bucket "$began"; sleep 300; done) & LOOK=$!
BG="$BG $LOOK"
if step bucket run "$FIX/drive/bucket" --model "model=$Q8" --on "$NAME"; then
  record bucket
  jq -r '.result.detail // ""' "$OUT/bucket.json" > "$OUT/bucket-detail.json"
  say "bucket: $(jq -c '{declared, launcher, drafted_launched, plain_launched, s, drafted: [.drafted[]? | [.bucket.agreed, .bucket.clear, .alone.agreed, .alone.clear]], plain: [.plain[]? | [.bucket.agreed, .bucket.clear, .alone.agreed, .alone.clear]]}' "$OUT/bucket-detail.json" 2> /dev/null)"
else
  say "bucket: FAILED · $(grep -ao 'failed \[[a-z_]*\][^.]*\.[^.]*' "$OUT/bucket.txt" | tail -1 | cut -c1-300)"
fi
pkill -P "$LOOK" 2> /dev/null; kill "$LOOK" 2> /dev/null
jq -e '.launcher and .drafted_launched and .plain_launched' "$OUT/bucket-detail.json" > /dev/null 2>&1 || FAILED+=("not-launched")
jq -e 'all([(.drafted // [])[], (.plain // [])[]][]; .bucket.agreed == .bucket.clear and .alone.agreed == .alone.clear)' \
  "$OUT/bucket-detail.json" > /dev/null 2>&1 || FAILED+=("bucket-forced")
[ "${#FAILED[@]}" = 0 ] || fail "failed: ${FAILED[*]}"
say "DONE: each path's outcome on driver $driver above"
