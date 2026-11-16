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
    if "waiting for the models to download" in line and run in line:
        waited.append(line.split(": ", 2)[-1][:200])
# A project run's id stays out of TensorD's log: what it waited on is in its own events.
for line in (out / "ondemand-ahead.ndjson").read_text(errors="replace").splitlines():
    try:
        event = json.loads(line)
    except ValueError:
        continue
    reason = event.get("reason") if isinstance(event, dict) else None
    if isinstance(reason, str) and "waiting for the models to download" in reason:
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

# cl-1077 (Paul, 11-16): "why would it download those other two? we don't need those other two for this workflow." H3's `edit`
# calls its segments, never `upscale`'s regenerate, yet the upscaler and regenerate-2k came as its guesses: an orchestrator's prefetch
# took every internal function of its package. calls.py's `edit` calls `used` (e2e/calls-used) and never `unused`
# (e2e/calls-unused): it holds 10 s while its prefetch runs, and this machine never resolves e2e/calls-unused.
ondemand_calls() {
[ "$RUNNER_OS" = Linux ] || return 0
step "an orchestrator's prefetch brings what the functions it calls read, never another's (cl-1077)"
local began
began=$(date -u +%s)
cell ondemand-calls "$here/script/calls.py/edit" seconds:=10 || return 0
cozy --json machine show local > "$OUT/ondemand-calls.machine.json" 2>&1 || { check "ondemand-calls: machine show local failed"; return 0; }
cozy machine logs local tensorfs --lines 0 --since 1h > "$OUT/ondemand-calls.tensorfs.log" 2>&1 || check "ondemand-calls: machine logs local tensorfs failed"
local held pulled
held=$(jq -r '[.status.models[]?.repository] | join(" ")' "$OUT/ondemand-calls.machine.json")
pulled=$(awk -v t="$began" '$1 >= t && /e2e\/calls-unused/' "$OUT/ondemand-calls.tensorfs.log" | head -3)
echo "ondemand-calls: the machine holds $held"
[[ " $held " == *" e2e/calls-used "* ]] || check "ondemand-calls: e2e/calls-used, which the call read, isn't on the machine: $held"
[[ " $held " != *" e2e/calls-unused "* ]] || check "ondemand-calls: e2e/calls-unused came ahead, and no function edit calls reads it: $held"
[ -z "$pulled" ] || check "ondemand-calls: e2e/calls-unused was pulled: $pulled"
}

# cl-1077 (Paul, 11-16): "Downloading the minimax h3 turbo lora makes sense, although it's stuck right now." H3's `edit` puts its edit
# LoRA on a segment's base, and TensorD pulled that base inline while it bound the slot: the segment's turbo LoRA was wanted only
# once the base's 72.8 GB had landed, and sat at 0 bytes for 10 minutes. calls.py's `adapt` calls `adapted` with e2e/calls-lora on
# its base's transformer (e2e/calls-base: 256 MiB, and a 16 MiB text encoder it doesn't read) beside e2e/calls-other, over a 30 MB/s
# link: both come in the call's one download, at once, and only what it loads (a cold H3 edit: transformer_ref and the shared
# parts, 51.7 of the int8 lane's 72.8 GB).
ondemand_adapted() {
[ "$RUNNER_OS" = Linux ] || return 0
step "a LoRA'd base comes in its call's one download with the call's other weights (cl-1077)"
[ -z "${E2E_CALLS_SEED:-}" ] || { check "ondemand-adapted: its models weren't seeded: $E2E_CALLS_SEED"; return 0; }
local began
slow_store ondemand-adapted || return 0
began=$(date -u +%s)
# Its downloads are judged whether or not the call then completes (`cell` records that).
cell ondemand-adapted "$here/script/calls.py/adapt" || :
sudo tc qdisc del dev lo root || check "ondemand-adapted: the store's link wasn't restored"
cozy machine logs local tensorfs --lines 0 --since 1h > "$OUT/ondemand-adapted.tensorfs.log" 2>&1 || check "ondemand-adapted: machine logs local tensorfs failed"
local pulled together
# The base's chunks pulled on their own: a pull of e2e/calls-base of more than its header.
pulled=$(awk -v t="$began" '$1 >= t && $2 == "pull" && $3 ~ /^e2e\/calls-base@/' "$OUT/ondemand-adapted.tensorfs.log" | grep -v ' objects=1 ' | head -2 || true)
# What the call waited on, in order, as its root's progress says: "downloading <models> weights". The root's own prefetch may land
# the base first (it reads only what `adapted` does); while the base still arrives, the call waits on both at once.
grep -o 'downloading [^"]*weights' "$OUT/ondemand-adapted.ndjson" | uniq | sed 's/^/ondemand-adapted: /' || true
together=$(grep -o 'downloading [^"]*weights' "$OUT/ondemand-adapted.ndjson" | grep -E 'e2e/calls-(base|other)' | head -1 || true)
[[ "$together" != *e2e/calls-base* || "$together" == *e2e/calls-other* ]] || check "ondemand-adapted: the call waited on e2e/calls-base alone first: $together"
[ -z "$pulled" ] || check "ondemand-adapted: the LoRA'd base was pulled before the call's other weights were wanted: $pulled"
local held
cozy --json model ls --on local > "$OUT/ondemand-adapted.models.json" 2>&1 || check "ondemand-adapted: model ls --on local failed"
held=$(jq -c '[.models[]? | select(.model == "e2e/calls-base") | {complete, bytes}]' "$OUT/ondemand-adapted.models.json" 2> /dev/null || true)
echo "ondemand-adapted: e2e/calls-base here: $held"
jq -e '[.models[] | select(.model == "e2e/calls-base")] | length == 1 and .[0].complete == false and .[0].bytes >= 268435456 and .[0].bytes < 276824064' "$OUT/ondemand-adapted.models.json" > /dev/null \
  || check "ondemand-adapted: e2e/calls-base here isn't its 256 MiB transformer alone (the call doesn't load its 16 MiB text encoder): $held"
}

