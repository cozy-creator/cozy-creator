# tfs-300's measurement cells (proof branch only; numbers for the download-system design, not a judge's bars).
#   link_trickle    a 4-segment trickle (TCP_WINDOW_CLAMP) beside a saturating flow in the 60 Mbit FIFO: its rate, and the
#                   sender's retransmissions and timeouts (ss -tin) — one and two trickle connections, alone and beside.
#   install_bytes   published packages installed on this fresh runner's own machine (no worker image's uv seed): the bytes
#                   uv's own sockets received per package (sock_diag), and how long each install took.

# A sampler of uv's sockets' received bytes each second (sock_diag; tcp_info bytes_received at 128), into <file>.
UVBYTES_PY=$(cat <<'PY'
import json, os, socket, struct, sys, time
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
                    found[inode] = struct.unpack_from("=Q", info, 128)[0]
                a += (size + 3) & ~3
            at += (length + 3) & ~3
seen, total, out = {}, 0, open(sys.argv[1], "a")
while not os.path.exists(sys.argv[1] + ".stop"):
    mine = set()
    for pid in filter(str.isdigit, os.listdir("/proc")):
        try:
            if os.path.basename(open(f"/proc/{pid}/cmdline", "rb").read().split(b"\0")[0]) != b"uv":
                continue
            for fd in os.listdir(f"/proc/{pid}/fd"):
                target = os.readlink(f"/proc/{pid}/fd/{fd}")
                if target.startswith("socket:["):
                    mine.add(int(target[8:-1]))
        except OSError:
            pass
    sockets = {**dump(socket.AF_INET), **dump(socket.AF_INET6)}
    for inode in mine & sockets.keys():
        total += max(0, sockets[inode] - seen.get(inode, 0))
        seen[inode] = sockets[inode]
    out.write(f"{time.time():.3f} {total}\n"); out.flush()
    time.sleep(1)
PY
)

install_bytes() {
[ -n "$LOCAL" ] || return 0
step "tfs-300: the third-party bytes each first-party package's install downloads on a fresh machine"
local dir=$OUT/install-bytes sampler p t0 t1 b0 b1 project=${RUNNER_TEMP:-$HOME}/install-bytes
[ -n "$PACKAGES" ] || { check "install-bytes: no packages checkout"; return 0; }
mkdir -p "$dir" && printf '%s' "$UVBYTES_PY" > "$dir/uvbytes.py"
rm -rf "$project" && cozy --json new --unencrypted "$project" > "$dir/new.json" 2>&1 || { check "install-bytes: cozy new: $(tail -c 300 "$dir/new.json")"; return 0; }
du -sh ~/.cache/uv 2> /dev/null | sed 's/^/install-bytes: uv cache before: /' || true
python3 "$dir/uvbytes.py" "$dir/uv.txt" > "$dir/uvbytes.err" 2>&1 & sampler=$!
for p in ${INSTALL_BYTES_PACKAGES:-sdxl krea-2 flux.3-klein hidream-o1}; do
  sleep 2; t0=$(date +%s); b0=$(tail -1 "$dir/uv.txt" 2> /dev/null | awk '{print $2}')
  (cd "$project" && timeout 1200 cozy package install "$PACKAGES/$p" --on local) > "$dir/install-$p.txt" 2>&1 || echo "install-bytes: $p: $(tail -2 "$dir/install-$p.txt" | tr '\n' ' ' | cut -c1-200)"
  sleep 2; t1=$(date +%s); b1=$(tail -1 "$dir/uv.txt" | awk '{print $2}')
  echo "install-bytes: $p: $(( (${b1:-0} - ${b0:-0}) / 1000000 )) MB by uv in $((t1 - t0)) s" | tee -a "$dir/summary.txt"
done
touch "$dir/uv.txt.stop"; sleep 2; kill "$sampler" 2> /dev/null || true
}

