# cl-1172 (Paul, 11-17, the LLM plan): every run spent about 2 s outside the GPU before and after its work (32 one-at-a-time
# captions took 88 s on a 4090, 0.86 s of GPU each). A warm repeat's time outside its compute is under 0.3 s on this computer.

# run_overhead: N warm repeats of two real functions on this computer's machine, each `cozy run` timed from its start to its exit:
# minimax-h3-tools' echo (published, no model) and Qwen3.5's write on the real 0.8B (published, a model held), each repeat asking
# something new so none is answered from the memo. A repeat's overhead is its wall time less its record's execution time; the
# median of each function's repeats is under OVERHEAD_MS.
OVERHEAD_MS=${OVERHEAD_MS:-300}
OVERHEAD_N=${OVERHEAD_N:-10}
run_overhead() {
[ -n "$LOCAL" ] || return 0
step "a warm run's time outside its compute (cl-1172)"
local package=qwen3.5
[ -z "$PACKAGES" ] || [ ! -d "$PACKAGES/qwen3" ] || package=qwen3
overhead_of echo minimax-h3-tools/echo text
overhead_of write "$package/write" instructions --model model=qwen/qwen3.5-0.8b@0.1.0 max_tokens:=4 seed:=7 temperature:=0
}

# overhead_of <name> <target> <text field> [more inputs]: one run to install and load, then OVERHEAD_N timed; checks their median.
overhead_of() {
local name=$1 target=$2 field=$3 i
shift 3
timed_run "$name-0" "$target" "$field=Say one word." "$@" || { check "overhead $name: its first run failed: $(tail -c 400 "$OUT/overhead-$name-0.err")"; return; }
for i in $(seq 1 "$OVERHEAD_N"); do
  timed_run "$name-$i" "$target" "$field=Say one word about the number $i." "$@" \
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
