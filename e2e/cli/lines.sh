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

# cl-905: what a running job reads on demand comes first, and the link stays full. An executor reads a model its call builds on
# demand through its own TensorFS session, outside TensorD's download line: tensor by tensor, each read a new download at a few
# requests, about a third of a link that many requests fill. A call here builds BiRefNet on demand (script/ondemand.py) and holds;
# a TensorFS client reads BiRefNet as its executor would, into the machine's store (`~/.cache/tensorfs`), while `cozy model
# install` downloads Qwen3.5 0.8B's 1.6 GB. TensorD sees the client's claim, finds the call's model and fetches the rest of it at a
# started run's order with all its requests; the install goes on with what the link has left. The isolated Hub's store is slowed
# (tc on the loopback, from its MinIO's port) to 30 MB/s, each request to 3 MB/s (fq), as a pod's link and R2's requests are.
# Alone (its own store, nothing beside it) the read is the executor's slow one. Beside, it finishes in at most its alone time
# (no later job takes the link from it) and the link stays full: the read's and the install's bytes in about bytes / link
# (#3084 held every download back instead: the read kept its alone pace, the link two thirds idle).
ondemand_first() {
[ "$RUNNER_OS" = Linux ] || return 0
step "an on-demand read beside a queued job's download comes first, the link full (cl-905)"
local port began with n
case ${CAND_TFS:-} in '') with=tensorfs ;; *.whl) with=$CAND_TFS ;; *) with=tensorfs==$CAND_TFS ;; esac
port=$(pgrep -af minio | grep -oE -- '--address[ =]127\.0\.0\.1:[0-9]+' | grep -oE '[0-9]+$' | head -1)
[ -n "$port" ] || { check "ondemand: the isolated Hub's MinIO port wasn't found: $(pgrep -af minio | head -2)"; return; }
# read_tensors <name> [store]: BiRefNet opened from the Hub and read tensor by tensor, as an executor fills a model, into the
# store (the machine's when none); its bytes and when it began and ended in <name>.json.
read_tensors() {
  uv run -q --no-project --with "$with" python -I - "$COZY_HUB_URL" "${2:-}" > "$OUT/$1.json" 2> "$OUT/$1.err" <<'READ'
import json, sys, time
import tensorfs
hub, store = sys.argv[1:3]
at = lambda thing: thing() if callable(thing) else thing
began = time.time()
checkpoint = tensorfs.open("zhengpeng7/birefnet", release="0.1.0", lane="fp16", endpoint=hub, **({"cache_dir": store} if store else {}))
for component in at(checkpoint.components).values():
    tensors = at(component.tensors)
    for key in tensors:
        tensors.read(key)
ended = time.time()
print(json.dumps({"bytes": sum(length for _, length in at(checkpoint.chunks)), "seconds": ended - began,
                  "started_unix_ms": int(began * 1000), "ended_unix_ms": int(ended * 1000)}))
READ
}
# done_now: the install's bytes so far, and when (Unix ms).
done_now() { echo "$(date -u +%s%3N) $(grep -oE '"bytes_done": *[0-9]+' "$OUT/ondemand-queued.ndjson" 2> /dev/null | tail -1 | grep -oE '[0-9]+$' || echo 0)"; }
sudo tc qdisc add dev lo root handle 1: prio && sudo tc qdisc add dev lo parent 1:3 handle 30: tbf rate 240mbit burst 1mb latency 1s \
  && sudo tc qdisc add dev lo parent 30:1 handle 31: fq maxrate 24mbit \
  && sudo tc filter add dev lo parent 1: protocol ip prio 1 u32 match ip sport "$port" 0xffff flowid 1:3 \
  || { sudo tc qdisc del dev lo root 2> /dev/null; check "ondemand: the store's link wasn't slowed"; return; }
