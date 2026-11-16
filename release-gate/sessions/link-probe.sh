# link-probe (tfs-300, measurement for the download-system design; proof branch only): on one host, while its machine
# downloads a model's weights and a local package installs a few hundred MB from PyPI at the same time, each second on the pod:
# every TCP socket's round trip (`tcp_info` rtt, rcv_rtt, min_rtt) and bytes, by owner (TensorD's weights, uv's install), read
# over sock_diag, which also answers whether sock_diag is open on this provider. Optionally the market is sampled for a slow
# host first: each host's link is read from its first 20 s of weights, and one faster than `keep_under_mb_s` is ended and
# another rented, at most `tries` times. The worker image's own TensorD is used (no share code: what the link does unpaced).
# params:
#   provider          runpod | vast ("" for any); `--provider` on `machine rent`
#   keep_under_mb_s   0 keeps the first host; else end a faster one and rent again
#   tries             hosts to sample at most (1)
#   model             the weights (stabilityai/sdxl)
#   install           the local package's extra requirement, downloaded from PyPI (tensorflow-cpu>=2.18,<2.19)
#   floor_mb_s        must be 0 when keep_under_mb_s is set: the point is a slow link
. "$(dirname "$0")/lib.sh"
PROVIDER=$(param provider '') KEEP=$(param keep_under_mb_s 0) TRIES=$(param tries 1) MODEL=$(param model stabilityai/sdxl)
EXTRA=$(param install 'tensorflow-cpu>=2.18,<2.19')
[[ $KEEP =~ ^[0-9]+$ && $TRIES =~ ^[0-9]+$ ]] || fail "keep_under_mb_s $KEEP, tries $TRIES"
[ "$KEEP" = 0 ] || [ "$FLOOR_MBS" = 0 ] || fail "keep_under_mb_s needs floor_mb_s 0: a slow link is the point"
[ -f ~/.ssh/id_ed25519.pub ] || ssh-keygen -q -t ed25519 -N '' -f ~/.ssh/id_ed25519 || fail "ssh-keygen"
RENT_ARGS="--ssh${PROVIDER:+ --provider $PROVIDER}"
PKG=$OUT/probe-pkg
mkdir -p "$PKG" && cat > "$PKG/probe.py" <<'PY'
import msgspec

from cozy_runtime.author import App

app = App()


class Ask(msgspec.Struct):
    pass


class Done(msgspec.Struct):
    done: bool


@app.entrypoint(accelerator=False)
def installed(payload: Ask) -> Done:
    return Done(done=True)
PY
cat > "$PKG/pyproject.toml" <<TOML
[project]
name = "tfs300-probe"
version = "0.0.0"
requires-python = ">=3.11"
dependencies = ["cozy-runtime>=0.45", "msgspec>=0.19", "$EXTRA"]

[project.entry-points."cozy.application"]
default = "probe:app"

[build-system]
requires = ["hatchling>=1.25"]
build-backend = "hatchling.build"

