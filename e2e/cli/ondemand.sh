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
# here: every other request gives way at its next bytes, so the emergency takes the link, and resumes after. The isolated Hub's
# store is slowed to 30 MB/s (tc on the loopback, from its MinIO's port), so the link is the bottleneck, as on a pod.
slow_store() {
local port
port=$(pgrep -af minio | grep -oE -- '--address[ =]127\.0\.0\.1:[0-9]+' | grep -oE '[0-9]+$' | head -1)
[ -n "$port" ] || { check "$1: the isolated Hub's MinIO port wasn't found: $(pgrep -af minio | head -2)"; return 1; }
sudo tc qdisc add dev lo root handle 1: prio && sudo tc qdisc add dev lo parent 1:3 handle 30: tbf rate 240mbit burst 1mb latency 1s \
  && sudo tc filter add dev lo parent 1: protocol ip prio 1 u32 match ip sport "$port" 0xffff flowid 1:3 \
  || { sudo tc qdisc del dev lo root 2> /dev/null; check "$1: the store's link wasn't slowed"; return 1; }
}

ondemand_need() {
[ "$RUNNER_OS" = Linux ] || return 0
step "a started call's read of a model not here comes before every other download (cl-966)"
local began
slow_store ondemand-need || return 0
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
# The downloader's floor (cl-966): each move is a line, with when it moved and what came while it stood where it was.
def floors(path):
    moves = []
    for line in path.read_text(errors="replace").splitlines():
        if "floor moved" in line:
            f = dict(re.findall(r"(\w+)=(\S+)", line))
            moves.append({"downloader": f["downloader"], "from": f["from"], "to": f["to"], "at": int(f["at_ms"]), "bytes": int(f["bytes"]),
                          "seconds": float(f["seconds"]), "mb_s": float(f["mb_s"])})
    return moves
# Each stand of the floor at an order named by `at`: (from, to, bytes, seconds, mb_s), in order.
def stands(moves, began, at):
    out, open_ = [], {}
    for m in moves:
        if m["at"] < began:
            continue
        if m["from"] != "none" and at(int(m["from"])) and m["downloader"] in open_:
            out.append((open_.pop(m["downloader"]), m["at"], m["bytes"], m["seconds"], m["mb_s"]))
        if m["to"] != "none" and at(int(m["to"])):
            open_[m["downloader"]] = m["at"]
    return out
urgent, queued, resumed = [], [], []
# A project's run says nothing in TensorD's log (#2858): its downloader's lines, which name objects and orders, are the record.
for line in (out / "ondemand-need.tensord.log").read_text(errors="replace").splitlines():
    t = at(line)
    if t < began:
        continue
    found = re.search(r": (fetching|landed) .*bytes=(\d+).*priority=(\d+)", line)
    if found:
        order, kind = int(found[3]), found[1]
        if order == 0:
            urgent.append((t, kind, int(found[2])))
        elif 2 <= order < 1 << 62:
            (queued if kind == "fetching" else resumed).append(t)
# A line names the order an object started at: BiRefNet's chunks already started as its guess land at that order.
landed = sum(b for _, kind, b in urgent if kind == "landed")
emergencies = stands(floors(out / "ondemand-need.tensord.log"), began, lambda order: order == 0)
s, e, came, seconds, rate = emergencies[0] if emergencies else (0, 0, 0, 0.0, 0.0)
during = [t for t in queued if s < t < e]
record = json.loads((out / "ondemand-need.json").read_text())
print(json.dumps({"run": {k: record.get(k) for k in ("number", "status")}, "emergency_ms": [s - began, e - began] if emergencies else None,
                  "emergency_bytes": came, "emergency_seconds": seconds, "emergency_mb_s": rate, "landed_at_order_0": landed,
                  "queued_objects_started": len(queued), "queued_objects_started_during": len(during)}, indent=1))
fail = lambda why: print(why, file=sys.stderr)
if landed < 100 << 20:
    fail(f"{landed} bytes of BiRefNet came at order 0: the call's read of it wasn't fetched first")
if not queued:
    fail("the queued install started nothing: there was no other download")
if not emergencies:
    fail("the downloader's floor never stood at 0: nothing else waited for the emergency")
else:
    if came < 100 << 20:
        fail(f"{came} bytes came while the floor stood at 0: the emergency wasn't BiRefNet's read")
    if during:
        fail(f"{len(during)} of the queued install's objects started during the emergency")
    # The link is 30 MB/s: with every other request given way, the emergency has nearly all of it.
    if rate < 24:
        fail(f"the emergency moved {rate:.1f} MB/s of the 30 MB/s link: the install didn't give way")
    if not any(t > e for t in resumed):
        fail("the install landed nothing after the emergency: it didn't resume")
JUDGE
cat "$OUT/ondemand-need.timeline.json"
while read -r why; do check "ondemand-need: $why"; done < "$OUT/ondemand-need.failed"
}