began=$(date -u +%s%3N)
read_tensors ondemand-alone "$OUT/ondemand-alone-store" || check "ondemand: BiRefNet alone wasn't read: $(tail -c 300 "$OUT/ondemand-alone.err")"
n=$(run_submit "$OUT" ondemand-holder run "$here/script/ondemand.py" seconds:=900 --on local)
[ -n "$n" ] || { sudo tc qdisc del dev lo root; check "ondemand: the holder was never a run: $(tail -c 300 "$OUT/ondemand-holder.err")"; return; }
for _ in $(seq 3000); do grep -qE '"stage": *"holding"' "$OUT/ondemand-holder.ndjson" && break; sleep 0.2; done
grep -qE '"stage": *"holding"' "$OUT/ondemand-holder.ndjson" || check "ondemand: the holder's call never began: $(tail -c 300 "$OUT/ondemand-holder.ndjson")"
run_submit "$OUT" ondemand-queued model install qwen/qwen3.5-0.8b --on local > /dev/null
# Well under way, its requests ramped up, so it has the link when the read begins; its rate then is the link's.
for _ in $(seq 900); do grep -qE '"bytes_done": *[1-9][0-9]{8}' "$OUT/ondemand-queued.ndjson" 2> /dev/null && break; sleep 0.2; done
local link_a link_b read_a read_b
link_a=$(done_now); sleep 4; link_b=$(done_now)
read_a=$(done_now)
read_tensors ondemand-read || check "ondemand: BiRefNet beside the install wasn't read: $(tail -c 300 "$OUT/ondemand-read.err")"
read_b=$(done_now)
run_wait "$OUT" ondemand-queued || check "ondemand: the install did not end: $(tail -c 300 "$OUT/ondemand-queued.ndjson")"
local ended
ended=$(date -u +%s%3N)
cozy --json run cancel "$n" > /dev/null || check "ondemand: run cancel $n failed"
run_wait "$OUT" ondemand-holder > /dev/null || true
sudo tc qdisc del dev lo root || check "ondemand: the store's link wasn't restored"
cozy machine logs local --lines 0 --since 1h > "$OUT/ondemand.tensord.log" 2>&1 || check "ondemand: machine logs local failed"
python3 - "$OUT" "$began" "$link_a" "$link_b" "$read_a" "$read_b" "$ended" <<'JUDGE' > "$OUT/ondemand.json" 2> "$OUT/ondemand.failed" || true
import json, re, sys
from datetime import datetime
from pathlib import Path
out, began = Path(sys.argv[1]), int(sys.argv[2])
(la_ms, la), (lb_ms, lb), (ra_ms, ra), (rb_ms, rb) = ([int(v) for v in arg.split()] for arg in sys.argv[3:7])
ended_ms = int(sys.argv[7])
def result(name):
    try:
        return json.loads((out / f"{name}.json").read_text().strip().splitlines()[-1])
    except (OSError, ValueError, IndexError):
        return {}
alone, read = result("ondemand-alone"), result("ondemand-read")
rate = lambda r: r["bytes"] / r["seconds"] / 1e6 if r.get("seconds") else 0.0
total = max((int(t) for t in re.findall(r'"bytes_total": *(\d+)', (out / "ondemand-queued.ndjson").read_text(errors="replace"))), default=0)
link = (lb - la) / max(lb_ms - la_ms, 1) / 1e3
promoted, fetched = [], 0
for line in (out / "ondemand.tensord.log").read_text(errors="replace").splitlines():
    try:
        at = int(datetime.fromisoformat(line.split(" ", 1)[0].replace("Z", "+00:00")).timestamp() * 1000)
    except ValueError:
        continue
    if at < began:
        continue
    if "which its call builds on demand" in line:
        promoted.append(line.split(" ", 1)[-1][-200:])
    if " landed" in line and re.search(r"\bpriority=1\b", line):
        fetched += int((re.search(r"\bbytes=(\d+)", line) or [0, 0])[1])
# From the read's start until both ended: its bytes and the install's rest.
window = (max(read.get("ended_unix_ms", 0), ended_ms) - ra_ms) / 1e3 if read else 0
moved = (read.get("bytes", 0) + total - ra) / 1e6
print(json.dumps({"alone": {**alone, "mb_s": round(rate(alone), 1)}, "beside": {**read, "mb_s": round(rate(read), 1)},
                  "link_mb_s": round(link, 1), "install_mb_during_the_read": round((rb - ra) / 1e6, 1),
                  "after_the_read_began": {"mb": round(moved, 1), "seconds": round(window, 1), "mb_s": round(moved / window, 1) if window else 0},
                  "tensord_fetched_for_the_read_mb": round(fetched / 1e6, 1), "promoted": promoted}, indent=1))
fail = lambda why: print(why, file=sys.stderr)
if not alone or not read:
    fail("a read returned no result")
elif rate(read) < 0.9 * rate(alone):
    fail(f"the read beside the install moved {rate(read):.1f} MB/s, under 90% of alone's {rate(alone):.1f}")
if not any("birefnet" in said for said in promoted):
    fail("TensorD never said it fetched the rest of BiRefNet for another process's read")
if not total or link <= 0:
    fail("the install's bytes weren't seen")
elif window and moved / window < 0.85 * link:
    fail(f"the link wasn't full while the read ran: {moved:.0f} MB in {window:.1f} s, {moved / window:.1f} MB/s, under 85% of the install's {link:.1f} alone")
if rb <= ra:
    fail("the install moved nothing while the read ran")
JUDGE
cat "$OUT/ondemand.json"
while read -r why; do check "ondemand: $why"; done < "$OUT/ondemand.failed"
}
