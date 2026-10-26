# capability: a pod renews its own run capability and outlives its runner's Calcifer (cl-675, AuthKit's workload
# grant). One rental, no GPU work:
#   1. the pod's TensorD becomes the session's cozy (`cozy machine update --tensord`, as the runner's already is);
#   2. a private commit the runner uploads from its own machine, read on the pod: model download --on;
#   3. an upload --on the pod: a private commit written under the pod's token;
#   4. the e2e cell (e2e/capabilities) with the rental live: renewal, a copied token, widened scope, the rental granted;
#   5. the executor probe (e2e/capabilities/probe.py), its weights a private commit: it waits probe_s while the
#      runner's Calcifer is stopped, then reads through the pod's gateway under a token the pod renewed by itself, and
#      reports what its executor holds;
#   6. the rental ends, and the cell's rental check again: refused.
# The throwaway models are deleted after. params:
#   probe_s    seconds the probe waits before it reads: past a token's 15 min (960)
#   off_s      seconds the runner's Calcifer stays stopped (1020)
#   traffic_gb GB the provider bills: the image and the session's cozy (12)
. "$(dirname "$0")/lib.sh"
PROBE_S=$(param probe_s 960) OFF_S=$(param off_s 1020)
CELL=$GITHUB_WORKSPACE/creator/e2e/capabilities
[ -f "$CELL/main.go" ] && [ -f "$CELL/probe.py" ] || fail "no e2e/capabilities checkout"
command -v go >/dev/null || fail "no go"
(cd "$CELL" && GOWORK=off go build -o "$OUT/capabilities" .) || fail "the cell does not build"
TS=$(date -u +%m%d%H%M) A=$(cozy --json whoami | jq -r .account)
[ -n "$A" ] && [ "$A" != null ] || fail "not signed in"
MA=$A/cl675-a-$TS MB=$A/cl675-b-$TS MC=$A/cl675-c-$TS
tiny() { python3 -I -c 'import json,struct,sys,os; h=json.dumps({"w":{"dtype":"F32","shape":[4,4],"data_offsets":[0,64]}}).encode(); h+=b" "*(-len(h)%8); open(sys.argv[1],"wb").write(struct.pack("<Q",len(h))+h+os.urandom(64))' "$1"; }
made() { awk '$1=="commit"{c=$2} END{sub(/.*@/, "", c); print c}' "$OUT/$1.txt"; }
cleanup() { for m in $MA $MB $MC; do cozy model delete "$m" -y >> "$OUT/delete.txt" 2>&1; done; }
quote "$(param traffic_gb 12)" $((OFF_S + 2400))
rent
trap 'cleanup; finish' EXIT

# 1. The session's TensorD on the pod.
step update machine update "$NAME" --tensord "$(command -v cozy)" -y || fail "update"
# Its workload key is in its own status frame, which only Calcifer reads; the private read below needs it.
v=$(cozy machine show "$NAME" --json | jq -r .status.version)
say "pod TensorD $v"
[ "$v" = "$(cozy --version | awk '{print $2}')" ] || fail "the pod runs TensorD $v, not the session's"

# 2. A private commit from the runner, read on the pod.
tiny "$OUT/a.safetensors"
step upload-a model upload "$OUT/a.safetensors" "$MA" --lane f32 || fail "upload from the runner"
CA=$(made upload-a); say "$MA@$CA (private)"
step read model download "$MA@$CA" --on "$NAME" || fail "the pod did not read a private commit"
say "read: $(grep -E 'bytes|hub' "$OUT/read.txt" | tail -2 | tr -s ' ' | paste -sd';')"

# 3. An upload on the pod, under its token.
tiny "$OUT/b.safetensors"
step upload-b model upload "$OUT/b.safetensors" "$MB" --lane f32 --on "$NAME" || fail "upload --on the pod"
CB=$(made upload-b); say "$MB@$CB written on the pod"

# 4. The cell, with the rental live (its Bearer fresh: it signs as this computer's device key).
cozy --json whoami > /dev/null
"$OUT/capabilities" --out "$OUT/cell" --commit "$MA@$CA" --rental "$RID" --rental-state live > "$OUT/cell.txt" 2>&1 \
  || fail "cell: $(grep FAIL "$OUT/cell.txt" | head -3 | tr '\n' ' ')"
