# cold-start: a package's first request right after its install, on one rental, once per TensorD arm. Each arm puts
# its TensorD on the pod (the pod's own image for `image`), installs each of its models' packages afresh (remove, then
# install), drops the weights from the page cache first, runs one cold request each and counts the environment's
# bytecode after it; the executor processes' memory is read after the machine starts and after the requests, and the
# machine's kernel boots at the end. An arm's `wait` says when
# its requests go after their install: `now`, once the package's level is `imported` (its import-only parent is up),
# or once that parent's kernel compiles have ended too (`compiled`); the session says how long each took. An arm's
# `warm` requests follow each cold one on the same executor, each a new prompt and seed (0 by default). Nothing
# here compares: the run records and the stamped event streams are the evidence. params:
#   arms        [{"name": "fix", "tensord": "<dev-build run id>", "models": ["sdxl", "anima"], "wait": "now", "warm": 1},
#               {"name": "again", "tensord": "same"}, {"name": "prod", "tensord": "0.13.7"}, …], in order. tensord:
#               `image` or `same` (no update), a release version, or a dev-build run id; models: sdxl, anima, qwen;
#               `runtime` and `tensorfs` (optional): a wheel URL, or a cozy-runtime dev-build run id for `runtime`
#   sdxl        the SDXL function (sdxl/generate@0.1.3)
#   anima       the Anima function (anima/generate@0.1.1)
#   qwen        the Qwen-Image function (qwen-image-2/generate@0.1.8), at 1 MP, 20 steps
#   traffic_gb  GB the provider bills: the image and both models' weights (40)
#   need_s      seconds the guard must buy (5400)
#   sample      true: the pod's disk, process I/O, pressure and GPU sampled for the session (samples.jsonl, dmon.txt)
. "$(dirname "$0")/lib.sh"
ARMS=$(param arms '[{"name": "prod", "tensord": "image"}]') SDXL=$(param sdxl sdxl/generate@0.1.3) ANIMA=$(param anima anima/generate@0.1.1)
QWEN=$(param qwen qwen-image-2/generate@0.1.8)
mapfile -t names < <(jq -r '.[].name' <<<"$ARMS")
mapfile -t builds < <(jq -r '.[].tensord' <<<"$ARMS")
[ "${#names[@]}" -gt 0 ] || fail "no arms"
# Each arm's cozy, fetched before renting, so a missing one costs nothing.
bins=()
for i in "${!names[@]}"; do
  [[ ${names[$i]} =~ ^[a-z0-9-]+$ ]] || fail "arm name ${names[$i]}"
  if [ "${builds[$i]}" = image ] || [ "${builds[$i]}" = same ]; then bins+=(""); continue; fi
  if [[ ${builds[$i]} =~ ^[0-9]+\.[0-9]+\.[0-9]+ ]]; then bins+=("${builds[$i]}"); continue; fi
  d=$OUT/arms/${names[$i]}; mkdir -p "$d"
  gh run download "${builds[$i]}" -R cozy-creator/cozy-creator -p 'cozy-*' -D "$d" >/dev/null 2>&1 || fail "no cozy artifact in run ${builds[$i]}"
  b=$(ls "$d"/cozy-*/cozy-linux-x86_64 2>/dev/null | head -1); [ -n "$b" ] || fail "run ${builds[$i]} has no cozy-linux-x86_64"
  chmod +x "$b"; say "arm ${names[$i]}: $("$b" --version)"; bins+=("$b")
done
# Each arm's Runtime and TensorFS wheels (`--runtime`, `--tensorfs` of its update), fetched before renting too.
wheel() {
  local d=$OUT/arms/$1-$2; mkdir -p "$d"
  case $3 in
    https://*) curl -fsSL --retry 5 -o "$d/${3##*/}" "$3" || return 1 ;;
    *) gh run download "$3" -R cozy-creator/cozy-runtime -p 'cozy-runtime-*' -D "$d" >/dev/null 2>&1 || return 1 ;;
  esac
  find "$d" -name '*manylinux*x86_64*.whl' | head -1
}
wheels=()
for i in "${!names[@]}"; do
  w=()
  for kind in runtime tensorfs; do
    src=$(jq -r --argjson i "$i" --arg k "$kind" '.[$i][$k] // empty' <<<"$ARMS"); [ -n "$src" ] || continue
    f=$(wheel "${names[$i]}" "$kind" "$src") && [ -n "$f" ] || fail "arm ${names[$i]}: no $kind wheel from $src"
    say "arm ${names[$i]}: $kind ${f##*/}"; w+=("--$kind" "$f")
  done
  wheels+=("${w[*]}")
