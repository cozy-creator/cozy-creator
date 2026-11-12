# cl-966 (Paul, 11-12): "We need to predict the downloads ahead of time, including things we may potentially need (such as a
# prompt rewriter); we don't necessarily need to wait for them though. … if we do need an emergency download, we should pause any
# other downloads and then have the new emergency download saturate bandwidth, then resume the other download when it's done."

# ondemand.py's `keep` has fontbox (115 MB) as an on-demand slot and holds without reading it. On this cold machine its weights come
# while it holds, each object at a guess's order (behind every run's sure needs), and the call never waits on them.
ondemand_ahead() {
[ "$RUNNER_OS" = Linux ] || return 0
step "a call's on-demand model comes ahead as a guess, never waited on (cl-966)"
local began
began=$(date -u +%s%3N)
cell ondemand-ahead "$here/script/ondemand.py/keep" seconds:=20 || return 0
cozy machine logs local --lines 0 --since 1h > "$OUT/ondemand-ahead.tensord.log" 2>&1 || check "ondemand-ahead: machine logs local failed"
python3 - "$OUT" "$began" "$(run_get "$OUT" ondemand-ahead .id)" <<'JUDGE' > "$OUT/ondemand-ahead.timeline.json" 2> "$OUT/ondemand-ahead.failed" || true
import json, re, sys
from datetime import datetime
from pathlib import Path
out, began, run = Path(sys.argv[1]), int(sys.argv[2]), sys.argv[3]
def at(line):
    try:
        return int(datetime.fromisoformat(line.split(" ", 1)[0].replace("Z", "+00:00")).timestamp() * 1000)
    except ValueError:
        return 0
fetching, landed, waited = [], [], []
for line in (out / "ondemand-ahead.tensord.log").read_text(errors="replace").splitlines():
    if at(line) < began:
        continue
    found = re.search(r": (fetching|landed) .*bytes=(\d+).*priority=(\d+)", line)
    # A guess's order: class 2 of the line's (`priority::guess`).
    if found and (int(found[3]) - 2) >> 62 == 2:
        (fetching if found[1] == "fetching" else landed).append((at(line), int(found[2])))
    if "waiting for required model inputs" in line and run in line:
        waited.append(line.split(": ", 2)[-1][:200])
# A project run's id stays out of TensorD's log: what it waited on is in its own events.
for line in (out / "ondemand-ahead.ndjson").read_text(errors="replace").splitlines():
    try:
        event = json.loads(line)
    except ValueError:
        continue
    reason = event.get("reason") if isinstance(event, dict) else None
    if isinstance(reason, str) and "waiting for required model inputs" in reason:
        waited.append(reason[:200])
record = json.loads((out / "ondemand-ahead.json").read_text())
fields = {k: record.get(k) for k in ("number", "status", "accepted_at_ms", "started_at_ms", "settled_at_ms")}
print(json.dumps({"run": fields, "guessed_objects": len(landed), "guessed_bytes": sum(b for _, b in landed),
                  "first_guess_ms": min((t for t, _ in fetching), default=None), "last_landed_ms": max((t for t, _ in landed), default=None),
                  "waited": waited}, indent=1))
fail = lambda why: print(why, file=sys.stderr)
if sum(b for _, b in landed) < 100 << 20:
    fail(f"its on-demand fontbox didn't come as a guess: {sum(b for _, b in landed)} bytes at a guess's order")
if waited:
    fail(f"the call waited on its on-demand model: {waited[0]}")
JUDGE
cat "$OUT/ondemand-ahead.timeline.json"
while read -r why; do check "ondemand-ahead: $why"; done < "$OUT/ondemand-ahead.failed"
}

