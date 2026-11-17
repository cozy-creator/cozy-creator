# cl-1172 (Paul, 11-17, the LLM plan): every run spent about 2 s outside the GPU before and after its work (32 one-at-a-time
# captions took 88 s on a 4090, 0.86 s of GPU each). A warm repeat's time outside its compute is under 0.3 s on this computer,
# and as close as the network allows on another computer's machine.

# run_overhead: OVERHEAD_N warm repeats of real functions, each `cozy run` timed from its start to its exit, less its record's
# execution time: minimax-h3-tools' echo (published, no model), and Qwen3.5 on the real 0.8B (published, a model held): `write`
# (words from words) and `caption` (a new picture each repeat, as boxes on a page are captioned). Each repeat asks something new,
# so none is answered from the memo. On this computer's machine the median of each is under OVERHEAD_MS; on another computer's
# (overhead_far), under OVERHEAD_MS plus OVERHEAD_TRIPS round trips of its link.
OVERHEAD_MS=${OVERHEAD_MS:-300}
OVERHEAD_N=${OVERHEAD_N:-10}
OVERHEAD_RTT_MS=${OVERHEAD_RTT_MS:-100}
OVERHEAD_TRIPS=${OVERHEAD_TRIPS:-2}
run_overhead() {
[ -n "$LOCAL" ] || return 0
step "a warm run's time outside its compute (cl-1172)"
local package=qwen3.5
[ -z "$PACKAGES" ] || [ ! -d "$PACKAGES/qwen3" ] || package=qwen3
local qwen=(--model model=${OVERHEAD_MODEL:-huihui-ai/huihui-qwen3.5-0.8b-abliterated@0.1.0} max_tokens:=4 seed:=7 temperature:=0)
overhead_of echo minimax-h3-tools/echo text
overhead_of write "$package/write" instructions "${qwen[@]}"
overhead_of caption "$package/caption" picture "${qwen[@]}"
overhead_far "$package"
}

# overhead_far <package>: the same on another computer's machine OVERHEAD_RTT_MS away: another user of this runner, added here as
# another computer's machine is (machineset.sh's stand_in), its port's packets each way held OVERHEAD_RTT_MS/2 (tc netem on the
# loopback), as a rental's link holds them.
overhead_far() {
[ "${RUNNER_OS:-}" = Linux ] && [ "${GITHUB_ACTIONS:-}" = true ] \
  || { echo "overhead far: its machine is another user of a fresh GitHub runner"; return; }
step "a warm run's time outside its compute on another computer's machine, $OVERHEAD_RTT_MS ms away (cl-1172)"
local port bar=$((OVERHEAD_MS + OVERHEAD_TRIPS * OVERHEAD_RTT_MS))
cozy --json machine show local > "$OUT/set-local.json" || { check "overhead far: machine show local: $(head -c 300 "$OUT/set-local.json")"; return; }
stand_in far || return
port=$(jq -r '.machine.reach // empty' "$OUT/far-authorize.json" | grep -oE '[0-9]+$')
[ -n "$port" ] || { check "overhead far: no port in its reach: $(head -c 300 "$OUT/far-authorize.json")"; return; }
sudo tc qdisc add dev lo root handle 1: prio \
  && sudo tc qdisc add dev lo parent 1:3 handle 30: netem delay "$((OVERHEAD_RTT_MS / 2))ms" \
  && sudo tc filter add dev lo parent 1: protocol ip prio 1 u32 match ip dport "$port" 0xffff flowid 1:3 \
  && sudo tc filter add dev lo parent 1: protocol ip prio 1 u32 match ip sport "$port" 0xffff flowid 1:3 \
  || { sudo tc qdisc del dev lo root 2> /dev/null; check "overhead far: its link wasn't delayed"; return; }
local qwen=(--model model=${OVERHEAD_MODEL:-huihui-ai/huihui-qwen3.5-0.8b-abliterated@0.1.0} max_tokens:=4 seed:=7 temperature:=0)
OVERHEAD_ON=far OVERHEAD_MS=$bar overhead_of far-echo minimax-h3-tools/echo text
OVERHEAD_ON=far OVERHEAD_MS=$bar overhead_of far-caption "$1/caption" picture "${qwen[@]}"
sudo tc qdisc del dev lo root || check "overhead far: its link wasn't restored"
timeout 60 cozy --json machine end far --force -y > "$OUT/far-end.json" 2>&1 || true
}

