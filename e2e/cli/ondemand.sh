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
