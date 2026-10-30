# one-executor: xs-113's red/green on one rental. release-gate/suites/tensord/idle-entrypoints.sh runs once per TensorD
# arm, in order, on the same pod under the session's Runtime: the release keys an executor per construction, xs-113 one
# per package environment, lane and degree. After each cell the pod's executor processes (pid, parent, /dev/nvidia
# descriptors: CUDA contexts) and each GPU's bytes in use are kept as <arm>/<cell>.gpu.json; idle_entrypoints.py judges
# each arm (<arm>/expect.tsv) and summary.tsv sets the arms side by side. params:
#   arms        [{"name": "release", "tensord": "image", "only": "idle-klein-a,idle-klein-b"},
#               {"name": "one", "tensord": "<dev-build run id>"}]; tensord `image` (no update), a release version or a
#               dev-build run id; `only` the arm's cells (default: the session's)
#   runtime     a candidate Runtime wheel, https://…#sha256=<hex>, put on before any package installs (optional)
#   tensorfs    a TensorFS wheel the same way, when the Runtime's floor is not on PyPI (optional)
#   gpus        GPUs rented, and the H3 cells' group (1)
#   only        the cells, comma-separated (default: every cell)
#   models      downloaded first, so no arm's call waits on its weights
#   traffic_gb  GB the provider bills (75)
#   need_s      seconds the guard must buy (4500)
. "$(dirname "$0")/lib.sh"
HERE=$(cd "$(dirname "$0")/.." && pwd)
ARMS=$(param arms '[{"name": "release", "tensord": "image"}]') URL=$(param runtime) GPUS=$(param gpus 1)
MODELS=$(param models '["black-forest-labs/flux-3-klein-base@0.1.0 --lane bf16", "black-forest-labs/flux-3-klein@0.1.0 --lane bf16", "minimaxai/minimax-h3@1.0.0-rc.6", "alibaba-pai/minimax-h3-turbo-lora@1.0.1"]')
mapfile -t names < <(jq -r '.[].name' <<<"$ARMS")
mapfile -t builds < <(jq -r '.[].tensord' <<<"$ARMS")
[ "${#names[@]}" -gt 0 ] || fail "no arms"
[ -f "$HERE/suites/tensord/idle-entrypoints.sh" ] && [ -f "$HERE/../e2e/run.sh" ] || fail "no idle-entrypoints cell or e2e/run.sh"
bins=()
for i in "${!names[@]}"; do
  [[ ${names[$i]} =~ ^[a-z0-9-]+$ ]] || fail "arm name ${names[$i]}"
  if [ "${builds[$i]}" = image ]; then bins+=(""); continue; fi
  if [[ ${builds[$i]} =~ ^[0-9]+\.[0-9]+\.[0-9]+ ]]; then bins+=("${builds[$i]}"); continue; fi
  d=$OUT/bin/${names[$i]}; mkdir -p "$d"
  gh run download "${builds[$i]}" -R cozy-creator/cozy-creator -p 'cozy-*' -D "$d" >/dev/null 2>&1 || fail "no cozy artifact in run ${builds[$i]}"
  b=$(ls "$d"/cozy-*/cozy-linux-x86_64 2>/dev/null | head -1); [ -n "$b" ] || fail "run ${builds[$i]} has no cozy-linux-x86_64"
  chmod +x "$b"; say "arm ${names[$i]}: $("$b" --version)"; bins+=("$b")