# cl-1077 (Paul, 11-16, chartreux): LTX 2.5 (LAB2-47841), accepted at 07:49:30, waited while LAB2-47843, an H3 `edit` accepted at
# 07:59:04, downloaded: "looks all messed up and discombobulated … it's not organizing things well." An orchestrator's root starts at
# once, and its call's weights went as a started run's, ahead of every queued run's. Over a 30 MB/s link `early` (256 MiB) is
# accepted, then `late` calls `late_call` (128 MiB): the later run's call takes its root's place in job order, never a started
# run's, and the earlier run's weights land first.
ondemand_order() {
[ "$RUNNER_OS" = Linux ] || return 0
step "a later run's call never downloads ahead of an earlier run's weights (cl-1077)"
local began n name
slow_store ondemand-order || return 0
began=$(date -u +%s%3N)
n=$(run_submit "$OUT" order-early run "$here/script/calls.py/early" --on local)
for _ in $(seq 600); do cozy --json run watch "$n" --once 2> /dev/null | jq -e '.accepted_at_ms != null' > /dev/null && break; sleep 0.2; done
run_submit "$OUT" order-late run "$here/script/calls.py/late" --on local > /dev/null
for name in order-early order-late; do
  run_wait "$OUT" "$name" || check "ondemand-order: $name did not end: $(tail -c 300 "$OUT/$name.ndjson")"
  [ "$(run_get "$OUT" "$name" .status)" = completed ] || check "ondemand-order: $name $(run_get "$OUT" "$name" .status): $(run_get "$OUT" "$name" .reason.message | head -c 300)"
done
sudo tc qdisc del dev lo root || check "ondemand-order: the store's link wasn't restored"
cozy machine logs local --lines 0 --since 1h > "$OUT/ondemand-order.tensord.log" 2>&1 || check "ondemand-order: machine logs local failed"
python3 - "$OUT" "$began" <<'JUDGE' > "$OUT/ondemand-order.json" 2> "$OUT/ondemand-order.failed" || true
import json, re, sys
from datetime import datetime
from pathlib import Path
out, began = Path(sys.argv[1]), int(sys.argv[2])
def at(line):
    try:
        return int(datetime.fromisoformat(line.split(" ", 1)[0].replace("Z", "+00:00")).timestamp() * 1000)
    except ValueError:
        return 0
# The line's orders (`origin::priority`): 2 + (class << 62 | run << 32 | rank); a started run's is run 0 of class 0.
runs = {}
for line in (out / "ondemand-order.tensord.log").read_text(errors="replace").splitlines():
    t = at(line)
    found = re.search(r": (fetching|landed) .*bytes=(\d+).*priority=(\d+)", line)
    if t < began or not found or int(found[3]) < 2 or (int(found[3]) - 2) >> 62:
        continue
    run = (int(found[3]) - 2) >> 32 & ((1 << 30) - 1)
    each = runs.setdefault(run, {"objects": 0, "bytes": 0, "first_ms": None, "last_landed_ms": None})
    if found[1] == "fetching":
        each["first_ms"] = t - began if each["first_ms"] is None else each["first_ms"]
    else:
        each["objects"], each["bytes"], each["last_landed_ms"] = each["objects"] + 1, each["bytes"] + int(found[2]), t - began
print(json.dumps({"by_run": runs}, indent=1))
fail = lambda why: print(why, file=sys.stderr)
if 0 in runs:
    fail(f"{runs[0]['objects']} objects ({runs[0]['bytes']} bytes) came at a started run's order, ahead of every queued run's: the later run's call")
queued = sorted(run for run in runs if run > 0)
if len(queued) < 2:
    fail(f"both runs' weights weren't in the line: orders of runs {queued}")
else:
    first, later = runs[queued[0]], runs[queued[-1]]
    if first["last_landed_ms"] is None or later["last_landed_ms"] is None or first["last_landed_ms"] > later["last_landed_ms"]:
        fail(f"the earlier run's weights landed last ({first['last_landed_ms']} ms), after the later run's ({later['last_landed_ms']} ms)")
JUDGE
cat "$OUT/ondemand-order.json"
while read -r why; do check "ondemand-order: $why"; done < "$OUT/ondemand-order.failed"
}

