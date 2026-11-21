# varena-matrix (Paul, 11-06): a varena, residency or loop change lands only if no row of this matrix is more than 5%
# worse in total than the release, and the candidate completes every call the release completes. rtx-4090s rented with
# SSH: one a cap, together (`parallel`), or one for all. The release is the Hub's current one (`machine update` to its
# target, the Hub's package releases); the
# candidate a TensorD and a Runtime. Rows: SDXL 1 MP, Qwen-Image 2.1 4 MP, Klein 1 MP (prompt upsampling on), H3 Turbo
# text-only at 768p and Flux-2 1 MP, each uncapped and at an equal-room 8 and 6 GiB (the candidate gets the driver's
# reservation on top when the release is varena v1, whose limit leaves it out). Per cap the release runs every row, then
# the candidate, each on a fresh `machine update`; per row a cold call (page cache dropped and checked, the previous row
# out of the warm set) and two warm calls; H3 is nonresident: a cold call and one more. Both arms send the same requests
# (each text unique in the session: a memo keyed on text serves nothing across rows or tiers, and none across arms, whose
# TensorD builds differ). A row's total is its warm calls' mean (H3: both calls'); cold calls are shown, not judged.
# params:
#   tensord      the candidate's TensorD: a cozy binary or a version (default: the session's cozy)
#   runtime      the candidate's Runtime: a wheel or a version (default: RUNTIME_WHEEL)
#   tensorfs     the candidate's TensorFS: a wheel or a version (default: what the candidate's TensorD installs)
#   packages     a packages_refs name: the candidate's packages from that checkout ("": the Hub's releases, as the release)
#   rows         subset of ["sdxl", "qwen", "klein", "h3turbo", "flux2"] (all); caps subset of ["off", "8GiB", "6GiB"] (all)
#   h3_seconds   H3 Turbo's clip (5: a third of the default 15 s, the same frames a second and steps)
#   flux2_steps  Flux-2's steps (20)
#   tolerance    a row's allowed excess over the release (0.05)
#   parallel     one pod a cap, rented and run together, the verdicts one table (true: about 45 min); false: one pod in turn
#   traffic_gb   GB the provider bills, all pods: the image and every row's weights on each (180 a pod); need_s (4200, one
#                pod: 7800)
. "$(dirname "$0")/lib.sh"
TD=$(param tensord "$(command -v cozy)") RT=$(param runtime "${RUNTIME_WHEEL:-}") TF=$(param tensorfs '')
PK=$(param packages '') ROWS=$(param rows '["sdxl", "qwen", "klein", "h3turbo", "flux2"]') CAPS=$(param caps '["off", "8GiB", "6GiB"]')
H3S=$(param h3_seconds 5) F2STEPS=$(param flux2_steps 20) TOL=$(param tolerance 0.05)
[ -n "$TD" ] && [ -n "$RT" ] || fail "params: the candidate's tensord and runtime (or a session dispatched from rc/<x>)"
[ -z "$PK" ] || [ -d "$PACKAGES/$PK" ] || fail "no packages checkout $PK"
RENT_ARGS=--ssh
[ -f ~/.ssh/id_ed25519.pub ] || ssh-keygen -q -t ed25519 -N '' -f ~/.ssh/id_ed25519 || fail "ssh-keygen"
command -v python3 > /dev/null || fail "no python3"
M=$OUT/matrix; mkdir -p "$M"
# The rows: package, function, lane (the main model's) and the request without its text.
cat > "$M/rows.json" <<E
{"sdxl": ["sdxl", "generate", "int8", {"aspect_ratio": "1:1", "megapixels": 1.0, "steps": 20, "guidance": 7.0}],
 "qwen": ["qwen-image-2", "generate", "int8", {"aspect_ratio": "1:1", "megapixels": 4}],
 "klein": ["flux-3-klein", "generate_turbo", "int8", {"aspect_ratio": "1:1", "megapixels": 1.0, "prompt_upsampling": true}],
 "h3turbo": ["minimax-h3", "generate", "int8", {"turbo": true, "aspect_ratio": "7:4", "resolution": "768p", "preset": "balanced",
             "music": "N/A", "soundscape": "Wind and ocean surf against the rocks.", "beats": [{"duration": $H3S}]}],
 "flux2": ["flux-2", "generate", "int8", {"aspect_ratio": "1:1", "megapixels": 1.0, "steps": $F2STEPS}]}
