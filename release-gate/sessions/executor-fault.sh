# executor-fault: a GPU call whose executor can no longer settle keeps its stderr with the failure (gpu_service's
# unsettled branch; 40459633948 lost a dead follower's SIGABRT this way). One rental, driven from a runner; the session's
# cozy carries the candidate TensorD onto the pod. The weightless fixture (fixtures/cuda-fault) prints a marker to its
# stderr, then trips a device-side assert, which leaves its CUDA context poisoned: the same branch a dead follower takes
# (its fault is the latch here, the lost rank there; every rank writes the leader's stderr). Then a successor completes
# in a fresh process. params:
#   traffic_gb  GB the provider bills: the image and the fixture's environment (default 8)
. "$(dirname "$0")/lib.sh"
FIX=$(cd "$(dirname "$0")/fixtures" && pwd)
MARK=fault-$(date +%s)-$RANDOM
[ -f "$FIX/cuda-fault/cuda_fault/__init__.py" ] || fail "no fixture checkout"
quote "$(param traffic_gb 8)" 1500
rent

step update machine update "$NAME" --tensord "$(command -v cozy)" -y || fail "update"
v=$(cozy machine show "$NAME" --json | jq -r .status.version)
[ "$v" = "$(cozy --version | awk '{print $2}')" ] || fail "the pod runs TensorD $v, not the session's"
say "pod TensorD $v"

cd "$FIX" || fail "no fixtures"
step fault run ./cuda-fault/fault --on "$NAME" seed:=$RANDOM marker="$MARK"  # expected to fail
F=$(runno fault)
[ -n "$F" ] || fail "the fault call named no run"
cozy run show "$F" --once --json > "$OUT/fault.json" 2>&1
st=$(jq -r '.status // empty' "$OUT/fault.json")
code=$(jq -r '.reason.code // empty' "$OUT/fault.json")
say "fault run $F: $st ($code): $(jq -r '.reason.message // empty' "$OUT/fault.json" | cut -c1-200)"
[ "$st" = failed ] || fail "the fault run is $st, not failed"
# The marker exists only on the executor's stderr: before this change the reason carried the traceback alone.
jq -r '.reason.detail // empty' "$OUT/fault.json" > "$OUT/fault-detail.txt"
grep -q "$MARK" "$OUT/fault-detail.txt" || fail "the failure's detail lacks the executor's stderr ($MARK): $(head -c 400 "$OUT/fault-detail.txt")"
grep -qiE 'device-side assert|index out of bounds' "$OUT/fault-detail.txt" || fail "the detail lacks the CUDA assert: $(tail -c 400 "$OUT/fault-detail.txt")"
fpid=$(grep -oE "$MARK: pid [0-9]+" "$OUT/fault-detail.txt" | head -1 | awk '{print $3}')
cozy machine logs "$NAME" tensord -n 5000 > "$OUT/tensord.log" 2>&1
grep -q "$MARK" "$OUT/tensord.log" || fail "the machine log does not carry the executor's stderr ($MARK)"
say "detail and machine log carry the executor's stderr: $(grep -c . "$OUT/fault-detail.txt") lines; fault pid $fpid"

step ok run ./cuda-fault/ok --on "$NAME" seed:=$RANDOM || fail "the successor failed"
S=$(runno ok)
cozy run show "$S" --once --json > "$OUT/ok.json" 2>&1
[ "$(jq -r '.status // empty' "$OUT/ok.json")" = completed ] || fail "successor $S is $(jq -r .status "$OUT/ok.json")"
spid=$(jq -r '.result.pid // (.result | fromjson? | .pid) // empty' "$OUT/ok.json")
{ [ -n "$spid" ] && [ "$spid" != "$fpid" ]; } || fail "the successor did not run in a fresh process ($fpid -> $spid)"
say "PASS: run $F failed with its stderr in the reason and the machine log; successor $S completed in pid $spid (fault pid $fpid)"