# A call holds (resolved, its on-demand BiRefNet guessed behind every run's needs, so part of it may land first), then `cozy model
# install` downloads Qwen3.5 0.8B's 1.6 GB, and while it does the call reads its on-demand BiRefNet (446 MB): its executor asks
# TensorD for it (`model_need`), which fetches what of it is absent before every other download (order 0) and answers once it's
# here, and the install goes on after. The isolated Hub's store is slowed to 30 MB/s (tc on the loopback, from its MinIO's port),
# so the link is the bottleneck, as on a pod.
ondemand_need() {
[ "$RUNNER_OS" = Linux ] || return 0
step "a started call's read of a model not here comes before every other download (cl-966)"
local began port
port=$(pgrep -af minio | grep -oE -- '--address[ =]127\.0\.0\.1:[0-9]+' | grep -oE '[0-9]+$' | head -1)
[ -n "$port" ] || { check "ondemand-need: the isolated Hub's MinIO port wasn't found: $(pgrep -af minio | head -2)"; return; }
sudo tc qdisc add dev lo root handle 1: prio && sudo tc qdisc add dev lo parent 1:3 handle 30: tbf rate 240mbit burst 1mb latency 1s \
  && sudo tc filter add dev lo parent 1: protocol ip prio 1 u32 match ip sport "$port" 0xffff flowid 1:3 \
  || { sudo tc qdisc del dev lo root 2> /dev/null; check "ondemand-need: the store's link wasn't slowed"; return; }
began=$(date -u +%s%3N)
run_submit "$OUT" ondemand-need run "$here/script/ondemand.py/hold" seconds:=10 read:=true --on local > /dev/null
for _ in $(seq 900); do grep -q '"holding"' "$OUT/ondemand-need.ndjson" 2> /dev/null && break; sleep 0.2; done
run_submit "$OUT" ondemand-queued model install qwen/qwen3.5-0.8b --on local > /dev/null
for name in ondemand-need ondemand-queued; do
  run_wait "$OUT" "$name" || check "ondemand-need: $name did not end: $(tail -c 300 "$OUT/$name.ndjson")"
  [ "$(run_get "$OUT" "$name" .status)" = completed ] || check "ondemand-need: $name $(run_get "$OUT" "$name" .status): $(run_get "$OUT" "$name" .reason.message | head -c 300)"
done
sudo tc qdisc del dev lo root || check "ondemand-need: the store's link wasn't restored"
cozy machine logs local --lines 0 --since 1h > "$OUT/ondemand-need.tensord.log" 2>&1 || check "ondemand-need: machine logs local failed"
python3 - "$OUT" "$began" <<'JUDGE' > "$OUT/ondemand-need.timeline.json" 2> "$OUT/ondemand-need.failed" || true
import json, re, sys
from datetime import datetime
from pathlib import Path
out, began = Path(sys.argv[1]), int(sys.argv[2])
def at(line):
    try:
        return int(datetime.fromisoformat(line.split(" ", 1)[0].replace("Z", "+00:00")).timestamp() * 1000)
    except ValueError:
        return 0
need, here, urgent, queued = [], [], [], []
for line in (out / "ondemand-need.tensord.log").read_text(errors="replace").splitlines():
    t = at(line)
    if t < began:
        continue
    if "emergency: zhengpeng7/birefnet is read now" in line:
        need.append((t, int(dict(re.findall(r"(\w+)=(\S+)", line)).get("bytes", "0"))))
    if "emergency: zhengpeng7/birefnet is here" in line:
        here.append((t, dict(re.findall(r"(\w+)=(\S+)", line))))
    found = re.search(r": (fetching|landed) .*bytes=(\d+).*priority=(\d+)", line)
    if found:
        order, kind = int(found[3]), found[1]
        if order == 0:
            urgent.append((t, kind, int(found[2])))
        elif 2 <= order < 1 << 62 and kind == "fetching":
            queued.append(t)
began_ms, ended_ms = (need[0][0], here[0][0]) if need and here else (None, None)
absent = need[0][1] if need else 0
during = [t for t in queued if began_ms and began_ms < t < ended_ms]
landed = sum(b for _, kind, b in urgent if kind == "landed")
record = json.loads((out / "ondemand-need.json").read_text())
print(json.dumps({"run": {k: record.get(k) for k in ("number", "status")}, "asked_ms": began_ms, "here_ms": ended_ms,
                  "emergency": here[0][1] if here else None, "absent_at_read": absent, "landed_at_order_0": landed,
                  "queued_objects_started": len(queued), "queued_objects_started_during": len(during)}, indent=1))
fail = lambda why: print(why, file=sys.stderr)
if not need or not here:
    fail("TensorD never said it fetched the call's on-demand BiRefNet as an emergency")
# What TensorD found absent at the read, and that it all came: its chunks may have started as the call's guesses (a log line
# names the order an object started at), so its own `is here` says what landed.
came = int(here[0][1].get("bytes", "0")) if here else 0
if absent < 100 << 20:
    fail(f"only {absent} bytes of BiRefNet were absent at its read: its guess had come, so nothing was an emergency")
elif came < absent:
    fail(f"{came} bytes of BiRefNet came of the {absent} absent at its read")
if not queued:
    fail("the queued install started nothing: there was no other download")
JUDGE
cat "$OUT/ondemand-need.timeline.json"
while read -r why; do check "ondemand-need: $why"; done < "$OUT/ondemand-need.failed"
}