# cl-1102 (Paul, 11-16): "we need a general (has, needs) list. Like, my machine has (X) but needs (Y). And then we download just Y, and
# nothing more. We can download extras (Z) in the background, in anticipation of maybe needing them, but they aren't in the critical
# path of inference (user request) so they shouldn't be waited on." calls.py's `needs` loads e2e/calls-needed (192 MiB) and has
# e2e/calls-extra (256 MiB) on demand, never read, on this cold machine over the 30 MB/s link. Its record says what the machine had
# and downloaded for it (`measurements.weights`), and what came at its run's order is that and no more; its progress names
# e2e/calls-needed and its bytes alone, while e2e/calls-extra comes behind it at an extra's order; what it waits on says how much it
# downloads (`cozy machine show`, its run); and a second call finds it all on the machine. `cozy machine show` lists a download only
# for this computer's own runs (a project's are its page's to list), so its list is checked where it has one.
ondemand_needs() {
[ "$RUNNER_OS" = Linux ] || return 0
step "a call downloads what it needs and no more; extras come in the background, never waited on (cl-1102)"
[ -z "${E2E_CALLS_SEED:-}" ] || { check "ondemand-needs: its models weren't seeded: $E2E_CALLS_SEED"; return 0; }
local began
slow_store ondemand-needs || return 0
began=$(date -u +%s%3N)
run_submit "$OUT" ondemand-needs run "$here/script/calls.py/needs" seconds:=12 --on local > /dev/null
# The machine as `cozy machine show` says it, while the call waits on its download (what it waits on) and once it runs.
for _ in $(seq 900); do grep -Eq 'downloading [^"]*weights|"holding"' "$OUT/ondemand-needs.ndjson" 2> /dev/null && break; sleep 0.2; done
(cd "${RUN_PROJECT:-.}" && cozy --json machine show local) > "$OUT/ondemand-needs.waiting.json" 2> /dev/null || check "ondemand-needs: machine show local failed"
for _ in $(seq 900); do grep -q '"holding"' "$OUT/ondemand-needs.ndjson" 2> /dev/null && break; sleep 0.2; done
(cd "${RUN_PROJECT:-.}" && cozy --json machine show local) > "$OUT/ondemand-needs.running.json" 2> /dev/null || check "ondemand-needs: machine show local failed"
run_wait "$OUT" ondemand-needs || check "ondemand-needs: it did not end: $(tail -c 300 "$OUT/ondemand-needs.ndjson")"
[ "$(run_get "$OUT" ondemand-needs .status)" = completed ] || check "ondemand-needs: $(run_get "$OUT" ondemand-needs .status): $(run_get "$OUT" ondemand-needs .reason.message | head -c 300)"
sudo tc qdisc del dev lo root || check "ondemand-needs: the store's link wasn't restored"
cell ondemand-needs-again "$here/script/calls.py/needs" seconds:=0 || :
cozy machine logs local --lines 0 --since 1h > "$OUT/ondemand-needs.tensord.log" 2>&1 || check "ondemand-needs: machine logs local failed"
python3 - "$OUT" "$began" <<'JUDGE' > "$OUT/ondemand-needs.judged.json" 2> "$OUT/ondemand-needs.failed" || true
import json, re, sys
from datetime import datetime
from pathlib import Path
out, began = Path(sys.argv[1]), int(sys.argv[2])
MIB, needed, extra = 1 << 20, "e2e/calls-needed", "e2e/calls-extra"
fail = lambda why: print(why, file=sys.stderr)
def read(name):
    try:
        return json.loads((out / name).read_text())
    except (OSError, ValueError):
        return {}
def at(line):
    try:
        return int(datetime.fromisoformat(line.split(" ", 1)[0].replace("Z", "+00:00")).timestamp() * 1000)
    except ValueError:
        return 0
# What its record says the machine had of its weights, downloaded for it, and fetched besides.
weights = (read("ondemand-needs.json").get("measurements") or {}).get("weights") or {}
again = (read("ondemand-needs-again.json").get("measurements") or {}).get("weights") or {}
# The downloader's lines by order (`origin::priority`): 2 + (class << 62 | run << 32 | rank); 0 is an emergency. Class 0 is a run's
# needs, class 2 its extras. A checkpoint's header and tree come at its run's order too, a few KiB each.
by_class = {"needs": 0, "extras": 0, "emergency": 0}
for line in (out / "ondemand-needs.tensord.log").read_text(errors="replace").splitlines():
    found = re.search(r": landed .*bytes=(\d+).*priority=(\d+)", line)
    if at(line) < began or not found:
        continue
    order = int(found[2])
    if order == 0:
        by_class["emergency"] += int(found[1])
    elif order >= 2 and (order - 2) >> 62 == 0:
        by_class["needs"] += int(found[1])
    elif order >= 2 and (order - 2) >> 62 == 2:
        by_class["extras"] += int(found[1])
# What its progress said it downloads, and what it said it waits on.
stages, waits = [], []
for line in (out / "ondemand-needs.ndjson").read_text(errors="replace").splitlines():
    try:
        event = json.loads(line)
    except ValueError:
        continue
    if not isinstance(event, dict):
        continue
    progress = event.get("progress") if isinstance(event.get("progress"), dict) else {}
    stage = progress.get("stage") or ""
    if "downloading" in stage and "weights" in stage:
        stages.append((stage, progress.get("bytes_total") or 0))
    if isinstance(event.get("reason"), str) and "waiting for the models to download: " in event["reason"]:
        waits.append(event["reason"])
# What the machine listed for it, waiting and running.
listed = {}
for when in ("waiting", "running"):
    status = read(f"ondemand-needs.{when}.json").get("status") or {}
    listed[when] = [{k: op.get(k) for k in ("kind", "subject", "bytes_done", "bytes_total", "runs")} for op in status.get("activity") or []]
    for run in status.get("runs") or []:
        said = run.get("waiting") or (run.get("run") or {}).get("waiting") or ""
        if "waiting for the models to download: " in said:
            waits.append(said)
print(json.dumps({"weights": weights, "again": again, "landed": by_class, "progress": sorted(set(stages)), "waits": sorted(set(w[:160] for w in waits)),
                  "listed": listed}, indent=1))
has, needs, extras = (weights.get(k) for k in ("has", "needs", "extras"))
if not weights:
    fail("its record doesn't say what the machine had of its weights and downloaded for it (measurements.weights)")
else:
    if has != 0 or not 192 * MIB <= needs < 200 * MIB:
        fail(f"a cold machine had {has} bytes and needed {needs}: not none, and {needed}'s 192 MiB")
    if not 256 * MIB <= extras < 264 * MIB:
        fail(f"its extras are {extras} bytes, not {extra}'s 256 MiB")
    if not 0 <= by_class["needs"] - needs <= 2 * MIB:
        fail(f"{by_class['needs']} bytes came at its run's order, and its record says it needed {needs}: its download isn't its needs alone")
    if not 0 < by_class["extras"] <= extras + 2 * MIB:
        fail(f"{by_class['extras']} bytes came at an extra's order, of {extras}: {extra} didn't come in the background")
    if again.get("needs") != 0 or again.get("has") != needs:
        fail(f"a second call's record says {again or 'nothing'}: not all {needs} bytes already on the machine, none to download")
if by_class["emergency"]:
    fail(f"{by_class['emergency']} bytes came as an emergency: the call read what it hadn't downloaded")
for stage, total in sorted(set(stages)):
    if extra in stage or needed not in stage or (weights and total != needs):
        fail(f"its progress said \"{stage}\" of {total} bytes: not {needed}'s {needs} alone")
for said in sorted(set(waits)):
    if extra in said:
        fail(f"it waited on its extra: {said[:200]}")
    if not re.search(r"download: [0-9.]+[KMGT]?B to download", said):
        fail(f"what it waits on doesn't say how much it downloads: {said[:200]}")
for when, operations in listed.items():
    for op in operations:
        if op["subject"] == extra:
            fail(f"while it was {when}, the machine listed {extra} as its download: {json.dumps(op)}")
JUDGE
cat "$OUT/ondemand-needs.judged.json"
while read -r why; do check "ondemand-needs: $why"; done < "$OUT/ondemand-needs.failed"
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

# tfs-300, LAB2-47829 (Paul, 11-16): "we don't need all components of a model to be on the critical path; that's just creating
# unnecessary latency for the end-user who is waiting on models to download and install". An edit's prompt rewriter (10 GiB, on
# demand) came when its stage first read it, in one fetch that said nothing: the run showed "upsampling prompt · no progress for
# 1m47s". ondemand.py's `read_now` reads e2e/ondemand-read (512 MiB, on demand) as it starts, on this cold machine over the 30 MB/s
# link: while it lands, the run's progress (`cozy run watch --json`'s events, as `cozy run` follows it) says "downloading
# e2e/ondemand-read weights" with its bytes rising toward their total, and once it's here, the run's own stage again.
ondemand_progress() {
[ "$RUNNER_OS" = Linux ] || return 0
step "a call's read of a model not here shows its download on the run, then clears (tfs-300)"
slow_store ondemand-progress || return 0
cell ondemand-progress "$here/script/ondemand.py/read_now" seconds:=3 || :
sudo tc qdisc del dev lo root || check "ondemand-progress: the store's link wasn't restored"
python3 - "$OUT" <<'JUDGE' > "$OUT/ondemand-progress.timeline.json" 2> "$OUT/ondemand-progress.failed" || true
import json, sys
from pathlib import Path
out, model = Path(sys.argv[1]), "e2e/ondemand-read"
said = []
for line in (out / "ondemand-progress.ndjson").read_text(errors="replace").splitlines():
    try:
        event = json.loads(line)
    except ValueError:
        continue
    if isinstance(event, dict) and event.get("type") == "run.progress":
        p = event.get("progress") or {}
        said.append((p.get("stage") or "", p.get("bytes_done") or 0, p.get("bytes_total") or 0))
landing = [i for i, (stage, _, _) in enumerate(said) if f"downloading {model} weights" in stage]
samples = [said[i][1:] for i in landing]
after = [stage for stage, _, _ in said[landing[-1] + 1:]] if landing else []
try:
    record = json.loads((out / "ondemand-progress.json").read_text())
except (OSError, ValueError):
    record = {}
ended = (record.get("progress") or {}).get("stage") or ""
print(json.dumps({"run": {k: record.get(k) for k in ("number", "status")}, "stages": list(dict.fromkeys(s for s, _, _ in said)),
                  "download_samples": len(samples), "first": samples[:3], "last": samples[-3:],
                  "stages_after": list(dict.fromkeys(after)), "record_stage": ended}, indent=1))
fail = lambda why: print(why, file=sys.stderr)
if not samples:
    fail(f"its progress never said {model} was downloading while the call read it: {list(dict.fromkeys(s for s, _, _ in said))}")
else:
    done = [d for d, _ in samples]
    totals = {t for _, t in samples}
    if len(totals) != 1 or not 0 < max(totals) <= (512 << 20) + (1 << 20):
        fail(f"its download's total isn't the model's absent bytes: {sorted(totals)}")
    if any(d > t for d, t in samples):
        fail(f"its download said more bytes done than its total: {samples}")
    if len(set(done)) < 2 or max(done[1:], default=0) <= done[0]:
        fail(f"its download's bytes never rose: {samples}")
    if not any(f"downloading {model}" not in stage for stage in after):
        fail(f"its progress still said {model} was downloading after it landed: {after[-3:]}")
if f"downloading {model}" in ended:
    fail(f"its record ended saying {model} was downloading: {ended}")
JUDGE
cat "$OUT/ondemand-progress.timeline.json"
while read -r why; do check "ondemand-progress: $why"; done < "$OUT/ondemand-progress.failed"
}

# cl-1007 (Paul, 11-15): "instead of pausing the download, wouldn't it be better to just leave extra room while another install is
# happening … you can saturate the bandwidth as needed when an install isn't in progress"; his worry: "some long package install (say 2
# or 3 minutes) might pause all model downloads unnecessarily". One 60 Mbit bottleneck: tc on the loopback, one FIFO for the isolated
# Hub's MinIO and a wheel server. A package installs in about 2.5 min, mostly downloading nothing: its path dependency's metadata hook
# sleeps (resolving), uv downloads a 30 MB wheel from a URL, the dependency's build sleeps, fetches 160 MB, sleeps again, then that
# 160 MB unpacks into the environment. (a) It installs alone. (c) It installs while pagebox's 1.6 GB downloads, asked just after it: a
# later job's weights, which #3126's hold, (b), stopped for the whole install. iptables counts each port's bytes every second, and the
# judge holds (c) to it: the weights never stop, take the link outside the install's downloads and a share inside them, and the
# install is no slower than alone. share.json keeps both series. The loopback's 64 KiB packets would leave a few dozen in the
# bottleneck's queue, where a paced flow's one or two in flight lose to drop-tail and wait out TCP's retransmission timer; a
# real link's are 1500 bytes, as here for the cell.
share_link() {
local minio=$1 wheels=$2
sudo ip link set dev lo mtu 1500 && sudo tc qdisc add dev lo root handle 1: prio && sudo tc qdisc add dev lo parent 1:3 handle 30: tbf rate 60mbit burst 1mb latency 200ms \
  && sudo tc filter add dev lo parent 1: protocol ip prio 1 u32 match ip sport "$minio" 0xffff flowid 1:3 \
  && sudo tc filter add dev lo parent 1: protocol ip prio 1 u32 match ip sport "$wheels" 0xffff flowid 1:3 \
  && sudo iptables -w -N E2E_SHARE && sudo iptables -w -I INPUT -i lo -j E2E_SHARE \
  && sudo iptables -w -A E2E_SHARE -p tcp --sport "$minio" && sudo iptables -w -A E2E_SHARE -p tcp --sport "$wheels" \
  || { share_unlink; check "ondemand-share: the shared link wasn't made"; return 1; }
}

share_unlink() {
sudo tc qdisc del dev lo root 2> /dev/null
sudo ip link set dev lo mtu 65536
sudo iptables -w -D INPUT -i lo -j E2E_SHARE 2> /dev/null
sudo iptables -w -F E2E_SHARE 2> /dev/null; sudo iptables -w -X E2E_SHARE 2> /dev/null
true
}

# share_package <dir> <arm> <port>: arm's own package (names and URLs its own, so no cache of uv's serves another arm), its files on
# the wheel server's directory <dir>/www.
share_package() {
local dir=$1 arm=$2 port=$3
mkdir -p "$dir/$arm/slow/e2e_share_slow_$arm" "$dir/www" && : > "$dir/$arm/slow/e2e_share_slow_$arm/__init__.py"
printf '[build-system]\nrequires = ["setuptools>=70"]\nbuild-backend = "setuptools.build_meta"\n' > "$dir/$arm/slow/pyproject.toml"
cat > "$dir/$arm/slow/setup.py" <<PY
import sys, time, urllib.request

from setuptools import setup

# Its metadata is the build's to say (no [project]): resolving waits for this hook, as for any sdist without static metadata.
if "dist_info" in sys.argv:
    time.sleep(45)
# Built: a compile's worth of waiting, a download in the middle, more waiting; its wheel carries what came (160 MB to unpack).
if "bdist_wheel" in sys.argv:
    time.sleep(20)
    urllib.request.urlretrieve("http://127.0.0.1:$port/blob-$arm.bin", "e2e_share_slow_$arm/blob.bin")
    time.sleep(45)
setup(name="e2e-share-slow-$arm", version="0.0.0", packages=["e2e_share_slow_$arm"], package_data={"e2e_share_slow_$arm": ["*.bin"]})
PY
python3 - "$dir/www" "$arm" <<'PY'
import base64, hashlib, os, sys, zipfile
www, arm = sys.argv[1], sys.argv[2]
with open(f"{www}/blob-{arm}.bin", "wb") as blob:
    for _ in range(160):
        blob.write(os.urandom(1 << 20))
name = f"e2e_share_dl_{arm}"
files = {f"{name}/__init__.py": b"", f"{name}/data.bin": os.urandom(30 << 20),
         f"{name}-0.0.0.dist-info/METADATA": f"Metadata-Version: 2.1\nName: e2e-share-dl-{arm}\nVersion: 0.0.0\n".encode(),
         f"{name}-0.0.0.dist-info/WHEEL": b"Wheel-Version: 1.0\nGenerator: e2e\nRoot-Is-Purelib: true\nTag: py3-none-any\n"}
digest = lambda data: "sha256=" + base64.urlsafe_b64encode(hashlib.sha256(data).digest()).rstrip(b"=").decode()
record = "".join(f"{path},{digest(data)},{len(data)}\n" for path, data in files.items()) + f"{name}-0.0.0.dist-info/RECORD,,\n"
with zipfile.ZipFile(f"{www}/{name}-0.0.0-py3-none-any.whl", "w", zipfile.ZIP_STORED) as wheel:
    for path, data in [*files.items(), (f"{name}-0.0.0.dist-info/RECORD", record.encode())]:
        wheel.writestr(path, data)
PY
package_at "$dir/$arm/caller" "e2e_share_$arm" "\"cozy-runtime>=0.45\", \"msgspec>=0.19\", \"e2e-share-slow-$arm\", \"e2e-share-dl-$arm\"" \
  "$(printf '\n[tool.uv.sources]\ne2e-share-slow-%s = { path = "../slow" }\ne2e-share-dl-%s = { url = "http://127.0.0.1:%s/e2e_share_dl_%s-0.0.0-py3-none-any.whl" }' "$arm" "$arm" "$port" "$arm")" <<'PY'
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
}

ondemand_share() {
[ "$RUNNER_OS" = Linux ] || return 0
step "an install's downloads get the link, the weights a share of it, and only while it downloads (cl-1007)"
local dir=$OUT/ondemand-share minio wheels server sampler began name
minio=$(pgrep -af minio | grep -oE -- '--address[ =]127\.0\.0\.1:[0-9]+' | grep -oE '[0-9]+$' | head -1)
[ -n "$minio" ] || { check "ondemand-share: the isolated Hub's MinIO port wasn't found: $(pgrep -af minio | head -2)"; return 0; }
wheels=$(python3 -c 'import socket; s = socket.socket(); s.bind(("127.0.0.1", 0)); print(s.getsockname()[1])')
mkdir -p "$dir/www"
share_package "$dir" a "$wheels" && share_package "$dir" c "$wheels" || { check "ondemand-share: its packages weren't made"; return 0; }
python3 -m http.server "$wheels" --bind 127.0.0.1 --directory "$dir/www" > "$dir/server.log" 2>&1 &
server=$!
share_link "$minio" "$wheels" || { kill "$server"; return 0; }
# Each second: the weights' bytes (from MinIO) and the install's (from the wheel server), as the bottleneck delivered them.
( while :; do
    printf '%s %s\n' "$(date +%s%3N)" "$(sudo iptables -w -nvxL E2E_SHARE | awk 'NR > 2 { printf "%s ", $2 }')"
    sleep 1
  done ) > "$dir/samples.txt" 2>&1 &
sampler=$!
began=$(date -u +%s%3N)
# Local code installs before its run is made: the install is from the ask to its run's number.
submitted() { local t0; t0=$(date +%s%3N); run_submit "$OUT" "$1" run "$dir/$2/caller/built" --on local > /dev/null; echo "$t0 $(date +%s%3N)" > "$dir/install-$2.ms"; }
submitted share-a a
run_wait "$OUT" share-a || check "ondemand-share: share-a did not end: $(tail -c 300 "$OUT/share-a.ndjson")"
submitted share-c c &
local installing=$!
for _ in $(seq 300); do grep -qs 'installing' "$OUT/share-c.err" "$OUT/share-c.ndjson" && break; sleep 0.2; done
run_submit "$OUT" share-weights model install lilylilith/pagebox --on local > /dev/null
wait "$installing"
for name in share-c share-weights; do
  run_wait "$OUT" "$name" || check "ondemand-share: $name did not end: $(tail -c 300 "$OUT/$name.ndjson")"
done
for name in share-a share-c share-weights; do
  [ "$(run_get "$OUT" "$name" .status)" = completed ] || check "ondemand-share: $name $(run_get "$OUT" "$name" .status): $(run_get "$OUT" "$name" .reason.message | head -c 300)"
done
kill "$sampler" "$server" 2> /dev/null
share_unlink
rm -rf "$dir/www"
cozy machine logs local --lines 0 --since 1h > "$OUT/ondemand-share.tensord.log" 2>&1 || check "ondemand-share: machine logs local failed"
python3 - "$OUT" "$began" <<'JUDGE' > "$OUT/ondemand-share.json" 2> "$OUT/ondemand-share.failed" || true
import json, re, sys
from datetime import datetime
from pathlib import Path
out, began = Path(sys.argv[1]), int(sys.argv[2])
LINK = 60e6 / 8  # the bottleneck, bytes/s
samples = []
for line in (out / "ondemand-share/samples.txt").read_text(errors="replace").splitlines():
    f = line.split()
    if len(f) >= 3 and all(x.isdigit() for x in f[:3]):
        samples.append((int(f[0]), int(f[1]), int(f[2])))
# Per second, from the sampler's own clock: (ms, weights bytes/s, install bytes/s).
rates = [(b[0], (b[1] - a[1]) / ((b[0] - a[0]) / 1e3), (b[2] - a[2]) / ((b[0] - a[0]) / 1e3)) for a, b in zip(samples, samples[1:]) if b[0] > a[0]]
installed = lambda arm: tuple(int(x) for x in (out / f"ondemand-share/install-{arm}.ms").read_text().split())
(a0, a1), (c0, c1) = installed("a"), installed("c")
w0 = json.loads((out / "share-weights.json").read_text()).get("accepted_at_ms") or 0
weights = [t for t, w, _ in rates if w > 0.005 * LINK]
w_first, w_last = (min(weights), max(weights)) if weights else (0, 0)
# The install with the weights beside it: from the weights' first second (asked once the install began) to either's end.
during = [(t, w, i) for t, w, i in rates if max(c0, w_first) + 3000 <= t <= min(c1, w_last)]
# The install downloads in a second it moves a tenth of the link or more; the seconds either side are its edges, judged as neither.
busy = {t for t, _, i in rates if i > 0.1 * LINK}
edge = lambda t: any(abs(t - b) <= 2000 for b in busy)
inside = [w for t, w, _ in during if t in busy]
outside = [w for t, w, _ in during if not edge(t)]
mean = lambda xs: sum(xs) / len(xs) if xs else 0.0
stalls, run = [], []
for t, w, _ in during:
    run = run + [t] if w < 0.02 * LINK else []
    if len(run) == 5:
        stalls.append(run[0] - began)
shares = []
for line in (out / "ondemand-share.tensord.log").read_text(errors="replace").splitlines():
    if "link shared with an install" in line or "weights take the whole link" in line:
        shares.append(line.split(" ", 1)[0] + " " + " ".join(re.findall(r"\w+=\S+", line)))
series = [[t - began, round(w / 1e6, 2), round(i / 1e6, 2)] for t, w, i in rates]
summary = {"link_mb_s": LINK / 1e6, "install_alone_s": (a1 - a0) / 1e3, "install_with_weights_s": (c1 - c0) / 1e3,
           "weights_asked_s": (w0 - began) / 1e3, "weights_first_s": (w_first - began) / 1e3, "weights_last_s": (w_last - began) / 1e3,
           "weights_inside_mb_s": round(mean(inside) / 1e6, 2), "weights_outside_mb_s": round(mean(outside) / 1e6, 2),
           "install_download_s": len([t for t in busy if c0 <= t <= c1]), "stalls_from_s": [s / 1e3 for s in stalls]}
print(json.dumps({"summary": summary, "share_lines": shares[-200:], "series_ms_weights_install_mb_s": series}, indent=1))
# The log's own copy: MB/s in 5 s buckets from the ask of (c) to the weights' end, weights / install.
buckets = {}
for t, w, i in rates:
    if c0 - 10000 <= t <= w_last + 5000:
        b = buckets.setdefault((t - c0) // 5000 * 5, [0.0, 0.0, 0])
        b[0], b[1], b[2] = b[0] + w, b[1] + i, b[2] + 1
(out / "ondemand-share.series").write_text(" ".join(f"{k:+d}s:{v[0] / v[2] / 1e6:.1f}/{v[1] / v[2] / 1e6:.1f}" for k, v in sorted(buckets.items())) + "\n")
fail = lambda why: print(why, file=sys.stderr)
if not during or not inside:
    fail(f"the install didn't download beside the weights: {summary}")
else:
    if stalls:
        fail(f"the weights stopped for 5 s or more during the install, from {summary['stalls_from_s']} s")
    if mean(outside) < 0.75 * LINK:
        fail(f"outside the install's downloads the weights moved {mean(outside) / 1e6:.2f} MB/s of the {LINK / 1e6:.2f} MB/s link")
    if not 0 < mean(inside) <= 0.35 * LINK:
        fail(f"during the install's downloads the weights moved {mean(inside) / 1e6:.2f} MB/s of the {LINK / 1e6:.2f} MB/s link: not a share")
if (c1 - c0) > 1.1 * (a1 - a0):
    fail(f"beside the weights the install took {(c1 - c0) / 1e3:.1f} s, alone {(a1 - a0) / 1e3:.1f} s")
JUDGE
python3 -c 'import json, sys; print(json.dumps(json.load(open(sys.argv[1]))["summary"], indent=1))' "$OUT/ondemand-share.json" || true
echo "weights/install MB/s from (c)'s ask: $(cat "$OUT/ondemand-share.series" 2> /dev/null)"
while read -r why; do check "ondemand-share: $why"; done < "$OUT/ondemand-share.failed"
}

# ondemand_ramp (th-378): a model's weights alone through cl-1007's 60 Mbit FIFO, from an empty store: their time, and the streams
# TensorFS's ramp settled at. 11-16 cale's ramp doubled on a window that received nothing (8 → 16 at 0 MB/s, then 32 against that)
# and settled keeping a doubling that bought nothing: 64 streams on a 28 MB/s link. ondemand-ramp.json keeps the numbers.
ondemand_ramp() {
[ "$RUNNER_OS" = Linux ] || return 0
step "a model's weights through a 60 Mbit link: their time and the streams the ramp settled at (th-378)"
local minio began ended ramp
minio=$(pgrep -af minio | grep -oE -- '--address[ =]127\.0\.0\.1:[0-9]+' | grep -oE '[0-9]+$' | head -1)
[ -n "$minio" ] || { check "ondemand-ramp: the isolated Hub's MinIO port wasn't found: $(pgrep -af minio | head -2)"; return 0; }
sudo ip link set dev lo mtu 1500 && sudo tc qdisc add dev lo root handle 1: prio \
  && sudo tc qdisc add dev lo parent 1:3 handle 30: tbf rate 60mbit burst 1mb latency 200ms \
  && sudo tc filter add dev lo parent 1: protocol ip prio 1 u32 match ip sport "$minio" 0xffff flowid 1:3 \
  || { sudo tc qdisc del dev lo root 2> /dev/null; sudo ip link set dev lo mtu 65536; check "ondemand-ramp: the link wasn't slowed"; return 0; }
began=$(date +%s%3N)
run_submit "$OUT" ramp-weights model install lilylilith/pagebox --on local > /dev/null
run_wait "$OUT" ramp-weights || check "ondemand-ramp: the install did not end: $(tail -c 300 "$OUT/ramp-weights.ndjson")"
ended=$(date +%s%3N)
sudo tc qdisc del dev lo root 2> /dev/null; sudo ip link set dev lo mtu 65536
[ "$(run_get "$OUT" ramp-weights .status)" = completed ] \
  || check "ondemand-ramp: $(run_get "$OUT" ramp-weights .status): $(run_get "$OUT" ramp-weights .reason.message | head -c 300)"
cozy machine logs local --lines 0 --since 1h > "$OUT/ondemand-ramp.tensord.log" 2>&1 || check "ondemand-ramp: machine logs local failed"
ramp=$(grep -oE '(more streams|streams settled|fewer streams)[^"]*' "$OUT/ondemand-ramp.tensord.log" | sed -E 's/ +/ /g' | paste -sd ';' -)
python3 - "$OUT/ondemand-ramp.tensord.log" "$((ended - began))" > "$OUT/ondemand-ramp.json" <<'PY'
import json, re, sys
log, ms = open(sys.argv[1], errors="replace").read(), int(sys.argv[2])
steps = [(m[1], int(m[2]), float(m[3])) for m in re.finditer(r"(more streams|streams settled|fewer streams)\S*\s+streams=(\d+)(?:\s+mb_s=([\d.]+))?", log) if m[3]]
settled = [s for s in steps if s[0] == "streams settled"]
print(json.dumps({"ms": ms, "settled_streams": settled[0][1] if settled else None, "ramp": steps}))
PY
echo "ondemand-ramp: pagebox in $((ended - began)) ms; ramp: $ramp"
}
