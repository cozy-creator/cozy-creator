# cl-905: a machine's three lines, installs, downloads and its runs, each in job order and all at once. Three jobs queued
# together on this cold machine: A, BiRefNet (its environment, its Hub weights, a run on the runner's CPU); B, Qwen3.5
# 0.8B's weights (`cozy model install`, a download alone); C, the weightless package (an environment of its own, and a run).
# One environment builds at a time, A's weights come whole before B's take the link, and B downloads while A starts and
# runs. lines.py prints the timeline (lines.json): each job's spans in each line, end to end against the ideal for them.

machine_lines() {
[ "$RUNNER_OS" = Linux ] || return 0
step "three jobs on one machine: each line in job order, all at once (cl-905)"
local began name n dog a c
began=$(date -u +%s%3N)
dog=$(picture dog2.jpg)
# A and C at once: local code installs before its call is sent, so their installs meet in the install line. B once A's call
# is on the machine, so the machine takes its download after A's.
run_submit "$OUT" lines-a run "$(birefnet)" --file "image=$dog" --on local > /dev/null & a=$!
run_submit "$OUT" lines-c run "$here/weightless/reverse" "text=lines$RANDOM" --on local > /dev/null & c=$!
wait "$a" "$c"
for name in a c; do [ "$(cat "$OUT/lines-$name.number" 2> /dev/null)" != none ] || { check "lines: $name was never a run: $(tail -c 300 "$OUT/lines-$name.err")"; return; }; done
n=$(cat "$OUT/lines-a.number")
for _ in $(seq 600); do cozy --json run watch "$n" --once 2> /dev/null | jq -e '.accepted_at_ms != null' > /dev/null && break; sleep 0.2; done
n=$(run_submit "$OUT" lines-b model install qwen/qwen3.5-0.8b --on local)
[ -n "$n" ] || { check "lines: b was never a run: $(tail -c 300 "$OUT/lines-b.err")"; return; }
for name in a b c; do
  run_wait "$OUT" "lines-$name" || check "lines: $name (run $(cat "$OUT/lines-$name.number")) did not end: $(tail -c 300 "$OUT/lines-$name.ndjson")"
  [ "$(run_get "$OUT" "lines-$name" .status)" = completed ] || check "lines: $name (run $(cat "$OUT/lines-$name.number")) $(run_get "$OUT" "lines-$name" .status): $(run_get "$OUT" "lines-$name" .reason.message | head -c 300)"
done
cozy --json run ls --all --limit 40 > "$OUT/lines-runs.json" || check "lines: run ls failed"
cozy machine logs local --since 2h > "$OUT/lines.tensord.log" || check "lines: machine logs local failed"
python3 "$here/lines.py" "$OUT" "$began" "$OUT/lines.tensord.log" a b c > "$OUT/lines.json" 2> "$OUT/lines.failed" || true
cat "$OUT/lines.json"
while read -r why; do check "lines: $why"; done < "$OUT/lines.failed"
}

# cl-905: what a running job reads on demand comes before a queued job's download. An executor reads an on-demand model through its own
# TensorFS session, into the machine's store, outside TensorD's download line; so does any process sharing the store, and this cell reads
# as one: a TensorFS client here (the computer's store, `~/.cache/tensorfs`, the local machine's too) opening a model whole from the Hub.
# The isolated Hub's store is slowed to 30 MB/s (tc on the loopback, from its MinIO's port), so the link is the bottleneck, as on a pod.
# fontbox (115 MB) alone first; then BiRefNet (444 MB) while `cozy model install` downloads Qwen3.5 0.8B's 1.6 GB. TensorD holds that
# download back while the read is in flight, its requests in flight finishing: none of its objects starts then (its log says so), and the
# read's rate is within 25% of alone's.
ondemand_first() {
[ "$RUNNER_OS" = Linux ] || return 0
step "a read beside a queued job's download comes first (cl-905)"
local port began with
case ${CAND_TFS:-} in '') with=tensorfs ;; *.whl) with=$CAND_TFS ;; *) with=tensorfs==$CAND_TFS ;; esac
port=$(pgrep -af minio | grep -oE -- '--address[ =]127\.0\.0\.1:[0-9]+' | grep -oE '[0-9]+$' | head -1)
[ -n "$port" ] || { check "ondemand: the isolated Hub's MinIO port wasn't found: $(pgrep -af minio | head -2)"; return; }
# read_whole <name> <model> <release> <lane>: the model opened whole from the Hub into the store, its bytes and seconds in <name>.json.
read_whole() {
  uv run -q --no-project --with "$with" python -I - "$2" "$3" "$4" "$COZY_HUB_URL" > "$OUT/$1.json" 2> "$OUT/$1.err" <<'READ'
import json, sys, time
import tensorfs
name, release, lane, hub = sys.argv[1:5]
began = time.time()
checkpoint = tensorfs.open(name, release=release, lane=lane, endpoint=hub, lazy=False)
ended = time.time()
chunks = checkpoint.chunks() if callable(checkpoint.chunks) else checkpoint.chunks
print(json.dumps({"bytes": sum(length for _, length in chunks), "seconds": ended - began,
                  "started_unix_ms": int(began * 1000), "ended_unix_ms": int(ended * 1000)}))
READ
}
sudo tc qdisc add dev lo root handle 1: prio && sudo tc qdisc add dev lo parent 1:3 handle 30: tbf rate 240mbit burst 1mb latency 1s \
  && sudo tc filter add dev lo parent 1: protocol ip prio 1 u32 match ip sport "$port" 0xffff flowid 1:3 \
  || { sudo tc qdisc del dev lo root 2> /dev/null; check "ondemand: the store's link wasn't slowed"; return; }