done
# wheel <https://…#sha256=hex>: the wheel fetched here under its own name (a URL's %2B is a local version's +).
wheel() {
  [[ $1 == https://*'#sha256='* ]] || fail "$1 is not https://…#sha256=<hex>"
  local w; w=$OUT/wheel/$(basename "${1%%#*}"); w=$(printf '%b' "${w//%/\\x}"); mkdir -p "$OUT/wheel"
  curl -fsSL --retry 5 --retry-all-errors "${1%%#*}" -o "$w" || fail "cannot fetch ${1%%#*}"
  [ "$(sha256sum "$w" | cut -c1-64)" = "${1##*sha256=}" ] || fail "$w sha256"
  echo "$w"
}
W='' TW=''
[ -z "$URL" ] || W=$(wheel "$URL") || exit 1
[ -z "$(param tensorfs)" ] || TW=$(wheel "$(param tensorfs)") || exit 1
# The pod's executors and GPUs now, as JSON: [{pid, ppid, nvidia_fds, rss_mib, age_s}], [{gpu, used_mib, total_mib}].
POD_PY=$(cat <<'EOF'
import glob, json, os, subprocess, time
rows = []
for d in glob.glob("/proc/[0-9]*"):
    try:
        if b"cozy_runtime.internal.executor" not in open(d + "/cmdline", "rb").read():
            continue
        status = dict(l.split(":", 1) for l in open(d + "/status").read().splitlines() if ":" in l)
        fds = [os.readlink(f) for f in glob.glob(d + "/fd/*") if os.path.exists(f)]
        rows.append({"pid": int(d[6:]), "ppid": int(status["PPid"]), "nvidia_fds": sum("/dev/nvidia" in f for f in fds),
                     "rss_mib": int(status["VmRSS"].split()[0]) // 1024, "age_s": round(time.time() - os.stat(d).st_mtime)})
    except (OSError, KeyError, ValueError):
        continue
smi = subprocess.run(["nvidia-smi", "--query-gpu=index,memory.used,memory.total", "--format=csv,noheader,nounits"],
                     capture_output=True, text=True).stdout
gpus = [dict(zip(("gpu", "used_mib", "total_mib"), map(int, l.split(",")))) for l in smi.splitlines() if l.strip()]
print(json.dumps({"executors": sorted(rows, key=lambda r: r["pid"]), "gpus": gpus}))
EOF
)
[ -f ~/.ssh/id_ed25519.pub ] || ssh-keygen -q -t ed25519 -N '' -f ~/.ssh/id_ed25519 || fail "ssh-keygen"
RENT_ARGS=--ssh; [ "$GPUS" = 1 ] || RENT_ARGS="--ssh --gpus $GPUS"
quote "$(param traffic_gb 75)" "$(param need_s 4500)"
rent
pod() { timeout "${T:-120}" cozy machine ssh "$NAME" "$@"; }
for i in $(seq 1 20); do pod true < /dev/null > /dev/null 2>&1 && break; [ "$i" = 20 ] && fail "no SSH into $NAME"; sleep 15; done
snapshot() { printf '%s' "$POD_PY" | pod "cat > /tmp/one.py && python3 /tmp/one.py" 2>/dev/null; }
[ -z "$W$TW" ] || step runtime machine update "$NAME" ${W:+--runtime "$W"} ${TW:+--tensorfs "$TW"} -y || fail "runtime update"
k=0
while read -r m; do
  k=$((k + 1)); [ -n "$m" ] || continue
  # shellcheck disable=SC2086 # a model and its flags
  step "model-$k" model download $m --on "$NAME" || fail "download $m"
done < <(jq -r '.[]' <<<"$MODELS")
# What idle-entrypoints.sh reads from gate.sh: cell, pkg, expect, $CLI, $M, $R, $REF, the scenario, $GPUS, $ONLY, $HERE.
RUN_CLI=cozy; . "$HERE/../e2e/run.sh"
CLI=cozy M=$NAME REF=$REF/seed.jpg
cell() {
  local name=$1 target=$2; shift 2
  [ -z "$ONLY" ] || [[ $ONLY == *",$name,"* ]] || return 0
  run_cell "$PWD" "$name" "$target" --on "$M" "$@"
  snapshot > "$name.gpu.json"
  say "$ARM $name: run $(cat "$name.number") $(run_get "$PWD" "$name" .status); $(jq -c '[.executors[] | select(.nvidia_fds > 0) | .pid], [.gpus[].used_mib]' "$name.gpu.json" 2>/dev/null | paste -sd' ')"
}
pkg() { echo "$1/$2"; }
expect() { printf '%s\t%s\t%s\n' "$1" "$2" "$3" >> expect.tsv; say "$ARM $1: $2 $3"; }
mapfile -t ROWS < <(grep -v '^#' "$HERE/scenarios.tsv")
BASE=$(( $(date +%s) / 60 ))
for i in "${!names[@]}"; do
  ARM=${names[$i]}
  if [ -n "${bins[$i]}" ]; then
    step "update-$ARM" machine update "$NAME" --tensord "${bins[$i]}" -y || fail "$ARM: update"
    v=$(cozy machine show "$NAME" --json | jq -r .status.version); want=${bins[$i]}
    [ -x "$want" ] && want=$("$want" --version | awk '{print $2}')
    [ "$v" = "$want" ] || fail "$ARM: the pod runs TensorD $v, not $want"
  fi
  say "$ARM: TensorD $(cozy machine show "$NAME" --json | jq -r .status.version)"
  ONLY=$(jq -r --argjson i "$i" '.[$i].only // empty' <<<"$ARMS"); ONLY=${ONLY:-$(param only)}; ONLY=${ONLY:+,$ONLY,}
  # Each arm its own scenario and seeds: no request repeats.
  IFS=$'\t' read -r _ CHAR MOTION OBJECT SCENE <<< "${ROWS[$(( (BASE + i) % ${#ROWS[@]} ))]}"
  R=$(( $(date +%s%N) / 1000 % 4000000000 ))
  mkdir -p "$OUT/$ARM"; cd "$OUT/$ARM" || fail "$ARM dir"
  . "$HERE/suites/tensord/idle-entrypoints.sh"
  cd "$OUT" || fail "out dir"
done
python3 -I - "$OUT" "${names[@]}" > "$OUT/summary.tsv" <<'EOF'
import json, sys
from pathlib import Path
out, arms = Path(sys.argv[1]), sys.argv[2:]
def load(arm, cell, ext):
    p = out / arm / f"{cell}{ext}"
    return json.loads(p.read_text()) if p.exists() and p.stat().st_size else None
print("arm\tcell\tstatus\texecution_s\tpids\texecutor_started_ms\tcontexts_after\tgpu_used_mib_after\tlegs_ms")
for arm in arms:
    for p in sorted((out / arm).glob("*.json")):
        cell = p.name[:-5]
        if cell.endswith(".gpu"):
            continue
        rec, snap = load(arm, cell, ".json"), load(arm, cell, ".gpu.json") or {}
        if not isinstance(rec, dict) or "status" not in rec:
            continue
        rows = [rec.get("measurements") or {}] + [c.get("measurements") or {} for c in rec.get("calls") or []]
        gpu = [m for m in rows if (m.get("execution") or {}).get("gpus")]
        e = gpu[-1]["execution"] if gpu else {}
        pids = sorted(g["pid"] for g in e.get("gpus") or [])
        ctx = [x["pid"] for x in snap.get("executors") or [] if x["nvidia_fds"] > 0]
        print("\t".join(map(str, (arm, cell, rec.get("status"), round((rec.get("execution_ms") or 0) / 1e3, 1), pids,
              (e.get("executor") or {}).get("started_unix_ms"), ctx, [g["used_mib"] for g in snap.get("gpus") or []],
              json.dumps((e.get("executor") or {}).get("legs_ms") or {})))))
EOF
cat "$OUT/summary.tsv" | tee -a "$R"
for a in "${names[@]}"; do sed "s/^/$a\t/" "$OUT/$a/expect.tsv" 2>/dev/null; done | tee "$OUT/verdicts.tsv" | tee -a "$R"
