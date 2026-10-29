# warming (xs-132 items 3-5), on one rental of the pod's own image (the current release). Nothing here compares: the
# run records, the pod's process and page-cache readings and the stamped event streams are the evidence.
#   3  page cache: an environment's import with its files evicted from the page cache vs cached. Through
#      `cozy run <sdxl> --warm imported` after a TensorD restart (no process left), and as the import-only parent's
#      imports run directly in each environment's Python (SDXL, H3: H3's `imported` downloads its weights first).
#      Each trial: seconds per stage, bytes read from storage, major faults; the pages an import touched.
#   4  warm on intent: a request queued behind another package's run; when its import-only parent starts.
#   5  `--warm gpu`: it loads, stays loaded, is listed at its level, the next request starts warm vs a cold one, and
#      `--warm off` takes it out of the set.
# params:
#   sdxl        the SDXL function (sdxl/generate@0.1.4)
#   h3          the H3 package release (minimax-h3@0.1.8)
#   queued      item 4's second function, another package (birefnet/remove_background@0.1.0)
#   trials      trials per condition (3)
#   hold_s      how long item 5 keeps the member loaded before its next request (180)
#   items       the items, in order ("3 4 5"); "3b" needs tensord
#   4b          needs tensord (the item-4 build): SDXL runs while birefnet and the xs132-pipeline fixture (a CPU
#               orchestrator calling birefnet) wait, once in shared mode (config.toml) and once dedicated; processes are
#               sampled each second: when each environment's import-only parent starts
#   tensord     a dev-build run id: after the items before "3b", the pod runs its TensorD, and 3b measures its
#               machine-start parent import with and without the environment's recorded import pages (its legs, from
#               the next request's record), each after the environment's files are evicted
#   traffic_gb  GB the provider bills (20); need_s, seconds the guard must buy (3600)
. "$(dirname "$0")/lib.sh"
SDXL=$(param sdxl sdxl/generate@0.1.4) H3=$(param h3 minimax-h3@0.1.8) QUEUED=$(param queued birefnet/remove_background@0.1.0)
TRIALS=$(param trials 3) HOLD=$(param hold_s 180) ITEMS=$(param items "3 4 5") BUILD=$(param tensord)
DEV=''
if [ -n "$BUILD" ]; then
  gh run download "$BUILD" -R cozy-creator/cozy-creator -p 'cozy-*' -D "$OUT/build" >/dev/null 2>&1 || fail "no cozy artifact in run $BUILD"
  DEV=$(ls "$OUT"/build/cozy-*/cozy-linux-x86_64 2>/dev/null | head -1); [ -n "$DEV" ] || fail "run $BUILD has no cozy-linux-x86_64"
  chmod +x "$DEV"; say "build: $("$DEV" --version)"