cat "$OUT/cell.txt" >> "$R"

# 5. The probe, its weights a private commit from the runner; the runner's Calcifer stops while it waits.
tiny "$OUT/c.safetensors"
step upload-c model upload "$OUT/c.safetensors" "$MC" --lane f32 || fail "upload from the runner"
CC=$(made upload-c)
cozy --json run "$CELL/probe.py" "model.weights=$MC@$CC" "ref=$MC@$CC" "hub=${COZY_HUB_URL:-https://tensorhub.com}" \
  "seconds=$PROBE_S" --on "$NAME" > "$OUT/probe.ndjson" 2> "$OUT/probe.err" & PP=$!
until grep -q '"machine_state":"running"' "$OUT/probe.ndjson" 2>/dev/null; do
  kill -0 $PP 2>/dev/null || fail "the probe ended before it ran: $(tail -c 300 "$OUT/probe.ndjson")"; sleep 2
done
N=$(jq -r 'select(.run.number?) | .run.number' "$OUT/probe.ndjson" | head -1)
# Its follow ends first: a cozy waiting on Calcifer starts it again. No cozy runs until the window ends.
kill $PP 2>/dev/null; wait $PP 2>/dev/null
systemctl --user stop cozy-calcifer; T=$(systemctl --user show cozy-calcifer -p ActiveEnterTimestampMonotonic --value)
say "probe run $N running; the runner's Calcifer: $(systemctl --user is-active cozy-calcifer)"
sleep "$OFF_S"
[ "$(systemctl --user show cozy-calcifer -p ActiveEnterTimestampMonotonic --value)" = "$T" ] &&
  ! systemctl --user is-active -q cozy-calcifer && ! pgrep -u "$(id -u)" -x -f '[^ ]*cozy calcifer' > /dev/null \
  || fail "the runner's Calcifer ran during the window"
say "the runner's Calcifer stayed stopped ${OFF_S}s"
systemctl --user start cozy-calcifer; say "the runner's Calcifer: $(systemctl --user is-active cozy-calcifer) again"
# The run's record: all of the output when it had settled, else the last line after the events it followed.
cozy --json run show "$N" > "$OUT/probe-show.json" 2> "$OUT/probe-show.err"
P=$(jq -sc --argjson n "$N" 'map(select(.number? == $n)) | last' "$OUT/probe-show.json")
say "probe: $(jq -c '{status, result, reason}' <<<"$P")"
cozy machine logs "$NAME" -n 0 > "$OUT/tensord.log" 2>&1
grep -E "redeemed the run capability|refused the run capability|through the gateway" "$OUT/tensord.log" | tail -10 | sed 's/^/    /' >> "$R"
jq -e '.status == "completed" and (.result.opened_through_endpoint | startswith("read ")) and
  (.result.opened_at_hub_with_token | test("401")) and .result.jwts == 0 and .result.private_keys == 0 and
  .result.tensord_memory != "opened" and (.result.endpoints | length > 0 and all(startswith("http://127.0.0.1:")))' <<<"$P" >/dev/null \
  || fail "the probe"
[ "$(grep -c "redeemed the run capability" "$OUT/tensord.log")" -ge 2 ] || fail "the pod never redeemed again"

# 6. The rental ends; its capability is refused.
cozy machine end "$RID" -y >> "$OUT/end.txt" 2>&1
until gone "$RID"; do sleep 10; done
say "GONE $RID ($NAME)"
cozy --json whoami > /dev/null
"$OUT/capabilities" --out "$OUT/cell-ended" --commit "$MA@$CA" --rental "$RID" --rental-state ended > "$OUT/cell-ended.txt" 2>&1
grep -E "rental-ended|unknown-rental|FAIL" "$OUT/cell-ended.txt" | sed 's/^/    /' >> "$R"
grep -q "^ok   rental-ended" "$OUT/cell-ended.txt" || fail "a capability for the ended rental was not refused"
RID=''
say "PASS: the pod read and wrote under its own renewed token, outlived its runner's Calcifer, and its capability ended with it"
