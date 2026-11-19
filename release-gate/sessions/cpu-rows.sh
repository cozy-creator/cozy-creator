# cpu-rows (cl-1279; Paul, 11-18: CPU rows are "for workloads that can run without a GPU"): a function's CPU benchmark row, measured
# on this runner's own machine, which has no GPU. No rental. The function, the Hub's release or with `tree` its package's source in
# that packages_refs checkout (an unpublished version: `cozy run <checkout>/<package>/<function>`, as e2e runs one), is run through
# `cozy run … --on local` on one picture (its first call here, a fresh executor: cold), then on another (warm), each watched by a
# sampler of the machine's executors: the most host memory they held and the most cores they used. Prints the row as its package's
# `benchmarks.json` takes it (`gpus: 0`; api::estimate::Benchmark) in results.txt and rows.json. It judges only that both runs
# completed on `local`. The row names the lane its run's record says its main slot ran, and its model (`org/name@release`) where
# `--model` picked one: a row naming none is the slot's default's.
#   gh workflow run pod-session.yaml -R cozy-creator/cozy-creator -f hub=prod -f script=cpu-rows -f sku=rtx-4090 -f disk=50 -f cap=1 \
#     -f packages_refs=pk=<packages ref> -f params='{"tree": "pk", "function": "qwen3/caption", "mention": "instructions", "args": […]}'
# params:
#   function  the function, as `cozy run` names it (default birefnet/remove_background)
#   tree      the packages_refs checkout its package runs from (default "": the Hub's release)
#   field     its file field, given a 1024×1024 picture (default image, none with a mention; "" for none)
#   mention   a words field given `<picture>` instead, the picture sent as the run's own reference, as a page's box sends one (default "")
#   args      more `cozy run` arguments, a JSON list: the request and `--model` (default [])
. "$(dirname "$0")/lib.sh"
fail() {
  cozy machine logs local tensord --since 2h -n 0 > "$OUT/tensord-local.log" 2>&1
  say "FAIL: $*"; RC=1; exit 1
}
FUNCTION=$(param function birefnet/remove_background) MENTION=$(param mention) TREE=$(param tree)
FIELD=$(param field "$([ -n "$MENTION" ] || echo image)")
[[ $FUNCTION =~ ^[a-z0-9._-]+/[a-z0-9_-]+$ && $FIELD =~ ^[a-z0-9_.]*$ && $MENTION =~ ^[a-z0-9_.]*$ && $TREE =~ ^[A-Za-z0-9_-]*$ ]] ||
  fail "function $FUNCTION, field $FIELD, mention $MENTION, tree $TREE"