# overhead_of <name> <target> <field> [more inputs]: one run to install and load, then OVERHEAD_N timed; checks their median. A
# `text` or `instructions` field asks words, new each repeat; `picture` gives `images` a new picture each repeat.
overhead_of() {
local name=$1 target=$2 field=$3 i input
shift 3
for i in $(seq 0 "$OVERHEAD_N"); do
  input=$(overhead_input "$name" "$field" "$i") || { check "overhead $name: no input for run $i"; return; }
  timed_run "$name-$i" "$target" "$input" "$@" \
    || { check "overhead $name: run $i failed: $(tail -c 400 "$OUT/overhead-$name-$i.err")"; return; }
done
python3 -I - "$OUT" "$name" "$OVERHEAD_N" "$OVERHEAD_MS" <<'PY' || check "$(tail -n 1 "$OUT/overhead-$name.txt")"
import json, statistics, sys
from pathlib import Path
out, name, n, bar = Path(sys.argv[1]), sys.argv[2], int(sys.argv[3]), float(sys.argv[4])
rows = []
for i in range(1, n + 1):
    wall = float((out / f"overhead-{name}-{i}.ms").read_text())
    lines = [json.loads(line) for line in (out / f"overhead-{name}-{i}.ndjson").read_text().splitlines() if line.startswith("{")]
    run = next(line["run"] for line in reversed(lines) if isinstance(line.get("run"), dict))
    execution = float(run.get("execution_ms") or 0)
    created, started, settled = run["created_at_ms"], run.get("started_at_ms") or 0, run.get("settled_at_ms") or 0
    rows.append((i, wall, execution, wall - execution, started - created if started else -1, settled - created if settled else -1))
lines = [f"{name}: run  wall_ms  execution_ms  overhead_ms  created_to_started_ms  created_to_settled_ms"]
lines += [f"{name}: {i:3d}  {w:7.0f}  {e:12.0f}  {o:11.0f}  {s:21.0f}  {t:21.0f}" for i, w, e, o, s, t in rows]
median = statistics.median(row[3] for row in rows)
said = f"overhead {name}: median {median:.0f} ms outside compute over {n} warm runs (bar {bar:.0f} ms)"
lines.append(said)
(out / f"overhead-{name}.txt").write_text("\n".join(lines) + "\n")
print("\n".join(lines))
sys.exit(0 if median < bar else 1)
PY
}

# overhead_input <name> <field> <i>: repeat i's input: words naming i, or a picture of its own colour (a 96x64 PNG).
overhead_input() {
case $2 in
  picture)
    local file=$OUT/overhead-inputs/$1-$3.png
    mkdir -p "${file%/*}"
    [ -f "$file" ] || python3 -I -c '
import struct, sys, zlib
i, w, h = int(sys.argv[2]), 96, 64
pixel = bytes(((40 * i) % 256, (97 * i) % 256, (180 + 7 * i) % 256))
rows = b"".join(b"\0" + pixel * w for _ in range(h))
chunk = lambda kind, data: struct.pack(">I", len(data)) + kind + data + struct.pack(">I", zlib.crc32(kind + data))
open(sys.argv[1], "wb").write(b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", struct.pack(">IIBBBBB", w, h, 8, 2, 0, 0, 0)) + chunk(b"IDAT", zlib.compress(rows)) + chunk(b"IEND", b""))
' "$file" "$3" || return 1
    printf 'images:=["%s"]' "$file" ;;
  *) printf '%s=Say one word about the number %s.' "$2" "$3" ;;
esac
}

# timed_run <cell> <target> [inputs]: one `cozy --json run` on OVERHEAD_ON (local) from the run project, its wall time in <cell>.ms.
timed_run() {
local cell=overhead-$1 began ended code=0
shift
began=$(date +%s%N)
(cd "$RUN_PROJECT" && cozy --json run "$@" --on "${OVERHEAD_ON:-local}") > "$OUT/$cell.ndjson" 2> "$OUT/$cell.err" || code=$?
ended=$(date +%s%N)
echo $(((ended - began) / 1000000)) > "$OUT/$cell.ms"
return "$code"
}
