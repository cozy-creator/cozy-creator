# link-probe2 (tfs-300 round 2; proof branch only): one host, stages in one session, each second on the pod: every TCP socket's
# round trip and bytes by owner (TensorD's weights, uv's installs, the probe's own curl/python), TensorD's disk writes and CPU, the
# page cache's dirty and writeback bytes, each disk's writes and busy time, and the Hub's connect/TLS/total time (control latency).
# Stages: idle; a big PyPI wheel alone; weights alone (a large model: where its bytes wait, arrival against write against hash);
# the same wheel beside the weights; TCP_WINDOW_CLAMP set mid-stream on a live download; published packages installed on the
# worker image (their third-party bytes); a local package that really downloads beside weights (rcv_rtt with and without it);
# optionally a GPU run's step time quiet and beside a large buffered weight write. Optionally the market is sampled for a slow host.
# params:
#   provider          runpod | vast ("" for any); `--provider` (dev-only, so this runner's config turns dev on)
#   keep_under_mb_s   0 keeps the first host; else a host whose weights come faster is ended and another rented (`tries`)
#   tries             hosts to sample at most (1)
#   model             the large weights (hidream-ai/hidream-o1-dev); model2 the second, written beside a GPU run (stage gpu)
#   wheel             a PyPI project==version whose cp312 manylinux x86_64 wheel is the big download (torch==2.5.1)
#   packages          published packages to install, space-separated (sdxl krea-2 flux.3-klein)
#   gpu               true: stage gpu (sdxl run quiet, then beside model2's download)
#   need_s            seconds the guard must buy; floor_mb_s must be 0 when keep_under_mb_s is set
. "$(dirname "$0")/lib.sh"
PROVIDER=$(param provider '') KEEP=$(param keep_under_mb_s 0) TRIES=$(param tries 1) MODEL=$(param model hidream-ai/hidream-o1-dev)
MODEL2=$(param model2 '') WHEEL=$(param wheel 'torch==2.5.1') PACKAGES=$(param packages 'sdxl krea-2 flux.3-klein') GPU=$(param gpu false)
[[ $KEEP =~ ^[0-9]+$ && $TRIES =~ ^[0-9]+$ ]] || fail "keep_under_mb_s $KEEP, tries $TRIES"
[ "$KEEP" = 0 ] || [ "$FLOOR_MBS" = 0 ] || fail "keep_under_mb_s needs floor_mb_s 0: a slow link is the point"
[ -f ~/.ssh/id_ed25519.pub ] || ssh-keygen -q -t ed25519 -N '' -f ~/.ssh/id_ed25519 || fail "ssh-keygen"
RENT_ARGS="--ssh${PROVIDER:+ --provider $PROVIDER}"
if [ -n "$PROVIDER" ] && ! grep -q '^dev *= *true' ~/.cozy/config.toml 2> /dev/null; then
  { echo 'dev = true'; cat ~/.cozy/config.toml 2> /dev/null || true; } > ~/.cozy/config.toml.next && mv ~/.cozy/config.toml.next ~/.cozy/config.toml
fi
PKG=$OUT/probe-pkg
mkdir -p "$PKG" && printf 'import msgspec\n\nfrom cozy_runtime.author import App\n\napp = App()\n\n\nclass Ask(msgspec.Struct):\n    pass\n\n\nclass Done(msgspec.Struct):\n    done: bool\n\n\n@app.entrypoint(accelerator=False)\ndef installed(payload: Ask) -> Done:\n    return Done(done=True)\n' > "$PKG/probe.py"
# About 200 MB of wheels no worker image seeds, none pinned against the Runtime's numpy.
cat > "$PKG/pyproject.toml" <<'TOML'
[project]
name = "tfs300-probe2"
version = "0.0.0"
requires-python = ">=3.11"
dependencies = ["cozy-runtime>=0.45", "msgspec>=0.19", "pyarrow", "polars", "opencv-python-headless", "scipy", "scikit-learn", "duckdb"]

[project.entry-points."cozy.application"]
default = "probe:app"