[tool.hatch.build.targets.wheel]
only-include = ["probe.py"]
TOML
# Each second on the pod: every TCP socket's (inode, rtt, rcv_rtt, min_rtt, bytes received) from one sock_diag dump per
# family, summed by owner: TensorD (`machine serve`) or uv. tcp_info offsets: rtt 68, rcv_rtt 92, bytes_received 128, min_rtt 148.
PROBE_PY=$(cat <<'PY'
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
                return found if kind == 3 else None
            body, a = data[at + 16:at + length], 72
            inode = struct.unpack_from("=I", body, 68)[0]
            while a + 4 <= len(body):
                size, attr = struct.unpack_from("=HH", body, a)
                if size < 4:
                    break
                info = body[a + 4:a + size]
                if attr == 2 and len(info) >= 136:
                    found[inode] = (struct.unpack_from("=I", info, 68)[0], struct.unpack_from("=I", info, 92)[0],
                                    struct.unpack_from("=I", info, 148)[0] if len(info) >= 152 else 0, struct.unpack_from("=Q", info, 128)[0])
                a += (size + 3) & ~3
            at += (length + 3) & ~3
def owners():
    out = {}
    for pid in filter(str.isdigit, os.listdir("/proc")):
        try:
            argv = open(f"/proc/{pid}/cmdline", "rb").read().split(b"\0")
            who = "weights" if b"machine" in argv and b"serve" in argv else "install" if os.path.basename(argv[0]) == b"uv" else None
            if who:
                for fd in os.listdir(f"/proc/{pid}/fd"):
                    target = os.readlink(f"/proc/{pid}/fd/{fd}")
                    if target.startswith("socket:["):
                        out[int(target[8:-1])] = who
        except OSError:
            pass
    return out
out = open("/tmp/probe.jsonl", "a")
while not os.path.exists("/tmp/probe.stop"):
    row = {"t": round(time.time(), 3)}
    try:
        sockets = {**(dump(socket.AF_INET) or {}), **(dump(socket.AF_INET6) or {})}
        row["sock_diag"] = True
    except OSError as error:
        sockets, row["sock_diag"] = {}, f"refused: {error}"
    who = owners()
    for name in ("weights", "install"):
        mine = [v for k, v in sockets.items() if who.get(k) == name]
        row[name] = {"n": len(mine), "bytes": sum(v[3] for v in mine), "rtt_us": sorted(v[0] for v in mine),
                     "rcv_rtt_us": sorted(v[1] for v in mine), "min_rtt_us": min((v[2] for v in mine if v[2]), default=0)}
    rows = open("/proc/net/dev").read().splitlines()[2:]
    row["link_bytes"] = sum(int(r.split(":", 1)[1].split()[0]) for r in rows if r.split(":", 1)[0].strip() != "lo")
    out.write(json.dumps(row) + "\n"); out.flush()
    time.sleep(1)
PY
)
quote 1800
pod() { timeout "${T:-120}" cozy machine ssh "$NAME" "$@"; }
# link_rate: MB/s the pod's interfaces received over `seconds` while its weights download.
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
  T=60 pod "rm -f /tmp/probe.stop /tmp/probe.jsonl" < /dev/null || fail "the probe's files"
  (printf '%s' "$PROBE_PY" | T=5400 pod "cat > /tmp/probe.py && python3 /tmp/probe.py 2> /tmp/probe.err" > /dev/null 2>&1) &
  say "try $try: $NAME ($RID): weights $MODEL asked"
  (cd "$RUN_PROJECT" && cozy model install "$MODEL" --on "$NAME" > "$OUT/weights-$try.txt" 2>&1) & W=$!
  # Sampling for a slow host: the link read from the weights alone first. Keeping any host: the install starts at once,
  # so a fast link's weights are still coming while it downloads.
  rate=''
  if [ "$KEEP" != 0 ]; then sleep 25; rate=$(link_rate 20); say "try $try: $NAME ($RID) receives $rate MB/s with its weights alone"; fi
  if [ "$KEEP" != 0 ] && awk -v r="$rate" -v k="$KEEP" 'BEGIN { exit !(r >= k) }'; then
    kill "$W" 2> /dev/null
    say "try $try: $rate MB/s is not under $KEEP: ending $RID and renting again"
    cozy machine end "$RID" -y >> "$OUT/end.txt" 2>&1 || cozy machine end "$RID" -y --force >> "$OUT/end.txt" 2>&1
    continue
  fi
  say "try $try: kept $NAME ($RID)${rate:+ at $rate MB/s}; the install starts beside the weights"
  ASK=$(date +%s)
  (cd "$RUN_PROJECT" && cozy run "$PKG/installed" --on "$NAME") > "$OUT/install.txt" 2>&1
  say "install done in $(( $(date +%s) - ASK )) s: $(tail -2 "$OUT/install.txt" | tr '\n' ' ' | cut -c1-200)"
  wait "$W"; say "weights: $(tail -2 "$OUT/weights-$try.txt" | tr '\n' ' ' | cut -c1-200)"
  T=60 pod "touch /tmp/probe.stop; sleep 2; cat /tmp/probe.jsonl" > "$OUT/probe.jsonl" 2> /dev/null < /dev/null || say "the probe wasn't read"
  T=30 pod "cat /tmp/probe.err" > "$OUT/probe.err" 2> /dev/null < /dev/null || true
  python3 - "$OUT/probe.jsonl" > "$OUT/probe-summary.json" <<'PY'
import json, statistics, sys
rows = [json.loads(line) for line in open(sys.argv[1]) if line.startswith("{")]
med = lambda xs: statistics.median(xs) if xs else None
out, last = [], None
for row in rows:
    if last:
        dt = row["t"] - last["t"]
        rate = lambda k: max(0, row[k]["bytes"] - last[k]["bytes"]) / dt / 1e6
        out.append({"t": round(row["t"] - rows[0]["t"]), "weights_mb_s": round(rate("weights"), 1), "install_mb_s": round(rate("install"), 1),
                    "link_mb_s": round(max(0, row["link_bytes"] - last["link_bytes"]) / dt / 1e6, 1), "weights_n": row["weights"]["n"],
                    "weights_rcv_rtt_ms": med(row["weights"]["rcv_rtt_us"]) and med(row["weights"]["rcv_rtt_us"]) / 1e3,
                    "weights_rtt_ms": med(row["weights"]["rtt_us"]) and med(row["weights"]["rtt_us"]) / 1e3,
                    "weights_min_rtt_ms": row["weights"]["min_rtt_us"] / 1e3})
    last = row
installing = [r for r in out if r["install_mb_s"] > 0.5]
alone = [r for r in out if r["install_mb_s"] == 0 and r["weights_mb_s"] > 0.5]
pick = lambda rs, k: med([r[k] for r in rs if r[k] is not None])
print(json.dumps({"sock_diag": sorted({str(r.get("sock_diag")) for r in rows}), "seconds": len(out),
    "weights_alone": {"s": len(alone), "mb_s": pick(alone, "weights_mb_s"), "rcv_rtt_ms": pick(alone, "weights_rcv_rtt_ms"), "rtt_ms": pick(alone, "weights_rtt_ms")},
    "with_install": {"s": len(installing), "weights_mb_s": pick(installing, "weights_mb_s"), "install_mb_s": pick(installing, "install_mb_s"),
                     "rcv_rtt_ms": pick(installing, "weights_rcv_rtt_ms"), "rtt_ms": pick(installing, "weights_rtt_ms")},
    "min_rtt_ms": min((r["weights_min_rtt_ms"] for r in out if r["weights_min_rtt_ms"]), default=None), "series": out}, indent=None))
PY
  say "probe: $(python3 -c 'import json,sys; d=json.load(open(sys.argv[1])); d.pop("series"); print(json.dumps(d))' "$OUT/probe-summary.json")"
  break
done