# An install's packages come from their own index, outside the downloader, whose many requests starved them on a pod (run
# 43316018343: 1080 s at 100-450 KiB/s while weights came). While an install builds, every download after it gives way and none
# starts; then they go on. `spare` holds with pagebox (900 MB, on demand) guessed behind every run's needs over the 30 MB/s link,
# and meanwhile a package installs whose path dependency takes seconds to build.
ondemand_install() {
[ "$RUNNER_OS" = Linux ] || return 0
step "while an install builds, the downloads after it give way (cl-966)"
local began dir=$OUT/ondemand-install
mkdir -p "$dir/slow"
printf '[project]\nname = "e2e-slow-build"\nversion = "0.0.0"\n\n[build-system]\nrequires = ["setuptools>=70"]\nbuild-backend = "setuptools.build_meta"\n' \
  > "$dir/slow/pyproject.toml"
printf 'import time\n\nfrom setuptools import setup\n\ntime.sleep(4)\nsetup(py_modules=[])\n' > "$dir/slow/setup.py"
package_at "$dir/caller" e2e_slow_caller '"cozy-runtime>=0.45", "msgspec>=0.19", "e2e-slow-build"' \
  "$(printf '\n[tool.uv.sources]\ne2e-slow-build = { path = "../slow" }')" <<'PY'
import msgspec

from cozy_runtime.author import App

app = App()


class Ask(msgspec.Struct):
    pass


class Built(msgspec.Struct):
    built: bool


@app.entrypoint(accelerator=False)
def built(payload: Ask) -> Built:
    return Built(built=True)
PY
slow_store ondemand-install || return 0
began=$(date -u +%s%3N)
run_submit "$OUT" ondemand-spare run "$here/script/ondemand.py/spare" seconds:=40 --on local > /dev/null
for _ in $(seq 900); do grep -q '"holding"' "$OUT/ondemand-spare.ndjson" 2> /dev/null && break; sleep 0.2; done
run_submit "$OUT" ondemand-install run "$dir/caller/built" --on local > /dev/null
for name in ondemand-install ondemand-spare; do
  run_wait "$OUT" "$name" || check "ondemand-install: $name did not end: $(tail -c 300 "$OUT/$name.ndjson")"
  [ "$(run_get "$OUT" "$name" .status)" = completed ] || check "ondemand-install: $name $(run_get "$OUT" "$name" .status): $(run_get "$OUT" "$name" .reason.message | head -c 300)"
done
sudo tc qdisc del dev lo root || check "ondemand-install: the store's link wasn't restored"
cozy machine logs local --lines 0 --since 1h > "$OUT/ondemand-install.tensord.log" 2>&1 || check "ondemand-install: machine logs local failed"
python3 - "$OUT" "$began" <<'JUDGE' > "$OUT/ondemand-install.timeline.json" 2> "$OUT/ondemand-install.failed" || true
import json, re, sys
from datetime import datetime
from pathlib import Path
out, began = Path(sys.argv[1]), int(sys.argv[2])
def at(line):
    try:
        return int(datetime.fromisoformat(line.split(" ", 1)[0].replace("Z", "+00:00")).timestamp() * 1000)
    except ValueError:
        return 0
# The downloader's floor (cl-966): each move is a line, with when it moved and what came while it stood where it was.
def floors(path):
    moves = []
    for line in path.read_text(errors="replace").splitlines():
        if "floor moved" in line:
            f = dict(re.findall(r"(\w+)=(\S+)", line))
            moves.append({"downloader": f["downloader"], "from": f["from"], "to": f["to"], "at": int(f["at_ms"]), "bytes": int(f["bytes"]),
                          "seconds": float(f["seconds"]), "mb_s": float(f["mb_s"])})
    return moves
# Each stand of the floor at an order named by `at`: (from, to, bytes, seconds, mb_s), in order.
def stands(moves, began, at):
    out, open_ = [], {}
    for m in moves:
        if m["at"] < began:
            continue
        if m["from"] != "none" and at(int(m["from"])) and m["downloader"] in open_:
            out.append((open_.pop(m["downloader"]), m["at"], m["bytes"], m["seconds"], m["mb_s"]))
        if m["to"] != "none" and at(int(m["to"])):
            open_[m["downloader"]] = m["at"]
    return out
fetching, landed = [], []
for line in (out / "ondemand-install.tensord.log").read_text(errors="replace").splitlines():
    t = at(line)
    if t < began:
        continue
    found = re.search(r": (fetching|landed) .*bytes=(\d+).*priority=(\d+)", line)
    # A guess's order: class 2 of the line's (`priority::guess`).
    if found and (int(found[3]) - 2) >> 62 == 2:
        (fetching if found[1] == "fetching" else landed).append((t, int(found[2])))
# A run's install holds the floor at its job's order (class 0).
builds = stands(floors(out / "ondemand-install.tensord.log"), began, lambda order: 2 <= order < 1 << 62)
build = max(builds, key=lambda w: w[1] - w[0], default=None)
s, e, came = (build[0], build[1], build[2]) if build else (0, 0, 0)
print(json.dumps({"builds_ms": [[a - began, b - began, n] for a, b, n, _, _ in builds], "guess_objects": len(landed),
                  "guess_bytes_before": sum(b for t, b in landed if t < s), "guess_bytes_after": sum(b for t, b in landed if t > e),
                  "bytes_during": came, "started_during": sum(1 for t, _ in fetching if s < t < e)}, indent=1))
fail = lambda why: print(why, file=sys.stderr)
if not build or e - s < 3000:
    fail(f"no install held the floor for its build of seconds: {builds}")
elif not any(t < s for t, _ in fetching) or not any(t > e for t, _ in landed):
    fail("pagebox's guess wasn't in flight across the install's build: nothing was there to give way")
else:
    if any(s < t < e for t, _ in fetching):
        fail(f"{sum(1 for t, _ in fetching if s < t < e)} of pagebox's objects started while the install built")
    # A request in flight gives way at its next bytes: while the install builds, the downloader receives next to nothing.
    if came > 8 << 20:
        fail(f"the downloader received {came} bytes while the install built: its requests in flight didn't give way")
JUDGE
cat "$OUT/ondemand-install.timeline.json"
while read -r why; do check "ondemand-install: $why"; done < "$OUT/ondemand-install.failed"
}