fi
[[ " $ITEMS " != *" 3b "* && " $ITEMS " != *" 4b "* || -n "$DEV" ]] || fail "items 3b and 4b need tensord"
PIPE=$(cd "$(dirname "$0")/fixtures/xs132-pipeline" && pwd)
[ -f "$PIPE/uv.lock" ] || fail "no $PIPE/uv.lock"
[[ $TRIALS =~ ^[1-9]$ && $HOLD =~ ^[0-9]+$ ]] || fail "trials $TRIALS, hold_s $HOLD"
[[ $SDXL == */*@* && $QUEUED == */*@* && $H3 == *@* ]] || fail "functions and releases are pinned: $SDXL $QUEUED $H3"
[ -f "$REF/seed.jpg" ] || fail "no $REF/seed.jpg"
POD_PY=$(cat <<'EOF'
import ctypes, glob, json, os, random, subprocess, sys, time
GENERATIONS = "/var/lib/cozy/tensord/generations"
# The import-only parent's imports (cozy_runtime.internal.executor _CONSTRUCTION_MODULES), after torch.
RUNTIME = ["cozy_runtime.author", "cozy_runtime.author._loader", "cozy_runtime.author._model", "cozy_runtime.internal.derive",
           "cozy_runtime.internal.encoding", "cozy_runtime.internal.fill", "cozy_runtime.internal.weights",
           "cozy_runtime.internal.resolution", "cozy_runtime.internal.triton_kernels"]
IMPORT = r'''
import importlib, json, resource, sys, time
t0 = time.perf_counter()
import torch
t1 = time.perf_counter()
for name in sys.argv[2:]:
    importlib.import_module(name)
t2 = time.perf_counter()
importlib.import_module(sys.argv[1])
t3 = time.perf_counter()
io = dict(l.split(": ") for l in open("/proc/self/io").read().splitlines())
ru = resource.getrusage(resource.RUSAGE_SELF)
print(json.dumps({"torch_s": round(t1 - t0, 3), "runtime_s": round(t2 - t1, 3), "package_s": round(t3 - t2, 3),
                  "read_bytes": int(io["read_bytes"]), "rchar": int(io["rchar"]), "majflt": ru.ru_majflt,
                  "cpu_s": round(ru.ru_utime + ru.ru_stime, 3), "modules": len(sys.modules)}))
'''
libc = ctypes.CDLL("libc.so.6", use_errno=True)
libc.mmap.restype = ctypes.c_void_p
libc.mmap.argtypes = [ctypes.c_void_p, ctypes.c_size_t, ctypes.c_int, ctypes.c_int, ctypes.c_int, ctypes.c_long]
def generation(module):
    rows = []
    for g in glob.glob(GENERATIONS + "/*/generation.json"):
        try:
            d = json.load(open(g))
        except (OSError, ValueError):
            continue
        if d.get("application", "").split(":")[0].split(".")[0] == module:
            rows.append((os.path.getmtime(g), os.path.dirname(g), d))
    if not rows:
        print(json.dumps({"module": module, "error": "no generation"})); sys.exit()
    _, root, d = max(rows, key=lambda r: r[0])
    env = os.path.realpath(os.path.join(root, "env"))
    return root, d, env, os.path.join(env, "bin", "python")
def files(module):
    """Every file the environment's imports can read: the environment (its links followed) and its interpreter's lib."""
    _, _, env, python = generation(module)
    base = os.path.dirname(os.path.dirname(os.path.realpath(python)))
    seen = set()
    for top in (env, os.path.join(base, "lib")):
        for dp, _, fs in os.walk(top, followlinks=True):
            for f in fs:
                try:
                    r = os.path.realpath(os.path.join(dp, f))
                    if r not in seen and os.path.isfile(r) and os.path.getsize(r) > 0:
                        seen.add(r)
                except OSError:
                    pass
    return sorted(seen)
def pages(path):
    """The file's pages in the page cache, one byte each (bit 0)."""
    size = os.path.getsize(path)
    fd = os.open(path, os.O_RDONLY)
    try:
        addr = libc.mmap(None, size, 1, 1, fd, 0)
        if addr in (None, ctypes.c_void_p(-1).value):
            return b""
        vec = ctypes.create_string_buffer((size + 4095) // 4096)
        rc = libc.mincore(ctypes.c_void_p(addr), ctypes.c_size_t(size), vec)
        libc.munmap(ctypes.c_void_p(addr), ctypes.c_size_t(size))
        return b"" if rc else vec.raw
    finally:
        os.close(fd)
def resident(path):
    return sum(b & 1 for b in pages(path)) * 4096
def spans(path):
    """The file's cached pages as [first page, count] runs."""
    out, start = [], None
    vec = pages(path)
    for i, b in enumerate(vec + b"\0"):
        if b & 1 and start is None:
            start = i
        elif not b & 1 and start is not None:
            out.append([start, i - start]); start = None
    return out
def kind(path):
    name = os.path.basename(path)
    return "so" if ".so" in name else "pyc" if name.endswith(".pyc") else "py" if name.endswith(".py") else "other"
def held(paths):
    by = {}
    for p in paths:
        try:
            r = resident(p)
        except OSError:
            continue
        k = by.setdefault(kind(p), {"files": 0, "bytes": 0, "touched": 0, "touched_bytes": 0, "resident": 0})
        k["files"] += 1; k["bytes"] += os.path.getsize(p); k["resident"] += r; k["touched"] += r > 0
        k["touched_bytes"] += os.path.getsize(p) if r else 0
    return by
def drop(paths):
    for p in paths:
        try:
            fd = os.open(p, os.O_RDONLY)
        except OSError:
            continue
        try:
            os.posix_fadvise(fd, 0, 0, os.POSIX_FADV_DONTNEED)
        finally:
            os.close(fd)
mode, args = sys.argv[1], sys.argv[2:]
if mode == "evict":
    paths = files(args[0])
    before = held(paths)
    os.sync()
    try:
        with open("/proc/sys/vm/drop_caches", "w") as f:
            f.write("3\n")
        dropped = "drop_caches"
    except OSError as e:
        dropped = f"fadvise ({e.strerror})"
    drop(paths)
    after = held(paths)
    gb = lambda by: round(sum(k["resident"] for k in by.values()) / 1e9, 3)
    print(json.dumps({"module": args[0], "how": dropped, "files": len(paths), "resident_gb_before": gb(before), "resident_gb_after": gb(after)}))
elif mode == "resident":
    by = held(files(args[0]))
    mb = lambda b: round(b / 1e6, 1)
    print(json.dumps({"module": args[0], "by_kind": {k: {**v, "bytes": mb(v["bytes"]), "touched_bytes": mb(v["touched_bytes"]), "resident": mb(v["resident"])} for k, v in by.items()}}))
elif mode == "snapshot":
    # The pages an import left cached (run right after a cold import, so nothing else put them there).
    rows = [[p, s] for p in files(args[0]) for s in [spans(p)] if s]
    json.dump(rows, open(f"/tmp/warming-{args[0]}.pages", "w"))
    print(json.dumps({"module": args[0], "files": len(rows), "mb": round(sum(n for _, s in rows for _, n in s) * 4096 / 1e6, 1)}))
elif mode == "preread":
    # What a machine-start pre-read would do: the snapshot's pages read back (WILLNEED, then reads in page order).
    rows = json.load(open(f"/tmp/warming-{args[0]}.pages"))
    t = time.perf_counter(); n = 0
    for p, runs in rows:
        try:
            fd = os.open(p, os.O_RDONLY)
        except OSError:
            continue
        try:
            for first, count in runs:
                os.posix_fadvise(fd, first * 4096, count * 4096, os.POSIX_FADV_WILLNEED)
            for first, count in runs:
                n += len(os.pread(fd, count * 4096, first * 4096))
        finally:
            os.close(fd)
    print(json.dumps({"module": args[0], "files": len(rows), "mb": round(n / 1e6, 1), "s": round(time.perf_counter() - t, 3)}))
elif mode == "import":
    root, d, env, python = generation(args[0])
    application = d["application"].split(":")[0]
    home = "/tmp/warming-home"; os.makedirs(home, exist_ok=True)
    w0 = time.perf_counter()
    p = subprocess.run([python, "-I", "-c", IMPORT, application, *RUNTIME], capture_output=True, text=True,
                       env={"PATH": "/usr/local/bin:/usr/bin:/bin", "LANG": "C.UTF-8", "HOME": home, "CUDA_VISIBLE_DEVICES": ""})
    wall = time.perf_counter() - w0
    out = json.loads(p.stdout.strip().splitlines()[-1]) if p.returncode == 0 and p.stdout.strip() else {"error": p.stderr[-600:]}
    out.update(module=args[0], generation=os.path.basename(root), wall_s=round(wall, 3))
    print(json.dumps(out))
elif mode == "disk":
    # The environment's largest file: sequential 1 MiB reads, then random 4 KiB reads, each from storage.
    paths = files(args[0])
    big = max(paths, key=os.path.getsize)
    size = os.path.getsize(big)
    drop([big]); fd = os.open(big, os.O_RDONLY); n = 0; t = time.perf_counter()
    while n < min(size, 512 << 20):
        n += len(os.pread(fd, 1 << 20, n))
    seq = n / (time.perf_counter() - t)
    drop([big]); lat = []
    for _ in range(300):
        off = random.randrange(0, size // 4096) * 4096; t = time.perf_counter(); os.pread(fd, 4096, off); lat.append(time.perf_counter() - t)
    os.close(fd); lat.sort()
    df = os.statvfs(os.path.dirname(big))
    print(json.dumps({"file": big, "gb": round(size / 1e9, 2), "seq_mb_s": round(seq / 1e6), "rand4k_p50_ms": round(lat[150] * 1e3, 3),
                      "rand4k_p90_ms": round(lat[270] * 1e3, 3), "fs_free_gb": round(df.f_bavail * df.f_frsize / 1e9)}))
elif mode == "ps":
    rows = []
    for d in glob.glob("/proc/[0-9]*"):
        try:
            argv = open(d + "/cmdline", "rb").read().split(b"\0")
            cmd = b" ".join(argv).decode(errors="replace")
            k = ("codec" if "frame-codec" in cmd else "compile" if "cozy_runtime.author.kernels" in cmd
                 else "executor" if "cozy_runtime.internal.executor" in cmd
                 else "tensord" if "machine serve" in cmd or os.path.basename(argv[0].decode(errors="replace")) == "tensord" else "")
            if not k:
                continue
            status = dict(l.split(":", 1) for l in open(d + "/status").read().splitlines() if ":" in l)
            fds = [os.readlink(f) for f in glob.glob(d + "/fd/*") if os.path.exists(f)]
            io = dict(l.split(": ") for l in open(d + "/io").read().splitlines())
            rows.append({"pid": int(d[6:]), "ppid": int(status["PPid"]), "kind": k, "rss_mib": int(status["VmRSS"].split()[0]) // 1024,
                         "nvidia_fds": sum("/dev/nvidia" in f for f in fds), "age_s": round(time.time() - os.stat(d).st_mtime, 1),
                         "read_mb": round(int(io["read_bytes"]) / 1e6, 1), "python": argv[0].decode(errors="replace")})
        except (OSError, KeyError, ValueError, IndexError):
            continue
    for r in rows:
        r["children"] = sum(o["ppid"] == r["pid"] for o in rows)
    print(json.dumps(sorted(rows, key=lambda r: r["pid"])))
elif mode in ("pages", "aside", "back"):
    # The environment's recorded import pages (xs-132's TensorD): what it holds, or moved aside and back.
    root, d, _, _ = generation(args[0])
    app = "".join(c if c.isascii() and c.isalnum() else "_" for c in d["application"])
    rec, aside = os.path.join(root, f"import-pages-{app}.json"), f"/tmp/import-pages-{args[0]}.aside"
    if mode == "aside":
        os.replace(rec, aside)
    elif mode == "back":
        os.replace(aside, rec)
    else:
        try:
            d = json.load(open(rec))
            print(json.dumps({"module": args[0], "files": len(d["files"]), "mb": round(d["bytes"] / 1e6, 1)}))
        except (OSError, ValueError) as e:
            print(json.dumps({"module": args[0], "error": str(e)}))
elif mode == "machine-mode":
    # /root/.cozy/config.toml's [machine] mode: set (shared, dedicated) or "unset" (the line this session added).
    path = os.path.expanduser("~/.cozy/config.toml")
    lines = open(path).read().splitlines() if os.path.exists(path) else []
    lines = [l for l in lines if l.strip() != 'mode = "shared"' and l.strip() != 'mode = "dedicated"']
    if args[0] != "unset":
        at = next((i for i, l in enumerate(lines) if l.strip() == "[machine]"), None)
        if at is None:
            lines += ["[machine]", f'mode = "{args[0]}"']
        else:
            lines.insert(at + 1, f'mode = "{args[0]}"')
    os.makedirs(os.path.dirname(path), exist_ok=True)
    open(path, "w").write("\n".join(lines) + "\n")
    print(json.dumps({"config": path, "machine_mode": args[0]}))
elif mode == "host":
    mem = dict(l.split(":", 1) for l in open("/proc/meminfo").read().splitlines())
    cpu = next((l.split(":", 1)[1].strip() for l in open("/proc/cpuinfo") if l.startswith("model name")), "")
    gens = []
    for g in glob.glob(GENERATIONS + "/*/generation.json"):
        try:
            gens.append({"generation": os.path.basename(os.path.dirname(g)), "application": json.load(open(g)).get("application")})
        except (OSError, ValueError):
            pass
    root = os.statvfs("/var/lib/cozy")
    print(json.dumps({"cpu": cpu, "cpus": os.cpu_count(), "mem_total_gb": round(int(mem["MemTotal"].split()[0]) / 1e6, 1),
                      "mem_available_gb": round(int(mem["MemAvailable"].split()[0]) / 1e6, 1), "cached_gb": round(int(mem["Cached"].split()[0]) / 1e6, 1),
                      "disk_free_gb": round(root.f_bavail * root.f_frsize / 1e9), "mount": next((l for l in open("/proc/mounts") if l.split()[1] in ("/var/lib/cozy", "/")), "").strip(),
                      "generations": gens}))
EOF
)
[ -f ~/.ssh/id_ed25519.pub ] || ssh-keygen -q -t ed25519 -N '' -f ~/.ssh/id_ed25519 || fail "ssh-keygen"
python3 -c 'import ast, sys; ast.parse(sys.stdin.read())' <<<"$POD_PY" || fail "the pod script does not parse"
RENT_ARGS=--ssh
quote "$(param traffic_gb 20)" "$(param need_s 3600)"
rent
pod() { timeout "${T:-120}" cozy machine ssh "$NAME" "$@"; }
for i in $(seq 1 20); do pod true < /dev/null > /dev/null 2>&1 && break; [ "$i" = 20 ] && fail "no SSH into $NAME"; sleep 15; done
podpy() { printf '%s' "$POD_PY" | T=900 pod "cat > /tmp/warming.py && python3 /tmp/warming.py $*"; }
smi() { pod "nvidia-smi --query-gpu=memory.used,memory.total --format=csv,noheader; nvidia-smi --query-compute-apps=pid,used_memory --format=csv,noheader" < /dev/null 2>&1 | paste -sd' '; }
podpy host > "$OUT/host.json"; say "host $(cat "$OUT/host.json")"

# runrec <label> <cozy run args…>: one run on $NAME followed to its end, its events stamped with the runner's clock
# (<label>.ndjson), its record in <label>.json and its number in <label>.n.
runrec() {
  local label=$1; shift; local t0 n
  t0=$(date +%s.%N)
  cozy run "$@" --on "$NAME" --json 2> "$OUT/$label.err" < /dev/null |
    python3 -u -c 'import sys,time; t0=float(sys.argv[1]); [print(f"{time.time()-t0:.3f} {l}", end="", flush=True) for l in sys.stdin]' "$t0" > "$OUT/$label.ndjson"
  n=$(grep -o '"number":[0-9]*' "$OUT/$label.ndjson" | head -1 | cut -d: -f2)
  [ -n "$n" ] || { say "$label: no run: $(tail -c 300 "$OUT/$label.err")"; return 1; }
  echo "$n" > "$OUT/$label.n"
  cozy run show "$n" --once --json > "$OUT/$label.json" 2>&1
  say "$label: run $n $(jq -r '"\(.status), accepted→settled \(((.settled_at_ms // 0) - (.accepted_at_ms // 0)) / 1000) s"' "$OUT/$label.json")"
}
# quiet <label>: no kernel compile child is running (each would import torch beside the trial).
quiet() {
  for i in $(seq 1 60); do
    podpy ps > "$OUT/ps-$1.json"
    [ "$(jq '[.[] | select(.kind == "compile")] | length' "$OUT/ps-$1.json")" = 0 ] && return 0
    sleep 5
  done
  say "$1: kernel compiles still running after 300 s"
}
# restart <label>: TensorD restarts (a machine update of only the GPU memory limit, already off), so no executor or
# import-only parent is left. Fails unless its process changed.
restart() {
  local before after
  before=$(podpy ps | jq -c '[.[] | select(.kind == "tensord") | .pid]')
  step "restart-$1" machine update "$NAME" --gpu-memory-limit off -y || fail "restart $1"
  for i in $(seq 1 60); do [ "$(state "$NAME")" = ready ] && break; sleep 2; done
  quiet "restart-$1"
  after=$(jq -c '[.[] | select(.kind == "tensord") | .pid]' "$OUT/ps-restart-$1.json")
  [ "$before" = "[]" ] && say "restart $1: no TensorD process seen by name; judged by the update alone"
  [ "$before" = "[]" ] || [ "$before" != "$after" ] || fail "restart $1: TensorD is still $before"
  say "restart $1: TensorD $before -> $after; executors $(jq -c '[.[] | select(.kind != "tensord") | {pid, kind, nvidia_fds}]' "$OUT/ps-restart-$1.json")"
}
PROMPTS=("a lighthouse keeper's chart table under a brass lamp, gouache" "a glass greenhouse on a cliff in sea fog, film photo"
  "a paper-crane mobile over a sleeping cat, watercolor" "a night ferry crossing a lantern-lit harbor, oil painting"
  "a botanist sketching ferns in a misty ravine, ink and wash" "a tin toy rocket on a windowsill at dawn, macro photo"
  "a beekeeper's cottage under a harvest moon, linocut" "an otter asleep on a floating lily pad, pastel"
  "a cable car above a cloud forest, travel poster" "a blacksmith's forge lit by sparks, chiaroscuro oil"
  "a desert caravan resting at a salt lake, ukiyo-e" "a toy train circling a snow globe village, tilt-shift photo"
  "a jazz trio on a rooftop at dusk, gouache" "a kelp forest with a sunken bicycle, underwater photo")
P=0
sdxl_input() {  # sdxl_input <file> <steps>: a request with its own prompt and seed
  jq -n --arg p "${PROMPTS[$P]}" --argjson s "$((RANDOM * 7 + P))" --argjson n "$2" \
    '{megapixels: 1.0, aspect_ratio: "1:1", steps: $n, guidance: 7.0, hidiffusion: false, prompt_upsampling: false, prompt: $p, seed: $s}' > "$1"
  P=$(( (P + 1) % ${#PROMPTS[@]} ))
}
gpu_parent() { jq -r '[.[] | select(.kind == "executor" and .nvidia_fds == 0)] | .[0].pid // empty' "$1"; }

item3() {
  runrec i3-prime "$SDXL" --warm imported || fail "sdxl --warm imported"
  runrec i3-prime-off "$SDXL" --warm off
  local t c label pid
  for t in $(seq 1 "$TRIALS"); do
    for c in cold warm; do
      label=i3-sdxl-$c-$t
      restart "$label"
      [ -z "$(gpu_parent "$OUT/ps-restart-$label.json")" ] || say "$label: a parent is up after the restart (this TensorD imports at start)"
      if [ "$c" = cold ]; then podpy evict sdxl > "$OUT/$label-evict.json"; say "$label evicted: $(cat "$OUT/$label-evict.json")"; fi
      runrec "$label" "$SDXL" --warm imported
      podpy ps > "$OUT/$label-ps.json"; pid=$(gpu_parent "$OUT/$label-ps.json")
      podpy resident sdxl > "$OUT/$label-resident.json"
      say "$label: parent $(jq -c --arg p "${pid:-0}" '[.[] | select(.pid == ($p | tonumber)) | {pid, rss_mib, read_mb, age_s}]' "$OUT/$label-ps.json"), resident $(jq -c .by_kind "$OUT/$label-resident.json")"
      runrec "$label-off" "$SDXL" --warm off
    done
  done
  step i3-in-h3 package install "$H3" --on "$NAME" -y || fail "install $H3"
  restart i3-direct
  podpy disk sdxl > "$OUT/i3-disk.json"; say "disk $(cat "$OUT/i3-disk.json")"
  local m
  for m in sdxl h3; do podpy import "$m" > "$OUT/i3-direct-$m-prime.json"; say "prime $m $(cat "$OUT/i3-direct-$m-prime.json")"; done
  # Each trial: cold (evicted), warm (cached), and pre-read (evicted, then the first cold import's pages read back, as
  # a machine-start pre-read would, timed on its own).
  local d
  for t in $(seq 1 "$TRIALS"); do
    for m in sdxl h3; do
      d=$OUT/i3-direct-$m
      podpy evict "$m" > "$d-cold-$t-evict.json"
      podpy import "$m" > "$d-cold-$t.json"; say "direct $m cold $t: $(cat "$d-cold-$t.json") after $(jq -c '{how, resident_gb_before, resident_gb_after}' "$d-cold-$t-evict.json")"
      if [ "$t" = 1 ]; then
        podpy snapshot "$m" > "$d-snapshot.json"; podpy resident "$m" > "$d-resident.json"
        say "direct $m: a cold import's pages $(cat "$d-snapshot.json"), by kind $(jq -c .by_kind "$d-resident.json")"
      fi
      podpy import "$m" > "$d-warm-$t.json"; say "direct $m warm $t: $(cat "$d-warm-$t.json")"
      podpy evict "$m" > /dev/null; podpy preread "$m" > "$d-preread-$t-read.json"
      podpy import "$m" > "$d-preread-$t.json"; say "direct $m pre-read $t: $(cat "$d-preread-$t-read.json"), then $(cat "$d-preread-$t.json")"
    done
  done
}

item4() {
  local pkg=${QUEUED%%/*} release=${QUEUED##*@}
  step i4-in package install "$pkg@$release" --on "$NAME" -y || fail "install $pkg@$release"
  runrec i4-prime "$QUEUED" --warm downloaded || fail "$QUEUED --warm downloaded"
  runrec i4-prime-off "$QUEUED" --warm off
  restart i4
  sdxl_input "$OUT/i4-a.yaml" 60
  runrec i4-a "$SDXL" --input "$OUT/i4-a.yaml" & local pa=$!
  local na='' i
  for i in $(seq 1 120); do
    na=$(grep -o '"number":[0-9]*' "$OUT/i4-a.ndjson" 2>/dev/null | head -1 | cut -d: -f2)
    [ -n "$na" ] && [ "$(cozy run show "$na" --once --json 2>/dev/null | jq -r .status)" = in_progress ] && break
    sleep 1
  done
  say "i4: A run ${na:-?} in progress"
  podpy ps > "$OUT/i4-ps-before-b.json"
  runrec i4-b "$QUEUED" --asset image="$REF/seed.jpg" & local pb=$!
  while kill -0 "$pa" 2>/dev/null; do
    printf '%s %s\n' "$(date +%s.%N)" "$(podpy ps)" >> "$OUT/i4-ps.ndjson"; sleep 1
  done
  wait "$pa"; wait "$pb"
  say "i4: A $(jq -c '{status, accepted_at_ms, started_at_ms, settled_at_ms, executor: .measurements.execution.executor.started_unix_ms}' "$OUT/i4-a.json")"
  say "i4: B $(jq -c '{status, accepted_at_ms, started_at_ms, settled_at_ms, executor: .measurements.execution.executor.started_unix_ms, legs: .measurements.execution.executor.legs_ms}' "$OUT/i4-b.json")"
}

item5() {
  runrec i5-clear "$SDXL" --warm off
  restart i5-cold
  sdxl_input "$OUT/i5-cold.yaml" 20
  runrec i5-cold "$SDXL" --input "$OUT/i5-cold.yaml" || fail "cold request"
  restart i5-warm
  runrec i5-warm-gpu "$SDXL" --warm gpu || fail "--warm gpu"
  say "i5: --warm gpu result $(jq -c .result "$OUT/i5-warm-gpu.json")"
  cozy machine show "$NAME" > "$OUT/i5-show-loaded.txt" 2>&1; cozy machine show "$NAME" --json > "$OUT/i5-show-loaded.json" 2>&1
  say "i5: loaded: $(grep '^warm ' "$OUT/i5-show-loaded.txt" | paste -sd';'); GPU $(smi)"
  sleep "$HOLD"
  cozy machine show "$NAME" > "$OUT/i5-show-held.txt" 2>&1; cozy machine show "$NAME" --json > "$OUT/i5-show-held.json" 2>&1
  podpy ps > "$OUT/i5-ps-held.json"
  say "i5: after ${HOLD} s: $(grep '^warm ' "$OUT/i5-show-held.txt" | paste -sd';'); GPU $(smi); executors $(jq -c '[.[] | select(.kind == "executor") | {pid, nvidia_fds, age_s}]' "$OUT/i5-ps-held.json")"
  sdxl_input "$OUT/i5-warm.yaml" 20
  runrec i5-warm "$SDXL" --input "$OUT/i5-warm.yaml" || fail "warm request"
  runrec i5-off "$SDXL" --warm off
  cozy machine show "$NAME" > "$OUT/i5-show-off.txt" 2>&1; cozy machine show "$NAME" --json > "$OUT/i5-show-off.json" 2>&1
  say "i5: after --warm off: warm lines $(grep -c '^warm ' "$OUT/i5-show-off.txt"); GPU $(smi)"
  local r
  for r in cold warm; do
    say "i5 $r: $(jq -c '{status, ms: ((.settled_at_ms // 0) - (.accepted_at_ms // 0)), execution_ms, executor: .measurements.execution.executor.started_unix_ms, accepted_at_ms}' "$OUT/i5-$r.json")"
  done
}

item3b() {
  step i3b-update machine update "$NAME" --tensord "$DEV" -y || fail "update to $BUILD"
  local v; v=$(cozy machine show "$NAME" --json | jq -r .status.version); say "3b: pod TensorD $v"
  [ "$v" = "$("$DEV" --version | awk '{print $2}')" ] || fail "3b: the pod runs TensorD $v"
  runrec i3b-prime "$SDXL" --warm imported || fail "3b: --warm imported"
  runrec i3b-prime-off "$SDXL" --warm off
  local t a l i
  for i in $(seq 1 30); do
    podpy pages sdxl > "$OUT/i3b-pages.json"; jq -e .mb "$OUT/i3b-pages.json" >/dev/null 2>&1 && break; sleep 2
  done
  jq -e .mb "$OUT/i3b-pages.json" >/dev/null 2>&1 || fail "3b: no import pages were recorded: $(cat "$OUT/i3b-pages.json")"
  say "3b: recorded $(cat "$OUT/i3b-pages.json")"
  for t in $(seq 1 "$TRIALS"); do
    for a in norecord record; do
      l=i3b-$a-$t
      [ "$a" = norecord ] && podpy aside sdxl
      podpy evict sdxl > "$OUT/$l-evict.json"; say "$l evicted: $(jq -c '{how, resident_gb_before, resident_gb_after}' "$OUT/$l-evict.json")"
      restart "$l"
      for i in $(seq 1 60); do
        [ "$(cozy package ls --on "$NAME" --json 2>/dev/null | jq -r '[.packages[] | select(.package == "sdxl" or .package == "_/sdxl") | .level] | first // empty')" = imported ] && break
        sleep 1
      done
      sdxl_input "$OUT/$l.yaml" 20
      runrec "$l" "$SDXL" --input "$OUT/$l.yaml"
      [ "$a" = norecord ] && podpy back sdxl
      say "$l: legs $(jq -c '.measurements.execution.executor.legs_ms | {torch_import, runtime_imports, package_import, fork}' "$OUT/$l.json")"
    done
  done
}

# png <file> <seed>: a 512x512 PNG of seeded noise, different per seed (birefnet memoizes by its input).
png() { python3 -I - "$1" "$2" <<'PY'
import random, struct, sys, zlib
out, seed = sys.argv[1], int(sys.argv[2])
rng = random.Random(seed); w = h = 512
rows = b"".join(b"\0" + bytes(rng.randrange(256) for _ in range(w * 3)) for _ in range(h))
chunk = lambda t, d: struct.pack(">I", len(d)) + t + d + struct.pack(">I", zlib.crc32(t + d) & 0xffffffff)
open(out, "wb").write(b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", struct.pack(">IIBBBBB", w, h, 8, 2, 0, 0, 0)) + chunk(b"IDAT", zlib.compress(rows, 6)) + chunk(b"IEND", b""))
PY
}
# queue4b <label>: SDXL (60 steps) executing; birefnet, then the pipeline, submitted behind it; processes each second.
queue4b() {
  local l=$1 n i
  restart "$l"
  podpy host > "$OUT/$l-host.json"
  sdxl_input "$OUT/$l-a.yaml" 60
  runrec "$l-a" "$SDXL" --input "$OUT/$l-a.yaml" & local pa=$!
  for i in $(seq 1 240); do
    n=$(grep -o '"number":[0-9]*' "$OUT/$l-a.ndjson" 2>/dev/null | head -1 | cut -d: -f2)
    [ -n "$n" ] && [ -n "$(cozy run show "$n" --once --json 2>/dev/null | jq -r '.started_at_ms // empty')" ] && break
    sleep 0.5
  done
  say "$l: SDXL run ${n:-?} executing"
  png "$OUT/$l-b.png" "$RANDOM"; png "$OUT/$l-c.png" "$RANDOM"
  runrec "$l-b" "$QUEUED" --asset image="$OUT/$l-b.png" & local pb=$!
  sleep 2
  runrec "$l-c" "$PIPE/cut" --asset image="$OUT/$l-c.png" & local pc=$!
  while kill -0 "$pa" 2>/dev/null || kill -0 "$pb" 2>/dev/null || kill -0 "$pc" 2>/dev/null; do
    printf '{"t": %s, "ps": %s}\n' "$(date +%s%3N)" "$(podpy ps)" >> "$OUT/$l-ps.ndjson"; sleep 1
  done
  wait "$pa" "$pb" "$pc"
  for r in a b c; do
    say "$l-$r: $(jq -c '{number, status, accepted_at_ms, started_at_ms, settled_at_ms, executor: .measurements.execution.executor.started_unix_ms, legs: .measurements.execution.executor.legs_ms, calls: [.calls[]? | {called_at_ms, executor: .measurements.execution.executor.started_unix_ms, legs: .measurements.execution.executor.legs_ms}]}' "$OUT/$l-$r.json" | cut -c1-600)"
  done
}
item4b() {
  step i4b-update machine update "$NAME" --tensord "$DEV" -y || fail "update to $BUILD"
  local v; v=$(cozy machine show "$NAME" --json | jq -r .status.version); say "4b: pod TensorD $v"
  [ "$v" = "$("$DEV" --version | awk '{print $2}')" ] || fail "4b: the pod runs TensorD $v"
  step i4b-in-sdxl package install "${SDXL%%/*}@${SDXL##*@}" --on "$NAME" -y || fail "install sdxl"
  step i4b-in-queued package install "${QUEUED%%/*}@${QUEUED##*@}" --on "$NAME" -y || fail "install $QUEUED"
  step i4b-in-pipe package install "$PIPE" --on "$NAME" -y || fail "install the pipeline fixture"
  runrec i4b-prime "$QUEUED" --warm downloaded; runrec i4b-prime-off "$QUEUED" --warm off
  podpy machine-mode shared > "$OUT/i4b-mode.json"; say "4b: $(cat "$OUT/i4b-mode.json")"
  queue4b i4b-shared
  say "4b shared: machine says mode $("$DEV" machine show "$NAME" --json 2>/dev/null | jq -r '.status.mode // "?"')"
  podpy machine-mode unset > /dev/null
  queue4b i4b-dedicated
  say "4b dedicated: machine says mode $("$DEV" machine show "$NAME" --json 2>/dev/null | jq -r '.status.mode // "?"')"
}

for item in $ITEMS; do
  case $item in
    3) item3;; 4) item4;; 5) item5;; 3b) item3b;; 4b) item4b;;
    *) fail "no item $item";;
  esac
done
say "PASS: items $ITEMS ran"