began=$(date -u +%s%3N)
read_whole ondemand-solo lilylilith/fontbox 0.1.0 bf16 || check "ondemand: fontbox alone wasn't read: $(tail -c 300 "$OUT/ondemand-solo.err")"
run_submit "$OUT" ondemand-queued model install qwen/qwen3.5-0.8b --on local > /dev/null
# Well under way, its requests ramped up, so it has the link when the read begins.
for _ in $(seq 900); do grep -qE '"bytes_done": *[1-9][0-9]{8}' "$OUT/ondemand-queued.ndjson" 2> /dev/null && break; sleep 0.2; done
read_whole ondemand-read zhengpeng7/birefnet 0.1.0 fp16 || check "ondemand: BiRefNet wasn't read: $(tail -c 300 "$OUT/ondemand-read.err")"
run_wait "$OUT" ondemand-queued || check "ondemand: the queued download did not end: $(tail -c 300 "$OUT/ondemand-queued.ndjson")"
sudo tc qdisc del dev lo root || check "ondemand: the store's link wasn't restored"
cozy machine logs local --lines 0 --since 1h > "$OUT/ondemand.tensord.log" 2>&1 || check "ondemand: machine logs local failed"
python3 - "$OUT" "$began" <<'JUDGE' > "$OUT/ondemand.json" 2> "$OUT/ondemand.failed" || true
import json, re, sys
from datetime import datetime
from pathlib import Path
out, began = Path(sys.argv[1]), int(sys.argv[2])
def result(name):
    try:
        return json.loads((out / f"{name}.json").read_text().strip().splitlines()[-1])
    except (OSError, ValueError, IndexError):
        return {}
solo, read = result("ondemand-solo"), result("ondemand-read")
rate = lambda r: r["bytes"] / r["seconds"] / 1e6 if r.get("seconds") else 0.0
held, starts = [], []
for line in (out / "ondemand.tensord.log").read_text(errors="replace").splitlines():
    try:
        at = int(datetime.fromisoformat(line.split(" ", 1)[0].replace("Z", "+00:00")).timestamp() * 1000)
    except ValueError:
        continue
    if at < began:
        continue
    if "later downloads wait" in line or "downloads go on" in line:
        held.append([at, line.split(": ", 1)[-1][:90]])
    found = re.search(r"fetching .*priority=(\d+)", line)
    if found and 0 < int(found[1]) < 1 << 62 and (int(found[1]) - 1) >> 32 > 0:
        starts.append(at)
during = [at for at in starts if read and read["started_unix_ms"] + 1000 < at < read["ended_unix_ms"]]
print(json.dumps({"alone": {**solo, "mb_s": round(rate(solo), 1)}, "beside": {**read, "mb_s": round(rate(read), 1)},
                  "queued_objects_started": len(starts), "queued_objects_started_during_the_read": len(during), "holds": held}, indent=1))
fail = lambda why: print(why, file=sys.stderr)
if not solo or not read:
    fail("a read returned no result")
elif rate(read) < 0.75 * rate(solo):
    fail(f"the read beside the queued download moved {rate(read):.1f} MB/s, under 75% of alone's {rate(solo):.1f}")
if not starts:
    fail("the queued download started nothing: there was no download to hold back")
if during:
    fail(f"{len(during)} of the queued download's objects started while the read was in flight")
if not any("later downloads wait" in said for _, said in held):
    fail("TensorD never said it held a download back for another process's fetch")
JUDGE
cat "$OUT/ondemand.json"
while read -r why; do check "ondemand: $why"; done < "$OUT/ondemand.failed"
}