[ -z "$FIELD" ] || [ -z "$MENTION" ] || fail "the picture goes to field $FIELD or is mentioned in $MENTION, not both"
mapfile -t ARGS < <(param args '[]' | jq -r '.[]') || fail "args isn't a JSON list"
for tool in jq ffmpeg python3 lscpu; do command -v "$tool" > /dev/null || fail "no $tool"; done
TARGET=$FUNCTION FROM="the Hub's release"
if [ -n "$TREE" ]; then
  SOURCE=$(cd "${PACKAGES:-}/$TREE/${FUNCTION%%/*}" 2> /dev/null && pwd) && [ -f "$SOURCE/pyproject.toml" ] ||
    fail "no ${FUNCTION%%/*} in packages_refs checkout '$TREE'"
  kernels "$SOURCE"
  TARGET=$SOURCE/${FUNCTION##*/} FROM="packages $(git -C "$SOURCE" rev-parse --short=10 HEAD 2> /dev/null) ($TREE)"
fi
mkdir -p "$OUT/runs"
for n in 1 2 3; do
  nice -n 19 ffmpeg -v error -y -f lavfi -i "testsrc2=size=1024x1024:rate=1" -vf "hue=h=$((n * 90))" -frames:v 1 "$OUT/picture-$n.png" || fail "no picture $n"
done
for _ in $(seq 60); do
  cozy --json machine show local > "$OUT/local.json" 2> "$OUT/local.err"
  [ "$(jq -r '.status.phase // empty' "$OUT/local.json" 2> /dev/null)" = ready ] && break
  sleep 5
done
[ "$(jq -r '.status.phase // empty' "$OUT/local.json")" = ready ] || fail "local isn't ready: $(tail -c 300 "$OUT/local.err")"
[ "$(jq '.status.gpus // [] | length' "$OUT/local.json")" = 0 ] || fail "local has a GPU: a CPU row is measured on a machine with none"
CPU=$(lscpu | sed -n 's/^Model name: *//p' | head -1) CORES=$(lscpu -p=core,socket | grep -v '^#' | sort -u | wc -l)
[ -n "$CPU" ] && [ "$CORES" -gt 0 ] || fail "this runner's CPU isn't read: $(lscpu | head -20)"
say "cozy $(cozy --version | awk '{print $2}'); $FUNCTION from $FROM; this runner: $CPU, $CORES cores, $(nproc) threads, $(awk '/MemTotal/ { printf "%.1f", $2 / 1048576 }' /proc/meminfo) GiB RAM"
(cd "$RUN_PROJECT" && cozy --json machine use local) > "$OUT/use.json" 2>&1 || fail "machine use local: $(tail -c 300 "$OUT/use.json")"

# sampled <name> <cozy args…>: the call followed to its end while the machine's executors (every process under its TensorD; where
# its pid isn't read, this user's python processes) are read twice a second: <name>.sample.json holds the most memory they held
# together and the most cores they used.
sampled() {
  local n=$1 code; shift
  python3 -I - "$OUT/$n.sample.json" "$HOME/.cozy/machine/var/lib/cozy/machine/agent.lock.holder" <<'PY' & local sampler=$!
import json, os, signal, sys, time
out, holder = sys.argv[1:3]
tick, page = os.sysconf("SC_CLK_TCK"), os.sysconf("SC_PAGE_SIZE")
peak = {"peak_host_bytes": 0, "cores": 0.0, "samples": 0}
def done(*_):
    json.dump(peak, open(out, "w")); sys.exit(0)
signal.signal(signal.SIGTERM, done)
def under(root):
    kids, stats, pythons = {}, {}, {}
    for pid in filter(str.isdigit, os.listdir("/proc")):
        try:
            name, rest = open(f"/proc/{pid}/stat").read().split("(", 1)[1].rsplit(")", 1)
            mine = os.stat(f"/proc/{pid}").st_uid == os.getuid()
        except OSError:
            continue
        fields = rest.split()
        kids.setdefault(int(fields[1]), []).append(int(pid))
        stats[int(pid)] = (int(fields[11]) + int(fields[12]), int(fields[21]) * page)
        if mine and name.startswith("python") and int(pid) != os.getpid():
            pythons[int(pid)] = stats[int(pid)]
    if root is None:
        return pythons
    todo, found = list(kids.get(root, [])), {}
    while todo:
        pid = todo.pop()
        found[pid] = stats[pid]
        todo += kids.get(pid, [])
    return found
last, at = {}, time.monotonic()
while True:
    time.sleep(0.5)
    try:
        root = int(json.load(open(holder))["pid"])
    except (OSError, ValueError, KeyError, TypeError):
        root = None
    peak["by"] = "this user's python processes" if root is None else "the processes under TensorD"
    now, seen = time.monotonic(), under(root)
    used = sum(ticks - last[pid][0] for pid, (ticks, _) in seen.items() if pid in last) / tick / (now - at)
    peak["peak_host_bytes"] = max(peak["peak_host_bytes"], sum(rss for _, rss in seen.values()))
    peak["cores"], peak["samples"] = max(peak["cores"], used), peak["samples"] + 1
    last, at = seen, now
PY
  (cd "$RUN_PROJECT" && cozy --json "$@") > "$OUT/$n.ndjson" 2> "$OUT/$n.err"; code=$?
  kill -TERM "$sampler" 2> /dev/null; wait "$sampler" 2> /dev/null
  [ "$code" = 0 ] || fail "$n: cozy $* exited $code: $(tail -c 400 "$OUT/$n.err") $(tail -c 300 "$OUT/$n.ndjson")"
  local number; number=$(handle "$OUT/$n.ndjson")
  [ -n "$number" ] || fail "$n: no run in its events: $(tail -c 300 "$OUT/$n.ndjson")"
  (cd "$RUN_PROJECT" && cozy --json --full run watch "$number" --once) > "$OUT/runs/$n.json" 2> "$OUT/runs/$n.err"
  [ "$(jq -r '.status' "$OUT/runs/$n.json")" = completed ] && [ "$(jq -r '.machine' "$OUT/runs/$n.json")" = local ] \
    || fail "$n: $(jq -c '{status, machine, reason: .reason.message}' "$OUT/runs/$n.json")"
  say "$n: $(jq -c --slurpfile s "$OUT/$n.sample.json" 'def s(a; b): if a != null and b != null then ((b - a) / 100 | round / 10) else null end;
    {run: .name, target, execution_ms, queued_s: s(.created_at_ms; .accepted_at_ms), preparing_s: s(.accepted_at_ms; .started_at_ms), ran_s: s(.started_at_ms; .settled_at_ms),
     memoized: (.memo != null), sampled: $s[0], measured_memory: (.measurements.memory // null | if . then {peak_allocated_bytes, peak_reserved_bytes} else null end)}' "$OUT/runs/$n.json" | cut -c1-900)"
}

# measured <name> <picture n>: one run of the function on picture n, in its file field or mentioned.
measured() {
  local picture=$OUT/picture-$2.png given=()
  if [ -n "$MENTION" ]; then given=("$MENTION=<picture>" "references:=[{\"name\": \"picture\", \"image\": \"$picture\"}]")
  elif [ -n "$FIELD" ]; then given=("--file=$FIELD=$picture"); fi
  sampled "$1" run "$TARGET" "${given[@]}" "${ARGS[@]}" --on local
}
measured cold 1
measured warm 2
measured warm-again 3

cozy --json machine show local > "$OUT/local-after.json" 2>&1
say "what local timed: $(jq -c '[.status.times[]? | {function, runs}]' "$OUT/local-after.json" 2> /dev/null | cut -c1-900)"
NOTE="pod-session ${GITHUB_RUN_ID:-local}: GitHub ubuntu-24.04 runner, $(nproc) threads; warm again $(jq -r .execution_ms "$OUT/runs/warm-again.json") ms"
[ -z "$TREE" ] || NOTE="$NOTE; ${FUNCTION%%/*} from $FROM"
jq -n --arg function "${FUNCTION##*/}" --arg cpu "$CPU" --argjson cores "$CORES" --arg cozy "$(cozy --version | awk '{print $2}')" \
  --arg runtime "$(jq -r '.status.runtime // empty' "$OUT/local-after.json")" --arg note "$NOTE" \
  --argjson picked "$(printf '%s\n' "${ARGS[@]}" | grep -qx -- --model && echo true || echo false)" --slurpfile cold "$OUT/runs/cold.json" --slurpfile warm "$OUT/runs/warm.json" --slurpfile cs "$OUT/cold.sample.json" --slurpfile ws "$OUT/warm.sample.json" \
  '([$cold[0].measurements.models[]?, $cold[0].calls[]?.measurements.models[]?] | first // {}) as $m
   | [{function: $function, gpus: 0, cpu: $cpu, cores: $cores, threads: ([$cs[0].cores, $ws[0].cores] | max | ceil),
     request: {}, lane: ($m.lane // ""), model: (if $picked and $m.repository then "\($m.repository)@\($m.release)" else "" end),
     warm_ms: $warm[0].execution_ms, cold_ms: $cold[0].execution_ms,
     peak_host_bytes: ([$cs[0].peak_host_bytes, $ws[0].peak_host_bytes] | max),
     ran_on: {cozy: $cozy, runtime: $runtime}, note: $note} | with_entries(select((.value != 0 and .value != "") or .key == "gpus"))]' > "$OUT/rows.json" || fail "the row wasn't written"
say "the CPU row of $FUNCTION:"; jq -c '.[]' "$OUT/rows.json" | tee -a "$R"
jq -e '.[0] | .warm_ms > 0 and .cold_ms > 0' "$OUT/rows.json" > /dev/null || fail "the row lacks a time"
jq -e '.[0] | .peak_host_bytes > 0 and .threads > 0' "$OUT/rows.json" > /dev/null || say "the sampler read no memory or cores: $(cat "$OUT/cold.sample.json" "$OUT/warm.sample.json" 2> /dev/null | tr '\n' ' ')"
say "PASS: $FUNCTION from $FROM ran cold and warm on this runner's CPU"