E
target() { local p; p=$(jq -r --arg r "$1" '.[$r][0] + "/" + .[$r][1]' "$M/rows.json"); [ "$2" = cand ] && [ -n "$PK" ] && echo "$PACKAGES/$PK/$p" || echo "$p"; }
lane() { jq -r --arg r "$1" '.[$r][2]' "$M/rows.json"; }
mapfile -t rows < <(jq -r '.[]' <<<"$ROWS"); mapfile -t caps < <(jq -r '.[]' <<<"$CAPS")
for r in "${rows[@]}"; do jq -e --arg r "$r" 'has($r)' "$M/rows.json" > /dev/null || fail "no row $r"; done
for c in "${caps[@]}"; do [[ $c =~ ^(off|[0-9]+GiB)$ ]] || fail "cap $c"; done
# Every request, written now: one text per row, cap and tier, the same for both arms; each checked against both arms'
# contracts (`cozy run <target> --describe`), so a field a release refuses fails here, at no cost.
python3 -I - "$M" "$ROWS" "$CAPS" <<'E' || fail "requests"
import itertools, json, sys, zlib
from pathlib import Path
m, rows, caps = Path(sys.argv[1]), json.loads(sys.argv[2]), json.loads(sys.argv[3])
spec = json.loads((m / "rows.json").read_text())
things = "lantern teapot sailboat bicycle fox clock kettle violin".split()
stuffs = "paper glass brass felt ceramic wooden copper marble".split()
places = ("on a windowsill at dawn", "on a workbench at noon", "on a stone ledge at dusk", "on a kitchen table under lamplight")
texts = (f"A {s} {t} {p}." for s, t, p in itertools.product(stuffs, things, places))
for r, c in itertools.product(rows, caps):
    for tier in ("cold", "w1") if r == "h3turbo" else ("cold", "w1", "w2"):
        req = json.loads(json.dumps(spec[r][3]))
        text, seed = next(texts), zlib.crc32(f"{r}/{c}/{tier}".encode()) & 0x7fffffff
        if r == "h3turbo":
            req["beats"][0]["description"] = f"[Shot 1] {text} Gulls ride the wind as the camera drifts past."
        else:
            req["prompt"] = text
        (m / f"{r}.{c}.{tier}.json").write_text(json.dumps(req | {"seed": seed}))
E
for arm in rel cand; do
  for r in "${rows[@]}"; do
    t=$(target "$r" "$arm")
    cozy --json run "$t" --describe > "$M/describe-$arm-$r.json" 2> "$M/describe-$arm-$r.err" || fail "$arm $t --describe: $(tail -c 300 "$M/describe-$arm-$r.err")"
    for f in "$M/$r".*.json; do
      bad=$(jq -r --slurpfile d "$M/describe-$arm-$r.json" '(keys - [$d[0].fields[].name]) | join(" ")' "$f")
      [ -z "$bad" ] || fail "$arm $t takes no $bad"
    done
  done
done
say "candidate: TensorD $TD, Runtime $RT${TF:+, TensorFS $TF}, packages ${PK:-the Hub releases}; rows ${rows[*]}; caps ${caps[*]}; $(ls "$M"/*.*.*.json | wc -l) requests"
PAR=$(param parallel true)
if [ "$PAR" = true ]; then
  pods=("${caps[@]/*/$SKU}")
  quote "$(param traffic_gb $((180 * ${#caps[@]})))" "$(param need_s 4200)" "${pods[@]}"
  for _ in "${caps[@]}"; do order; done  # booted together; the guard and the trap cover every one
  NAMES=(); for i in "${!caps[@]}"; do ready "$i"; NAMES+=("$NAME"); done
else
  quote "$(param traffic_gb 180)" "$(param need_s 7800)"
  rent; NAMES=("${caps[@]/*/$NAME}")
fi

