# gpu-queue (cl-1077, chartreux 11-16: LTX 2.5, accepted 07:49:30, waited while LAB2-47843, an H3 `edit` accepted at 07:59:04, took
# the GPU: "looks all messed up and discombobulated … it's not organizing things well"): one rental, red then green, each arm's TensorD
# put on with `machine update --tensord`. `hold` keeps the pod's one GPU busy 90 s. Meanwhile `early`, a plain GPU run, is accepted and
# queues; then `late`, an orchestrator on the CPU, calls `late_call` on the GPU, which queues too. When the GPU frees, the earlier run
# goes first (green). Red offers every call of a started run first, so the later run's call goes first. fixtures/gpu-queue/gpuq.py.
# params:
#   red   the dev-build run id whose cozy is red's TensorD; green's is the session's cozy (a dev build, pod-session's `cozy`)
. "$(dirname "$0")/lib.sh"
RED_RUN=$(param red)
[[ $RED_RUN =~ ^[0-9]+$ ]] || fail "red is not a dev-build run id: '$RED_RUN'"
mkdir -p "$OUT/red-cozy"
gh run download "$RED_RUN" -R cozy-creator/cozy-creator -p 'cozy-*' -D "$OUT/red-cozy" > /dev/null 2>&1 || fail "red's cozy (dev-build $RED_RUN) wasn't fetched"
RED=$(find "$OUT/red-cozy" -type f -name 'cozy-linux-x86_64' | head -1)
[ -n "$RED" ] && chmod +x "$RED" || fail "dev-build $RED_RUN holds no cozy-linux-x86_64"
GREEN=$(command -v cozy)
FIX=$(cd "$(dirname "$0")/fixtures/gpu-queue" && pwd)/gpuq.py
say "red TensorD $("$RED" --version), green $("$GREEN" --version)"
quote 2400
rent

# taken <name>: run <name> of this session accepted by the pod.
taken() {
  local i
  for i in $(seq 300); do
    (cd "$RUN_PROJECT" && cozy --json run watch "$(run_ref "$OUT" "$1" "$(cat "$OUT/$1.number")")" --once 2> /dev/null) |
      jq -e '.accepted_at_ms != null' > /dev/null && return 0
    sleep 1
  done
  return 1
}

declare -A FIRST
arm() {
  local a=$1 bin=$2 v n name e l
  # The pod keeps its image's released Runtime (as prepare-ahead's): the RC's pure wheel carries no kernels (runtime_without_kernels).
  step "$a-tensord" machine update "$NAME" --tensord "$bin" -y || fail "$a: the pod refused its TensorD"
  v=$(cozy machine show "$NAME" --json | jq -r .status.version)
  [ "$v" = "$("$bin" --version | awk '{print $2}')" ] || fail "$a: the pod runs TensorD $v, not $("$bin" --version)"
  say "$a: TensorD $v"
  # Each function once, so its environment and model are here and the measured runs wait on nothing but the GPU.
  for name in hold early late; do step "$a-warm-$name" run "$FIX/$name" --on "$NAME" || fail "$a: $name didn't run"; done
  n=$(run_submit "$OUT" "$a-hold" run "$FIX/hold" seconds:=90 --on "$NAME")
  [ -n "$n" ] || fail "$a: hold wasn't submitted: $(tail -c 300 "$OUT/$a-hold.err")"
  for _ in $(seq 600); do grep -q '"holding"' "$OUT/$a-hold.ndjson" 2> /dev/null && break; sleep 0.5; done
  grep -q '"holding"' "$OUT/$a-hold.ndjson" || fail "$a: hold never held the GPU"
  n=$(run_submit "$OUT" "$a-early" run "$FIX/early" --on "$NAME")
  [ -n "$n" ] && taken "$a-early" || fail "$a: early wasn't accepted: $(tail -c 300 "$OUT/$a-early.err")"
  n=$(run_submit "$OUT" "$a-late" run "$FIX/late" --on "$NAME")
  [ -n "$n" ] && taken "$a-late" || fail "$a: late wasn't accepted: $(tail -c 300 "$OUT/$a-late.err")"
  for name in hold early late; do
    run_wait "$OUT" "$a-$name" || fail "$a: $name did not end: $(tail -c 300 "$OUT/$a-$name.ndjson")"
    [ "$(run_get "$OUT" "$a-$name" .status)" = completed ] || fail "$a: $name $(run_get "$OUT" "$a-$name" .status): $(run_get "$OUT" "$a-$name" .reason.message | head -c 300)"
  done
  # Each function says when it ran on the pod's clock; `late` says its call's.
  e=$(run_get "$OUT" "$a-early" .result.done_ms) l=$(run_get "$OUT" "$a-late" .result.done_ms)
  [ -n "$e" ] && [ -n "$l" ] || fail "$a: no done_ms in early ($e) or late ($l)"
  FIRST[$a]=$([ "$e" -lt "$l" ] && echo early || echo late)
  say "$a: after hold, ${FIRST[$a]} took the GPU first (early ran at $e, late's call at $l, pod ms; hold ended $(run_get "$OUT" "$a-hold" .result.done_ms))"
}
arm red "$RED"
arm green "$GREEN"
[ "${FIRST[red]}" = late ] || fail "inconclusive: red's GPU queue let the earlier run go first too"
[ "${FIRST[green]}" = early ] || fail "green: the later run's call took the GPU before the earlier run"
say "PASS: red offered the later orchestrator's call first; green, the earlier run"