[build-system]
requires = ["hatchling>=1.25"]
build-backend = "hatchling.build"

[tool.hatch.build.targets.wheel]
only-include = ["probe.py"]
TOML
# Each second on the pod. Sockets by owner from sock_diag (rtt 68, rcv_rtt 92, bytes_received 128, min_rtt 148 in tcp_info); TensorD's
# write_bytes and CPU ticks; /proc/meminfo Dirty and Writeback; each disk's sectors written and ms busy; the stage marker.
SAMPLER_PY=$(cat <<'PY'
import json, os, socket, struct, time
def dump(family):
    s = socket.socket(socket.AF_NETLINK, socket.SOCK_DGRAM, 4)
    req = struct.pack("=BBBBI", family, socket.IPPROTO_TCP, 1 << 1, 0, 0xffffffff) + b"\0" * 48
    s.sendto(struct.pack("=IHHII", 16 + len(req), 20, 0x301, 1, 0) + req, (0, 0))
    found = {}
    while True:
        data, at = s.recv(1 << 16), 0
        while at + 16 <= len(data):
            length, kind = struct.unpack_from("=IH", data, at)
            if kind in (2, 3):
                return found
            body, a = data[at + 16:at + length], 72
            inode = struct.unpack_from("=I", body, 68)[0]
            while a + 4 <= len(body):
                size, attr = struct.unpack_from("=HH", body, a)
                if size < 4:
                    break
                info = body[a + 4:a + size]
                if attr == 2 and len(info) >= 136:
                    found[inode] = (struct.unpack_from("=I", info, 68)[0], struct.unpack_from("=I", info, 92)[0],
                                    struct.unpack_from("=I", info, 148)[0] if len(info) >= 152 else 0, struct.unpack_from("=Q", info, 128)[0],
                                    struct.unpack_from("=B", info, 2)[0], struct.unpack_from("=I", info, 100)[0])
                a += (size + 3) & ~3
            at += (length + 3) & ~3
def owners():
    out, tensord = {}, []
    for pid in filter(str.isdigit, os.listdir("/proc")):
        try:
            argv = open(f"/proc/{pid}/cmdline", "rb").read().split(b"\0")
            base = os.path.basename(argv[0])
            who = "weights" if b"machine" in argv and b"serve" in argv else "install" if base == b"uv" else "probe" if base in (b"curl", b"python3") and b"/tmp/clamp.py" in argv or base == b"curl" else None
            if who == "weights":
                tensord.append(pid)
            if who:
                for fd in os.listdir(f"/proc/{pid}/fd"):
                    target = os.readlink(f"/proc/{pid}/fd/{fd}")
                    if target.startswith("socket:["):
                        out[int(target[8:-1])] = who
        except OSError:
            pass
    return out, tensord
def io(pid):
    try:
        d = dict(line.split(": ") for line in open(f"/proc/{pid}/io").read().splitlines())
        stat = open(f"/proc/{pid}/stat").read().rsplit(")", 1)[1].split()
        return int(d["write_bytes"]), int(stat[11]) + int(stat[12])
    except (OSError, KeyError, IndexError):
        return 0, 0
out = open("/tmp/probe2.jsonl", "a")
while not os.path.exists("/tmp/probe2.stop"):
    row = {"t": round(time.time(), 3), "stage": open("/tmp/stage").read().strip() if os.path.exists("/tmp/stage") else ""}
    try:
        sockets = {**dump(socket.AF_INET), **dump(socket.AF_INET6)}
        row["sock_diag"] = True
    except OSError as error:
        sockets, row["sock_diag"] = {}, f"refused: {error}"
    who, tensord = owners()
    for name in ("weights", "install", "probe"):
        mine = [v for k, v in sockets.items() if who.get(k) == name]
        row[name] = {"n": len(mine), "bytes": sum(v[3] for v in mine), "rtt_us": sorted(v[0] for v in mine), "rcv_rtt_us": sorted(v[1] for v in mine),
                     "min_rtt_us": min((v[2] for v in mine if v[2]), default=0), "retrans": sum(v[5] for v in mine)}
    written, ticks = (sum(x) for x in zip(*[io(p) for p in tensord])) if tensord else (0, 0)
    row["tensord"] = {"pids": len(tensord), "write_bytes": written, "cpu_ticks": ticks}
    mem = dict(line.split(":", 1) for line in open("/proc/meminfo"))
    row["dirty_kb"], row["writeback_kb"] = int(mem["Dirty"].split()[0]), int(mem["Writeback"].split()[0])
    # The container's memory pressure and its own dirty and writeback page-cache bytes (cgroup v2), where it can be read.
    try:
        some = open("/sys/fs/cgroup/memory.pressure").readline().split()
        row["memory_some_total_us"] = int(some[-1].split("=")[1])
        stat = dict(line.split() for line in open("/sys/fs/cgroup/memory.stat"))
        row["cgroup_dirty_kb"], row["cgroup_writeback_kb"] = int(stat.get("file_dirty", 0)) // 1024, int(stat.get("file_writeback", 0)) // 1024
    except (OSError, ValueError, IndexError):
        pass
    disks = {}
    for line in open("/proc/diskstats"):
        f = line.split()
        if not f[2].startswith(("loop", "ram")) and int(f[7]):
            disks[f[2]] = [int(f[9]) * 512, int(f[12])]
    row["disks"] = disks
    rows = open("/proc/net/dev").read().splitlines()[2:]
    row["link_bytes"] = sum(int(r.split(":", 1)[1].split()[0]) for r in rows if r.split(":", 1)[0].strip() != "lo")
    out.write(json.dumps(row) + "\n"); out.flush()
    time.sleep(1)
PY
)
# The Hub's connect, TLS and total time each second (control latency), with the stage.
CONTROL_SH='while [ ! -e /tmp/probe2.stop ]; do printf "%s %s %s\n" "$(date +%s.%N | cut -c1-14)" "$(cat /tmp/stage 2>/dev/null)" "$(curl -s -o /dev/null -m 20 -w "%{time_connect} %{time_appconnect} %{time_total} %{http_version} %{http_code}" https://tensorhub.com/readyz)" >> /tmp/control.txt; sleep 1; done'
# The wheel's URL (cp312 manylinux x86_64) from PyPI's JSON, then a download of it: curl to /dev/null, its protocol and rate.
WHEEL_PY=$(cat <<'PY'
import json, sys, urllib.request
name, version = sys.argv[1].split("==")
files = json.load(urllib.request.urlopen(f"https://pypi.org/pypi/{name}/{version}/json"))["urls"]
pick = [f for f in files if f["filename"].endswith(".whl") and "cp312" in f["filename"] and "x86_64" in f["filename"] and "manylinux" in f["filename"]]
print(max(pick, key=lambda f: f["size"])["url"] if pick else "")
PY
)
# A live download's rate before, during and after TCP_WINDOW_CLAMP (64 KiB): http/1.1 over TLS on our own socket.
CLAMP_PY=$(cat <<'PY'
import socket, ssl, sys, time, urllib.parse
url = urllib.parse.urlparse(sys.argv[1])
raw = socket.create_connection((url.hostname, 443))
tls = ssl.create_default_context().wrap_socket(raw, server_hostname=url.hostname)
tls.sendall(f"GET {url.path} HTTP/1.1\r\nHost: {url.hostname}\r\nConnection: close\r\n\r\n".encode())
phases, start, got, at = [], time.time(), 0, time.time()
def rate(name):
    global got, at
    now = time.time(); phases.append((name, round(got / (now - at) / 1e6, 2))); got, at = 0, now
clamped = released = False
while True:
    data = tls.recv(1 << 16)
    if not data:
        break
    got += len(data)
    t = time.time() - start
    if t > 4 and not clamped:
        rate("before"); raw.setsockopt(socket.IPPROTO_TCP, socket.TCP_WINDOW_CLAMP, 64 << 10); clamped = True
    elif t > 10 and not released:
        rate("clamped_64k"); raw.setsockopt(socket.IPPROTO_TCP, socket.TCP_WINDOW_CLAMP, 64 << 20); released = True
    elif t > 16:
        rate("released"); break
print(phases)
PY
)
quote "$(param need_s 2400)"
pod() { timeout "${T:-120}" cozy machine ssh "$NAME" "$@"; }
stage() { T=30 pod "echo $1 > /tmp/stage" < /dev/null > /dev/null 2>&1; say "stage $1"; }
link_rate() {
  local a b
  a=$(T=30 pod "awk -F'[: ]+' 'NR>2 && \$2!=\"lo\" {s+=\$3} END {print s}' /proc/net/dev" < /dev/null 2> /dev/null | tr -d '\r')
  sleep "$1"
  b=$(T=30 pod "awk -F'[: ]+' 'NR>2 && \$2!=\"lo\" {s+=\$3} END {print s}' /proc/net/dev" < /dev/null 2> /dev/null | tr -d '\r')
  awk -v a="$a" -v b="$b" -v s="$1" 'BEGIN { printf "%.1f", (b - a) / s / 1e6 }'
}
for try in $(seq 1 "$TRIES"); do
  rent
  for i in $(seq 1 20); do pod true < /dev/null > /dev/null 2>&1 && break; [ "$i" = 20 ] && fail "no SSH into $NAME"; sleep 15; done
  say "try $try: $NAME ($RID): $(T=30 pod 'uname -r; nproc; free -g | sed -n 2p; df -h / | tail -1' < /dev/null 2> /dev/null | tr '\n' ' ' | tr -d '\r')"
  if [ "$KEEP" != 0 ]; then
    (cd "$RUN_PROJECT" && cozy model install "$MODEL" --on "$NAME" > "$OUT/sample-$try.txt" 2>&1) & W=$!
    sleep 25; rate=$(link_rate 20); kill "$W" 2> /dev/null
    say "try $try: $NAME ($RID) receives $rate MB/s with weights alone"
    if awk -v r="$rate" -v k="$KEEP" 'BEGIN { exit !(r >= k) }'; then
      say "try $try: not under $KEEP MB/s: ending $RID"
      cozy machine end "$RID" -y >> "$OUT/end.txt" 2>&1 || cozy machine end "$RID" -y --force >> "$OUT/end.txt" 2>&1
      continue
    fi
    # The sample's partial download is wiped so the model's stage starts cold.
    T=600 pod "cat > /tmp/reset.py && python3 /tmp/reset.py" < "$(dirname "$0")/empty-store.py" > "$OUT/reset.json" 2>&1 || say "the store wasn't emptied"
  fi
  say "kept $NAME ($RID)${rate:+ at $rate MB/s}"
  T=60 pod "rm -f /tmp/probe2.stop /tmp/probe2.jsonl /tmp/control.txt; echo setup > /tmp/stage" < /dev/null || fail "the probe's files"
  (printf '%s' "$SAMPLER_PY" | T=7200 pod "cat > /tmp/probe2.py && python3 /tmp/probe2.py 2> /tmp/probe2.err" > /dev/null 2>&1) &
  (T=7200 pod "$CONTROL_SH" < /dev/null > /dev/null 2>&1) &
  url=$(printf '%s' "$WHEEL_PY" | T=60 pod "cat > /tmp/wheel.py && python3 /tmp/wheel.py '$WHEEL'" 2> /dev/null | tr -d '\r' | tail -1)
  say "wheel $WHEEL: ${url:-none found}"
  wheel() { T=900 pod "curl -s -o /dev/null -w '%{http_version} %{size_download} %{time_total} %{speed_download}' '$url'" < /dev/null 2> /dev/null | tr -d '\r'; }
  stage idle; sleep 20
  [ -z "$url" ] || { stage wheel-alone; say "wheel alone: $(wheel)"; }
  stage weights-alone
  (cd "$RUN_PROJECT" && cozy model install "$MODEL" --on "$NAME" > "$OUT/weights.txt" 2>&1) & W=$!
  sleep 20
  if [ -n "$url" ] && kill -0 "$W" 2> /dev/null; then stage wheel-beside; say "wheel beside weights: $(wheel)"; fi
  if [ -n "$url" ] && kill -0 "$W" 2> /dev/null; then
    stage clamp; say "clamp (rate MB/s per phase): $(printf '%s' "$CLAMP_PY" | T=120 pod "cat > /tmp/clamp.py && python3 /tmp/clamp.py '$url'" 2>&1 | tr -d '\r' | tail -1)"
  fi
  wait "$W"; stage weights-done; say "weights: $(tail -2 "$OUT/weights.txt" | tr '\n' ' ' | cut -c1-200)"
  if [ -z "$url" ] || true; then
    [ -z "$url" ] || { stage clamp-alone; say "clamp alone (rate MB/s per phase): $(printf '%s' "$CLAMP_PY" | T=120 pod "cat > /tmp/clamp.py && python3 /tmp/clamp.py '$url'" 2>&1 | tr -d '\r' | tail -1)"; }
  fi
  for p in $PACKAGES; do
    stage "install-$p"; t0=$(date +%s)
    (cd "$RUN_PROJECT" && cozy package install "$p" --on "$NAME") > "$OUT/install-$p.txt" 2>&1 || say "install $p: $(tail -2 "$OUT/install-$p.txt" | tr '\n' ' ' | cut -c1-200)"
    say "installed $p in $(( $(date +%s) - t0 )) s"
  done
  stage local-beside
  (cd "$RUN_PROJECT" && cozy model install "${MODEL2:-$MODEL}" --on "$NAME" > "$OUT/weights-2.txt" 2>&1) & W=$!
  t0=$(date +%s)
  (cd "$RUN_PROJECT" && cozy run "$PKG/installed" --on "$NAME") > "$OUT/local.txt" 2>&1
  say "local package installed beside weights in $(( $(date +%s) - t0 )) s: $(tail -1 "$OUT/local.txt" | cut -c1-160)"
  if [ "$GPU" = true ]; then
    wait "$W"
    stage gpu-quiet
    (cd "$RUN_PROJECT" && cozy run sdxl/generate 'prompt=a copper kettle on a stove, photo' seed:=1 --on "$NAME") > "$OUT/gpu-quiet.txt" 2>&1
    say "sdxl quiet: $(grep -E '^time|steady' "$OUT/gpu-quiet.txt" | tr '\n' ' ' | cut -c1-200)"
    if [ -n "$MODEL2" ]; then
      stage gpu-beside
      (cd "$RUN_PROJECT" && cozy model install "$MODEL2" --on "$NAME" > "$OUT/weights-3.txt" 2>&1) & W=$!
      sleep 10
      (cd "$RUN_PROJECT" && cozy run sdxl/generate 'prompt=a copper kettle on a stove, photo' seed:=2 --on "$NAME") > "$OUT/gpu-beside.txt" 2>&1
      say "sdxl beside a weight write: $(grep -E '^time|steady' "$OUT/gpu-beside.txt" | tr '\n' ' ' | cut -c1-200)"
    fi
  fi
  kill "$W" 2> /dev/null
  stage done; sleep 2
  T=60 pod "touch /tmp/probe2.stop; sleep 2; cat /tmp/probe2.jsonl" > "$OUT/probe2.jsonl" 2> /dev/null < /dev/null || say "the probe wasn't read"
  T=60 pod "cat /tmp/control.txt" > "$OUT/control.txt" 2> /dev/null < /dev/null || true
  T=60 pod "cat /tmp/probe2.err" > "$OUT/probe2.err" 2> /dev/null < /dev/null || true
  cozy machine logs "$NAME" --lines 0 --since 2h > "$OUT/tensord.log" 2>&1 || true
  python3 "$(dirname "$0")/link-probe2.py" "$OUT" > "$OUT/probe2-summary.json" 2> "$OUT/probe2-summary.err" || say "summary failed: $(tail -c 300 "$OUT/probe2-summary.err")"
  say "summary: $(head -c 3000 "$OUT/probe2-summary.json")"
  break
done