# pod <cmd>: a shell command on the pod.
pod() { cozy machine ssh "$NAME" -- "$1" 2>> "$OUT/ssh.err"; }
reservation() {  # NVML's total minus CUDA's, the largest GPU's: what the driver keeps for itself
  pod "python3 -I -c 'import ctypes
class M(ctypes.Structure): _fields_ = [(\"t\", ctypes.c_ulonglong), (\"f\", ctypes.c_ulonglong), (\"u\", ctypes.c_ulonglong)]
nv, cu = ctypes.CDLL(\"libnvidia-ml.so.1\"), ctypes.CDLL(\"libcuda.so.1\"); nv.nvmlInit_v2(); cu.cuInit(0)
h, m, d, x, f, t = ctypes.c_void_p(), M(), ctypes.c_int(), ctypes.c_void_p(), ctypes.c_size_t(), ctypes.c_size_t()
nv.nvmlDeviceGetHandleByIndex_v2(0, ctypes.byref(h)); nv.nvmlDeviceGetMemoryInfo(h, ctypes.byref(m))
cu.cuDeviceGet(ctypes.byref(d), 0); cu.cuDevicePrimaryCtxRetain(ctypes.byref(x), d); cu.cuCtxSetCurrent(x); cu.cuMemGetInfo_v2(ctypes.byref(f), ctypes.byref(t))
print(m.t, t.value)'" | awk 'NF==2 && $2>0 && $1-$2 < 4*2^30 {print $1-$2}'
}
# evict: every regular file of 1 MiB or more out of the page cache, and the weights' residency checked (mincore) to be
# under 5% after: the same disk state before every cold call.
evict() {
  local r
  r=$(pod "python3 -I -c 'import ctypes, os, stat
libc = ctypes.CDLL(\"libc.so.6\"); libc.mmap.restype = ctypes.c_void_p
libc.mmap.argtypes = [ctypes.c_void_p, ctypes.c_size_t, ctypes.c_int, ctypes.c_int, ctypes.c_int, ctypes.c_long]
libc.mincore.argtypes = [ctypes.c_void_p, ctypes.c_size_t, ctypes.c_char_p]; libc.munmap.argtypes = [ctypes.c_void_p, ctypes.c_size_t]
def res(p, n):
    try: fd = os.open(p, os.O_RDONLY)
    except OSError: return 0
    a = libc.mmap(None, n, 1, 1, fd, 0); os.close(fd)
    if a in (None, ctypes.c_void_p(-1).value): return 0
    v = ctypes.create_string_buffer((n + 4095) // 4096); rc = libc.mincore(a, n, v); libc.munmap(a, n)
    return 0 if rc else sum(b & 1 for b in v.raw) * 4096
files = []
for d, ds, fs in os.walk(\"/\"):
    ds[:] = [x for x in ds if os.path.join(d, x) not in (\"/proc\", \"/sys\", \"/dev\", \"/run\")]
    for f in fs:
        p = os.path.join(d, f)
        try: s = os.lstat(p)
        except OSError: continue
        if stat.S_ISREG(s.st_mode) and s.st_size >= 1 << 20: files.append((p, s.st_size))
os.sync()
for p, _ in files:
    try: fd = os.open(p, os.O_RDONLY); os.posix_fadvise(fd, 0, 0, os.POSIX_FADV_DONTNEED); os.close(fd)
    except OSError: pass
w = [(p, n) for p, n in files if \"tensorfs\" in p]
print(sum(n for _, n in w), sum(res(p, n) for p, n in w))'" | tail -1)
  read -r total left <<<"$r"
  [ "${total:-0}" -gt 0 ] && [ "${left:-1}" -le $((total / 20)) ] || fail "the page cache kept the weights: ${r:-no reading}"
}
# swap <arm> <cap>: the arm's stack and the cap, one update and restart, both checked from the pod's own report; then the
# boot's kernel check waited for (a first call before it would run on other kernels).
swap() {
  local arm=$1 cap=$2 limit=off args=()
  [ "$cap" = off ] || limit=$(( ${cap%GiB} * 2**30 + $([ "$arm" = cand ] && echo "$CAND_EXTRA" || echo 0) ))
  [ "$arm" = rel ] || { args=(--tensord "$TD" --runtime "$RT"); [ -z "$TF" ] || args+=(--tensorfs "$TF"); }
  local t s k0; k0=$(cozy machine logs "$NAME" -n 100000 2>/dev/null | grep -c '"kernels":\[{')
  step "swap-$arm-$cap" machine update "$NAME" "${args[@]}" --gpu-memory-limit "$limit" -y || fail "update $arm $cap"
  t=$(date +%s)
  until s=$(cozy machine show "$NAME" --json 2>/dev/null) && jq -e --arg l "$limit" \
      '.status.gpus | all(.[]; (.memory_limit_bytes // 0) == (if $l == "off" then 0 else ($l | tonumber) end))' <<<"$s" > /dev/null; do
    [ $(($(date +%s) - t)) -lt 120 ] || fail "$NAME does not report limit $limit"; sleep 5
  done
  jq -c '.status | {version, tensorfs, runtime, commit, limit: [.gpus[].memory_limit_bytes]}' <<<"$s" > "$M/stack-$arm-$cap.json"
  for _ in $(seq 24); do [ "$(cozy machine logs "$NAME" -n 100000 2>/dev/null | grep -c '"kernels":\[{')" -gt "$k0" ] && break; sleep 5; done
  say "$arm at $cap: $(cat "$M/stack-$arm-$cap.json")"
}
# call <arm> <row> <cap> <tier>: one call, its record in $M/<arm>.<row>.<cap>.<tier>.rec.json (a failed run's too).
call() {
  local n=$1.$2.$3.$4
  step "$n" run "$(target "$2" "$1")" --on "$NAME" --lane "$(lane "$2")" --input "$M/$2.$3.$4.json"
  local no; no=$(runno "$n")
  [ -n "$no" ] || { echo '{"status": "no run"}' > "$M/$n.rec.json"; say "$n: no run: $(tail -c 300 "$OUT/$n.txt")"; return; }
  cozy run show "$no" --once --json --full > "$M/$n.rec.json" 2> /dev/null  # a failed run's record comes with exit 1
  say "$n: run $no $(jq -r '"\(.status) \((.execution_ms // 0) / 1000) s"' "$M/$n.rec.json" 2>/dev/null)"
  # A package locked to another Runtime minor runs on its own pair (cl-795): a call on any other stack measures the wrong arm.
  local ran want; ran=$(jq -r '[.measurements.software // {} | .runtime, .tensord] | join(" ")' "$M/$n.rec.json" 2>/dev/null)
  want=$(jq -r '[.runtime, .version] | join(" ")' "$M/stack-$1-$3.json")
  local rr=${ran%% *} wr=${want%% *}  # a release's record adds its commit (0.42.0+386146d6); a dev build's version is its own
  [ "$ran" = " " ] || [ "$rr" = "$wr" ] || { [[ $wr != *+* && $rr == "$wr"+* && $rr != *+dev* ]]; } || fail "$n ran on Runtime $rr, not the $1 arm's $wr"
}

# matrix_cap <cap>: one cap on $NAME, the release's rows then the candidate's. Equal room is the pod's own: a varena v1
# release (TensorD below 0.21) leaves the driver's reservation out of its limit, varena v2 counts it.
matrix_cap() {
  local cap=$1 r arm tier rv reserve
  swap rel "$cap"
  reserve=$(reservation); [ -n "$reserve" ] || fail "$NAME: no reservation reading"
  rv=$(jq -r .version "$M/stack-rel-$cap.json")
  CAND_EXTRA=$(python3 -c "import sys; v = tuple(int(x) for x in sys.argv[1].split('+')[0].split('.')[:2]); print(int(sys.argv[2]) if v < (0, 21) else 0)" "$rv" "$reserve")
  say "$NAME at $cap: release TensorD $rv; driver reservation $((reserve >> 20)) MiB; the candidate's limit $((CAND_EXTRA >> 20)) MiB over the release's"
  [ -f "$M/weights-$NAME" ] || for r in "${rows[@]}"; do  # every row's weights before anything is timed: no timed call fetches
    step "weights-$cap-$r" run "$(target "$r" rel)" --on "$NAME" --lane "$(lane "$r")" --warm downloaded || fail "$NAME: weights $r"
    cozy run "$(target "$r" rel)" --on "$NAME" --warm off -p "$RUN_PROJECT" > /dev/null 2>&1
  done
  touch "$M/weights-$NAME"
  for arm in rel cand; do
    [ "$arm" = rel ] || swap "$arm" "$cap"
    for r in "${rows[@]}"; do
      step "ready-$arm-$r-$cap" run "$(target "$r" "$arm")" --on "$NAME" --lane "$(lane "$r")" --warm downloaded || fail "$NAME: install $arm $r"
      evict
      for tier in cold w1 w2; do [ "$r.$tier" != h3turbo.w2 ] && call "$arm" "$r" "$cap" "$tier"; done
      cozy run "$(target "$r" "$arm")" --on "$NAME" --warm off -p "$RUN_PROJECT" > /dev/null 2>&1  # the next row starts with the GPU free
    done
  done
}
CUT=()  # caps whose pod stopped short: their rows read as missing in the verdict
if [ "$PAR" = true ]; then
  JOBS=()
  for i in "${!caps[@]}"; do ( NAME=${NAMES[$i]}; matrix_cap "${caps[$i]}" ) & JOBS+=($!); done  # a subshell's fail ends it alone
  for i in "${!caps[@]}"; do wait "${JOBS[$i]}" || CUT+=("${caps[$i]}"); done
else
  for cap in "${caps[@]}"; do ( matrix_cap "$cap" ) || CUT+=("$cap"); done
fi

python3 -I - "$M" "$TOL" "$ROWS" "$CAPS" > "$OUT/verdict.md" <<'E'
import json, statistics, sys
from pathlib import Path
m, tol, rows, caps = Path(sys.argv[1]), float(sys.argv[2]), json.loads(sys.argv[3]), json.loads(sys.argv[4])
def rec(arm, r, c, t):
    f = m / f"{arm}.{r}.{c}.{t}.rec.json"
    try:
        d = json.loads(f.read_text())
    except (OSError, ValueError):
        return {"status": "no record"}
    steps = {}
    for x in [d.get("measurements") or {}] + [k.get("measurements") or {} for k in d.get("calls") or []]:
        steps |= (x.get("attribution") or {}).get("steps") or {}
    main = max(steps.values(), key=lambda s: s.get("count", 0)) if steps else {}
    series = (main.get("device") or {}).get("series") or [s[1] for s in main.get("series") or []]
    return {"status": d.get("status"), "run": d.get("number"), "total": (d.get("execution_ms") or 0) / 1000,
            "steady": statistics.median(series[1:]) / 1000 if len(series) > 1 else None}
fmt = lambda v: "-" if v is None else f"{v:.2f}"
lines = ["| row | cap | release s | candidate s | Δ | cold, release → candidate s | steady s | runs (release; candidate) | verdict |",
         "|---|---|---|---|---|---|---|---|---|"]
fails = []
for r in rows:
    tiers = ["cold", "w1"] if r == "h3turbo" else ["cold", "w1", "w2"]
    judged = tiers if r == "h3turbo" else tiers[1:]
    for c in caps:
        a, b = ({t: rec(arm, r, c, t) for t in tiers} for arm in ("rel", "cand"))
        ok = lambda x: all(x[t]["status"] == "completed" for t in judged)
        mean = lambda x: statistics.mean(x[t]["total"] for t in judged) if ok(x) else None
        ma, mb = mean(a), mean(b)
        if ok(a) and not ok(b):
            verdict = "FAIL: the candidate fails where the release completes"
        elif not ok(a) and not ok(b):
            verdict = "both fail"
        elif not ok(a):
            verdict = "the candidate completes where the release fails"
        elif mb > ma * (1 + tol):
            verdict = f"FAIL: {mb / ma - 1:+.1%}, over {tol:.0%}"
        else:
            verdict = "pass"
        if verdict.startswith("FAIL"):
            fails.append(f"{r} at {c}")
        d = f"{mb / ma - 1:+.1%}" if ma and mb else "-"
        st = lambda x: fmt(statistics.median([x[t]["steady"] for t in judged if x[t].get("steady")]) if any(x[t].get("steady") for t in judged) else None)
        runs = lambda x: ",".join(str(x[t].get("run") or x[t]["status"]) for t in tiers)
        lines.append(f"| {r} | {c} | {fmt(ma)} | {fmt(mb)} | {d} | {fmt(a['cold'].get('total'))} → {fmt(b['cold'].get('total'))} | "
                     f"{st(a)} → {st(b)} | {runs(a)}; {runs(b)} | {verdict} |")
print("\n".join(lines))
print(f"\nVERDICT {'FAIL: ' + ', '.join(fails) if fails else 'PASS'} (a row: its warm calls' mean, H3 both calls'; over {tol:.0%} worse fails)")
E
cat "$OUT/verdict.md" >> "$R"
[ "${#CUT[@]}" = 0 ] || fail "cut short at ${CUT[*]}: $(grep '^VERDICT' "$OUT/verdict.md")"
grep -q '^VERDICT PASS' "$OUT/verdict.md" || fail "$(grep '^VERDICT' "$OUT/verdict.md")"
say "PASS: no row over $(awk -v t="$TOL" 'BEGIN { printf "%d%%", t * 100 }') worse than the release, and nothing the release completes fails"