link_trickle() {
[ "$RUNNER_OS" = Linux ] || return 0
step "tfs-300: a 4-segment trickle beside a saturating flow in the 60 Mbit FIFO: its rate and the sender's timeouts"
local dir=$OUT/link-trickle port server
mkdir -p "$dir/www"
python3 -c "import os, sys; open(sys.argv[1], 'wb').write(os.urandom(800 << 20))" "$dir/www/big.bin"
port=$(python3 -c 'import socket; s = socket.socket(); s.bind(("127.0.0.1", 0)); print(s.getsockname()[1])')
python3 -m http.server "$port" --bind 127.0.0.1 --directory "$dir/www" > "$dir/server.log" 2>&1 & server=$!
sleep 1
sudo ip link set dev lo mtu 1500 && sudo tc qdisc add dev lo root handle 1: prio && sudo tc qdisc add dev lo parent 1:3 handle 30: tbf rate 60mbit burst 1mb latency 200ms \
  && sudo tc filter add dev lo parent 1: protocol ip prio 1 u32 match ip sport "$port" 0xffff flowid 1:3 || { check "link-trickle: the link wasn't made"; kill "$server"; return 0; }
# A reader whose window is clamped to <segments> × 1448 bytes from its first byte, reporting its bytes each second, for <seconds>.
cat > "$dir/trickle.py" <<'PY'
import socket, sys, time
port, segments, seconds, name = int(sys.argv[1]), int(sys.argv[2]), float(sys.argv[3]), sys.argv[4]
s = socket.socket()
if segments:
    s.setsockopt(socket.IPPROTO_TCP, socket.TCP_WINDOW_CLAMP, segments * 1448)
s.connect(("127.0.0.1", port))
s.sendall(b"GET /big.bin HTTP/1.1\r\nHost: x\r\n\r\n")
s.settimeout(1.0)
start, last, got, out = time.time(), time.time(), 0, open(f"{sys.argv[5]}/{name}.txt", "w")
while time.time() - start < seconds:
    try:
        data = s.recv(1 << 16)
        if not data:
            break
    except socket.timeout:
        data = b""
    got += len(data)
    if time.time() - last >= 1:
        out.write(f"{time.time() - start:.1f} {got}\n"); out.flush(); got, last = 0, time.time()
PY
# The sender's sockets each second: retransmissions, timeouts' backoff and rto, from ss -tin.
( while [ ! -e "$dir/stop" ]; do printf '%s ' "$(date +%s.%N | cut -c1-14)"; ss -tinH state established "( sport = :$port )" | tr '\n' ' '; echo; sleep 1; done ) > "$dir/ss.txt" 2>&1 &
local ssp=$!
python3 "$dir/trickle.py" "$port" 4 20 alone-1 "$dir"
python3 "$dir/trickle.py" "$port" 0 95 saturating "$dir" & local sat=$!
sleep 5
python3 "$dir/trickle.py" "$port" 4 40 beside-1 "$dir"
python3 "$dir/trickle.py" "$port" 4 40 beside-2a "$dir" & python3 "$dir/trickle.py" "$port" 4 40 beside-2b "$dir"
wait "$sat" 2> /dev/null || true
touch "$dir/stop"; sleep 1; kill "$ssp" "$server" 2> /dev/null || true
sudo tc qdisc del dev lo root 2> /dev/null || true; sudo ip link set dev lo mtu 65536 || true
python3 - "$dir" <<'PY' | tee "$dir/summary.txt"
import pathlib, re, statistics, sys
d = pathlib.Path(sys.argv[1])
for name in ("alone-1", "saturating", "beside-1", "beside-2a", "beside-2b"):
    f = d / f"{name}.txt"
    rates = [int(line.split()[1]) / 1e6 for line in f.read_text().splitlines()] if f.exists() else []
    zero = max((len(run) for run in re.findall(r"0+", "".join("0" if r == 0 else "1" for r in rates))), default=0)
    print(f"link-trickle: {name}: {statistics.mean(rates) if rates else 0:.3f} MB/s mean, longest zero stretch {zero} s over {len(rates)} s")
text = (d / "ss.txt").read_text()
retrans = [int(m) for m in re.findall(r"retrans:\d+/(\d+)", text)]
backoff = [int(m) for m in re.findall(r"backoff:(\d+)", text)]
rto = [float(m) for m in re.findall(r"rto:([\d.]+)", text)]
print(f"link-trickle: sender: total retransmits seen max {max(retrans, default=0)}, backoff max {max(backoff, default=0)}, rto max {max(rto, default=0)} ms")
PY
rm -f "$dir/www/big.bin"
}