done
POD_PY=$(cat <<'EOF'
import glob, json, os, sys, time
def roots(name):
    found = set()
    for env in glob.glob("/proc/[0-9]*/environ"):
        try:
            for kv in open(env, "rb").read().split(b"\0"):
                if kv.startswith(name.encode() + b"="):
                    found.add(kv.split(b"=", 1)[1].decode())
        except OSError:
            pass
    return sorted(r for r in found if os.path.isdir(r))
def generations():
    for g in glob.glob("/var/lib/cozy/tensord/generations/*/generation.json"):
        try:
            yield os.path.getmtime(g), g, json.load(open(g))
        except (OSError, ValueError):
            pass
def resident(path):
    import ctypes
    libc = ctypes.CDLL("libc.so.6", use_errno=True)
    libc.mmap.restype = ctypes.c_void_p
    libc.mmap.argtypes = [ctypes.c_void_p, ctypes.c_size_t, ctypes.c_int, ctypes.c_int, ctypes.c_int, ctypes.c_long]
    size = os.path.getsize(path)
    fd = os.open(path, os.O_RDONLY)
    try:
        addr = libc.mmap(None, size, 1, 1, fd, 0)
        if addr in (None, ctypes.c_void_p(-1).value):
            return 0
        vec = ctypes.create_string_buffer((size + 4095) // 4096)
        rc = libc.mincore(ctypes.c_void_p(addr), ctypes.c_size_t(size), vec)
        libc.munmap(ctypes.c_void_p(addr), ctypes.c_size_t(size))
        return 0 if rc else sum(b & 1 for b in vec.raw) * 4096
    finally:
        os.close(fd)
mode = sys.argv[1]
if mode == "pyc":
    module = sys.argv[2]
    rows = sorted((t, g, d) for t, g, d in generations() if d.get("application", "").split(":")[0] == module)
    if not rows:
        print(json.dumps({"module": module, "error": "no generation"})); sys.exit()
    t, g, d = rows[-1]
    env = os.path.realpath(os.path.join(os.path.dirname(g), "env"))
    site = glob.glob(os.path.join(env, "lib/python3*/site-packages"))[0]
    out = {"module": module, "generation": os.path.basename(os.path.dirname(g)), "packages": {}}
    for name in ("torch", "diffusers", "transformers", "sympy", "peft", "accelerate", "huggingface_hub", "safetensors", "cozy_runtime"):
        p = os.path.join(site, name)
        if os.path.isdir(p):
            py = pyc = 0
            for _, _, files in os.walk(p):
                py += sum(f.endswith(".py") for f in files); pyc += sum(f.endswith(".pyc") for f in files)
            out["packages"][name] = [py, pyc]
    print(json.dumps(out))
elif mode == "evict":
    # Executors name the store (TENSORFS_HOME); before the first install only the machine runs, which the image
    # names it to (COZY_TENSORFS_ROOT). The first arm otherwise found no weights and ran on a warm page cache.
    tops = set(roots("TENSORFS_HOME")) | set(roots("COZY_TENSORFS_ROOT"))
    files = sorted({os.path.join(dp, f) for r in tops for dp, _, fs in os.walk(r) for f in fs})
    files = [f for f in files if os.path.isfile(f) and not os.path.islink(f) and os.path.getsize(f) > (1 << 20)]
    before = sum(resident(f) for f in files)
    os.sync()
    for f in files:
        fd = os.open(f, os.O_RDONLY)
        try:
            os.posix_fadvise(fd, 0, 0, os.POSIX_FADV_DONTNEED)
        finally:
            os.close(fd)
    print(json.dumps({"files": len(files), "gb": round(sum(os.path.getsize(f) for f in files) / 1e9, 2),
                      "resident_gb_before": round(before / 1e9, 2), "resident_gb_after": round(sum(resident(f) for f in files) / 1e9, 2)}))
elif mode == "sample":
    # Every 0.5 s until /tmp/sample.stop: each block device's reads, each cozy process's I/O (read_bytes is
    # what came from storage, rchar every read), and the pressure stalls: where a slow encode's time went.
    def cozy_processes():
        for d in glob.glob("/proc/[0-9]*"):
            try:
                cmd = open(d + "/cmdline", "rb").read().replace(b"\0", b" ").decode(errors="replace")
            except OSError:
                continue
            kind = ("executor" if "cozy_runtime.internal.executor" in cmd else "compile" if "compileall" in cmd
                    else "kernels" if "cozy_runtime.author.kernels" in cmd or "machine_kernels" in cmd
                    else "cozy" if cmd.split(" ")[0].endswith("/cozy") else "")
            if kind:
                yield int(d[6:]), kind, d
    out = open(sys.argv[2], "a")
    while not os.path.exists("/tmp/sample.stop"):
        row = {"t": round(time.time(), 3), "disk": {}, "procs": [], "psi": {}}
        for line in open("/proc/diskstats"):
            f = line.split()
            if not f[2].startswith(("loop", "ram")) and int(f[3]) + int(f[7]):
                row["disk"][f[2]] = [int(f[5]) * 512, int(f[9]) * 512, int(f[11])]  # bytes read, written, ios in flight
        for pid, kind, d in cozy_processes():
            try:
                io = dict(l.split(": ") for l in open(d + "/io").read().splitlines())
                state = open(d + "/stat").read().rsplit(")", 1)[1].split()[0]
                row["procs"].append([pid, kind, int(io["read_bytes"]), int(io["rchar"]), int(io["write_bytes"]), state])
            except (OSError, KeyError, ValueError):
                pass
        for res in ("io", "memory", "cpu"):
            try:
                row["psi"][res] = [l.split()[0] + " " + l.split()[4] for l in open(f"/proc/pressure/{res}")]
            except OSError:
                pass
        out.write(json.dumps(row) + "\n"); out.flush()
        time.sleep(0.5)
elif mode == "ps":
    rows = []
    for d in glob.glob("/proc/[0-9]*"):
        try:
            cmd = open(d + "/cmdline", "rb").read().replace(b"\0", b" ").decode(errors="replace")
            kind = ("codec" if "frame-codec" in cmd else "compile" if "cozy_runtime.author.kernels" in cmd
                    else "executor" if "cozy_runtime.internal.executor" in cmd else "")
            if not kind:
                continue
            status = dict(l.split(":", 1) for l in open(d + "/status").read().splitlines() if ":" in l)
            pss = next((int(l.split()[1]) for l in open(d + "/smaps_rollup") if l.startswith("Pss:")), 0)
            fds = [os.readlink(f) for f in glob.glob(d + "/fd/*") if os.path.exists(f)]
            started = os.stat(d).st_mtime
            rows.append({"pid": int(d[6:]), "ppid": int(status["PPid"]), "rss_mib": int(status["VmRSS"].split()[0]) // 1024,
                         "pss_mib": pss // 1024, "nvidia_fds": sum("/dev/nvidia" in f for f in fds),
                         "age_s": round(time.time() - started), "kind": kind})
        except (OSError, KeyError, ValueError, StopIteration):
            continue
    for r in rows:
        r["children"] = sum(o["ppid"] == r["pid"] for o in rows)
    print(json.dumps(sorted(rows, key=lambda r: r["pid"])))
EOF
)
[ -f ~/.ssh/id_ed25519.pub ] || ssh-keygen -q -t ed25519 -N '' -f ~/.ssh/id_ed25519 || fail "ssh-keygen"
RENT_ARGS=--ssh
quote "$(param traffic_gb 40)" "$(param need_s 5400)"
rent
pod() { timeout "${T:-120}" cozy machine ssh "$NAME" "$@"; }
for i in $(seq 1 20); do pod true < /dev/null > /dev/null 2>&1 && break; [ "$i" = 20 ] && fail "no SSH into $NAME"; sleep 15; done
podpy() { printf '%s' "$POD_PY" | T=600 pod "cat > /tmp/cold.py && python3 /tmp/cold.py $*"; }
# `sample: true`: the pod's disk, process I/O, pressure and GPU (nvidia-smi dmon) every 0.5-1 s for the whole
# session, into samples.jsonl and dmon.txt.
if [ "$(param sample false)" = true ]; then
  printf '%s' "$POD_PY" | T=60 pod "cat > /tmp/cold.py && rm -f /tmp/sample.stop /tmp/samples.jsonl &&
    (setsid nohup python3 /tmp/cold.py sample /tmp/samples.jsonl > /dev/null 2>&1 &) &&
    (setsid nohup nvidia-smi dmon -s ut -d 1 -o T > /tmp/dmon.txt 2>&1 & echo \$! > /tmp/dmon.pid)" < /dev/null || fail "sampler"
  say "sampling the pod's disk, process I/O, pressure and GPU"
fi
# One request, its events stamped with the runner's clock from just before the CLI starts.
request() {
  local label=$1 fn=$2 input=$3 t0
  t0=$(date +%s.%N)
  cozy run "$fn" --on "$NAME" --input "$input" --json --out "$OUT/$label" 2> "$OUT/$label.err" |
    python3 -u -c 'import sys,time; t0=float(sys.argv[1]); [print(f"{time.time()-t0:.3f} {l}", end="", flush=True) for l in sys.stdin]' "$t0" > "$OUT/$label.ndjson"
  local n; n=$(grep -o '"number":[0-9]*' "$OUT/$label.ndjson" | head -1 | cut -d: -f2)
  [ -n "$n" ] || { say "$label: no run"; return 1; }
  cozy run show "$n" --once --json > "$OUT/$label.json" 2>&1
  say "$label: run $n $(jq -r .status "$OUT/$label.json"), landed $(grep output_item.landed "$OUT/$label.ndjson" | tail -1 | cut -d' ' -f1) s; legs $(jq -c '.measurements.execution.executor.legs_ms' "$OUT/$label.json")"
}
declare -A FN=([sdxl]=$SDXL [anima]=$ANIMA [qwen]=$QWEN) PKG=([sdxl]=sdxl [anima]=anima [qwen]=qwen-image-2)
declare -A WEIGHTS=([sdxl]=stabilityai/sdxl@1.0.0 [anima]=circlestone-labs/anima@1.0.0 [qwen]=qwen/qwen-image-2.1@0.1.0)
for m in $(jq -r '[.[].models // ["sdxl", "anima"] | .[]] | unique | .[]' <<<"$ARMS"); do
  [ -n "${FN[$m]:-}" ] || fail "no model $m"
  step "dl-$m" model download "${WEIGHTS[$m]}" --on "$NAME" || fail "$m weights"
done
PROMPTS=("a lighthouse keeper's desk at dawn, watercolor" "a night market under paper lanterns, film photo" "a fox asleep in a library window, gouache"
  "a desert observatory under the milky way, matte painting" "a tram crossing a snowy bridge, ukiyo-e" "a greenhouse of carnivorous plants, macro photo"
  "a clocktower interior with brass gears, ink and wash" "a fishing village at low tide, oil painting"
  "a glacier cave lit by a single torch, concept art" "a rooftop garden in the rain, pastel" "an abandoned arcade at noon, flash photo"
  "a beekeeper among lavender rows, impressionist" "a submarine galley at midnight, cel shading" "a salt flat mirror at sunset, long exposure"
  "a puppet workshop full of marionettes, charcoal" "a canyon railway viaduct in fog, linocut")
k=0
for i in "${!names[@]}"; do
  a=${names[$i]}
  if [ -n "${bins[$i]}" ]; then
    step "update-$a" machine update "$NAME" --tensord "${bins[$i]}" -y || fail "$a: update"
    v=$(cozy machine show "$NAME" --json | jq -r .status.version); say "$a: pod TensorD $v"
    want=${bins[$i]}; [ -x "$want" ] && want=$("$want" --version | awk '{print $2}')
    [ "$v" = "$want" ] || fail "$a: the pod runs TensorD $v, not $want"
  fi
  if [ -n "${wheels[$i]}" ]; then
    # shellcheck disable=SC2086 # the arm's flags and wheel paths (no spaces)
    step "pair-$a" machine update "$NAME" ${wheels[$i]} -y || fail "$a: Runtime/TensorFS update"
    say "$a: pod Runtime $(cozy machine show "$NAME" --json | jq -r .status.runtime), TensorFS $(cozy machine show "$NAME" --json | jq -r .status.tensorfs)"
  fi
  if [ -n "${bins[$i]}${wheels[$i]}" ]; then
    sleep 30; podpy ps > "$OUT/$a-ps-start.json"; say "$a: executors 30 s after start $(cat "$OUT/$a-ps-start.json")"
  fi
  wait=$(jq -r --argjson i "$i" '.[$i].wait // "now"' <<<"$ARMS") warm=$(jq -r --argjson i "$i" '.[$i].warm // 0' <<<"$ARMS")
  for m in $(jq -r --argjson i "$i" '.[$i].models // ["sdxl", "anima"] | .[]' <<<"$ARMS"); do
    fn=${FN[$m]} pkg=${PKG[$m]} module=${PKG[$m]//-/_}
    # The weights leave the page cache first, so nothing this session does runs beside the imports
    # an install starts.
    podpy evict > "$OUT/$a-$m-evict.json"; say "$a $m evicted: $(cat "$OUT/$a-$m-evict.json")"
    [ "$(jq -r .files "$OUT/$a-$m-evict.json" 2>/dev/null)" != 0 ] || fail "$a $m: found no weights to evict"
    step "rm-$a-$m" package remove "$pkg" --on "$NAME" -y
    # The release the request names, never the newest: another would install on the request's own path.
    step "in-$a-$m" package install "$pkg@${fn##*@}" --on "$NAME" || fail "$a: install $m"
    installed=$(date +%s)
    if [ "$wait" != now ]; then
      level=''
      for t in $(seq 1 120); do
        level=$(cozy package ls --on "$NAME" --json 2>/dev/null | jq -r --arg p "$pkg" '[.packages[] | select(.package == $p or .package == "_/" + $p) | .level] | first // empty')
        [ "$level" = imported ] && break
        sleep 2
      done
      say "$a $m: level $level $(( $(date +%s) - installed )) s after its install"
      if [ "$wait" = compiled ]; then
        for t in $(seq 1 120); do
          podpy ps > "$OUT/$a-$m-ps-compiling.json"
          [ "$(jq '[.[] | select(.kind == "compile")] | length' "$OUT/$a-$m-ps-compiling.json")" = 0 ] && break
          sleep 5
        done
        say "$a $m: its kernel compiles ended $(( $(date +%s) - installed )) s after its install"
      fi
      podpy ps > "$OUT/$a-$m-ps-ready.json"; say "$a $m: executors before its request $(cat "$OUT/$a-$m-ps-ready.json")"
    fi
    for w in $(seq 0 "$warm"); do
      k=$(( (k + 1) % ${#PROMPTS[@]} ))
      if [ "$m" = sdxl ]; then
        jq -n --arg p "${PROMPTS[$k]}" --argjson s "$RANDOM$i$w" '{megapixels: 1.0, aspect_ratio: "1:1", steps: 20, guidance: 7.0, hidiffusion: false, prompt_upsampling: false, prompt: $p, seed: $s}' > "$OUT/$a-$m-$w.yaml"
      elif [ "$m" = qwen ]; then
        jq -n --arg p "${PROMPTS[$k]}" --argjson s "$RANDOM$i$w" '{megapixels: 1.0, aspect_ratio: "1:1", steps: 20, prompt_upsampling: false, prompt: $p, seed: $s}' > "$OUT/$a-$m-$w.yaml"
      else
        jq -n --arg p "${PROMPTS[$k]}" --argjson s "$RANDOM$i$w" '{megapixels: 2.25, aspect_ratio: "1:1", steps: 30, guidance: 4.5, cfg_interval_start: 0.0, cfg_interval_stop: 1.0, prompt_upsampling: false, prompt: $p, seed: $s, negative_prompt: "worst quality, low quality, blurry, jpeg artifacts"}' > "$OUT/$a-$m-$w.yaml"
      fi
      label="$a-$m-cold"; [ "$w" = 0 ] || label="$a-$m-warm$w"
      request "$label" "$fn" "$OUT/$a-$m-$w.yaml" || fail "$a: $m request"
    done
    podpy pyc "$module" > "$OUT/$a-$m-pyc.json"; say "$a $m bytecode after its request (py, pyc): $(jq -c .packages "$OUT/$a-$m-pyc.json")"
  done
  podpy ps > "$OUT/$a-ps-after.json"; say "$a: executors after the requests $(cat "$OUT/$a-ps-after.json")"
done
T=60 pod "cat /var/lib/cozy/tensord/gpu/kernel-boot.jsonl" > "$OUT/kernel-boot.jsonl" 2>&1 < /dev/null
if [ "$(param sample false)" = true ]; then
  T=120 pod "touch /tmp/sample.stop; sleep 1; cat /tmp/samples.jsonl" > "$OUT/samples.jsonl" 2> /dev/null < /dev/null
  T=60 pod "kill \$(cat /tmp/dmon.pid); cat /tmp/dmon.txt" > "$OUT/dmon.txt" 2> /dev/null < /dev/null
  for log in tensord memory; do cozy machine logs "$NAME" "$log" -n 0 > "$OUT/machine-$log.log" 2>&1 < /dev/null; done
  say "samples: $(wc -l < "$OUT/samples.jsonl") rows"
fi
say "kernel boots: $(wc -l < "$OUT/kernel-boot.jsonl") lines"
say "PASS: ${#names[@]} arms"
