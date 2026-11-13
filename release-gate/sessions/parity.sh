# parity (xs-194): a package run from source behaves as its Hub install does, since gates run the RC's packages from
# source (Paul 11-13). One rental with SSH. For each cell's package: its source from a packages_refs checkout (Triton sets
# built into it first, as a release ships them: lib.sh `kernels`) and its Hub release, each run cold (a new executor,
# its weights on disk and out of the page cache), source first so it can't take the release's kernels from the machine's
# store, then each warm on the next seed. Held side by side: the two environments' distributions and the `_triton`
# kernels each imports (each generation's, over SSH), cold and warm execution, step 1, steady step and the kernels
# served, and the output bytes.
# params:
#   tree     the packages_refs checkout (pk)
#   cells    [{"package": dir, "release": the Hub's, "function", "args": [inputs and flags; {seed}, {clip}: a 1.5 s
#            320x176 test video made here], "download": [[model ref, `cozy model install` flags…], …]}]
#   need_s   seconds the cap must buy (2700)
#   slack    how much slower a source cold run's execution may be than the release's (1.15)
. "$(dirname "$0")/lib.sh"
TREE=$(param tree pk) CELLS=$(param cells '[]') SLACK=$(param slack 1.15)
SRC=$PACKAGES/$TREE
[ -d "$SRC" ] || fail "no packages_refs checkout $TREE"
[ "$(jq length <<<"$CELLS")" -gt 0 ] || fail "no cells"
[ -f ~/.ssh/id_ed25519.pub ] || ssh-keygen -q -t ed25519 -N '' -f ~/.ssh/id_ed25519 || fail "ssh-keygen"
RENT_ARGS=--ssh
command -v ffmpeg > /dev/null || fail "no ffmpeg"
CLIP=$OUT/clip.mp4
ffmpeg -nostdin -v error -y -f lavfi -i testsrc2=size=320x176:rate=24 -t 1.5 -pix_fmt yuv420p -c:v libx264 "$CLIP" || fail "the test clip"
mapfile -t PKGS < <(jq -r '.[].package' <<<"$CELLS")
for p in "${PKGS[@]}"; do
  [[ $p =~ ^[a-z0-9.-]+$ && -f $SRC/$p/pyproject.toml ]] || fail "no package $p in $TREE"
  rel=$(jq -r --arg p "$p" 'map(select(.package==$p))[0].release' <<<"$CELLS")
  say "cell $p: source $(git -C "$SRC" rev-parse --short HEAD) at $(grep -m1 '^version = ' "$SRC/$p/pyproject.toml" | cut -d'"' -f2), against the Hub's $p@$rel"
  kernels "$SRC/$p"
done
quote "$(param need_s 2700)"
rent
pod() { timeout "${T:-120}" cozy machine ssh "$NAME" "$@"; }
for i in $(seq 1 20); do pod true < /dev/null > /dev/null 2>&1 && break; [ "$i" = 20 ] && fail "no SSH into $NAME"; sleep 15; done
POD_PY=$(cat <<'EOF'
import glob, json, os, subprocess, sys
mode = sys.argv[1]
if mode == "evict":
    # The weights out of the page cache: each cold run reads them from the disk, as the first one did.
    tops = set()
    for env in glob.glob("/proc/[0-9]*/environ"):
        try:
            for kv in open(env, "rb").read().split(b"\0"):
                for name in (b"TENSORFS_HOME=", b"COZY_TENSORFS_ROOT="):
                    if kv.startswith(name) and os.path.isdir(kv[len(name):].decode()):
                        tops.add(kv[len(name):].decode())
        except OSError:
            pass
    files = [os.path.join(d, f) for r in tops for d, _, fs in os.walk(r) for f in fs]
    files = [f for f in files if os.path.isfile(f) and not os.path.islink(f) and os.path.getsize(f) > (1 << 20)]
    os.sync()
    for f in files:
        fd = os.open(f, os.O_RDONLY)
        try:
            os.posix_fadvise(fd, 0, 0, os.POSIX_FADV_DONTNEED)
        finally:
            os.close(fd)
    print(json.dumps({"files": len(files), "gb": round(sum(os.path.getsize(f) for f in files) / 1e9, 2)}))
elif mode == "envs":
    # Each generation: its record's names, every distribution its environment holds, and the `_triton` kernels each
    # named module imports (its manifest's keys), read by the generation's own Python.
    probe = ("import importlib.metadata as m, importlib.util as u, json, os, sys\n"
             "d = {x.metadata['Name'].lower().replace('_', '-'): x.version for x in m.distributions()}\n"
             "t = {}\n"
             "for mod in sys.argv[1:]:\n"
             "    s = u.find_spec(mod)\n"
             "    f = os.path.join(os.path.dirname(s.origin), '_triton', 'manifest.json') if s and s.origin else ''\n"
             "    t[mod] = {'at': os.path.dirname(s.origin) if s and s.origin else None,\n"
             "              'kernels': sorted(json.load(open(f)).get('kernels', {})) if f and os.path.exists(f) else None}\n"
             "print(json.dumps({'dists': d, 'triton': t}))\n")
    for g in sorted(glob.glob("/var/lib/cozy/tensord/generations/*/generation.json")):
        record = json.load(open(g))
        python = os.path.join(os.path.dirname(g), "env", "bin", "python")
        said = subprocess.run([python, "-I", "-c", probe, *sys.argv[2:]], capture_output=True, text=True)
        row = {"generation": os.path.basename(os.path.dirname(g)), "record": {k: v for k, v in record.items() if isinstance(v, str)}}
        try:
            row.update(json.loads(said.stdout))
        except ValueError:
            row["error"] = (said.stderr or said.stdout)[-400:]
        print(json.dumps(row))
EOF
)
podpy() { printf '%s' "$POD_PY" | T=600 pod "cat > /tmp/parity.py && python3 /tmp/parity.py $*"; }
# summary <name>: execution, the longest step track's step 1 and steady step, peak VRAM, each Triton set's source
# (shipped: the package's own `_triton`; store: the machine's; compiled) and the plans applied.
summary() { jq -r '.measurements as $m | ($m.attribution.steps // {} | to_entries | max_by(.value.count // 0)) as $s | ($s.value.series // [] | map(.[1])) as $x
  | "exec \((.execution_ms // 0) / 1000 * 10 | floor / 10)s, \($s.key // "-") step1 \(($x[0] // 0) / 100 | floor / 10)s steady \((if ($x | length) > 1 then ($x[1:] | add / length) else 0 end) / 100 | floor / 10)s x\($x | length), "
  + "peak VRAM \(([$m.memory.driver[]?.peak_bytes] | max // 0) / 107374182.4 | floor / 10) GiB, "
  + "kernels \([.. | objects | select(has("kernel") and has("source")) | "\(.kernel)=\(.source)"] | unique | join(" ")), "
  + "plan \([$m.execution.plan_reports[]? | to_entries[] | "\(.key)=\(.value.applied)"] | join(" "))"' "$OUT/$1.json" 2>/dev/null; }
exec_ms() { jq -r '.execution_ms // 0' "$OUT/$1.json" 2>/dev/null; }
digests() { jq -r '[.result | .. | .digest? // empty] | join(",")' "$OUT/$1.json" 2>/dev/null; }
# cell <name> <target> <seed> <cell JSON>: one run followed to its end, its record and outputs kept.
cell() {
  local args=() a N st
  mapfile -t args < <(jq -r '.args[]' <<<"$4")
  for a in "${!args[@]}"; do args[$a]=${args[$a]//\{seed\}/$3}; args[$a]=${args[$a]//\{clip\}/$CLIP}; done
  (cd "$SRC" && step "$1" run "$2" "${args[@]}" --on "$NAME")
  N=$(runno "$1")
  [ -n "$N" ] || fail "$1: no run: $(tail -c 400 "$OUT/$1.txt")"
  cozy run watch "$N" --once --json --full > "$OUT/$1.json" 2>/dev/null; cozy run watch "$N" --once -o "$OUT/$1.out" > /dev/null 2>&1
  st=$(jq -r '.status // "none"' "$OUT/$1.json" 2>/dev/null)
  say "$1 run $N $st: $(summary "$1")"
  [ "$st" = completed ] || fail "$1 $st: $(jq -r '.reason.message // .reason // empty' "$OUT/$1.json" 2>/dev/null | head -c 400)"
}
bad=0
for p in "${PKGS[@]}"; do
  c=$(jq -c --arg p "$p" 'map(select(.package==$p))[0]' <<<"$CELLS")
  fn=$(jq -r .function <<<"$c") rel=$(jq -r .release <<<"$c") seed=$(( $(date +%s) % 100000 ))
  mapfile -t downloads < <(jq -c '.download // [] | .[]' <<<"$c")
  for i in "${!downloads[@]}"; do
    mapfile -t dl < <(jq -r '.[]' <<<"${downloads[$i]}")
    step "download-$p-$i" model install "${dl[@]}" --on "$NAME" < /dev/null || fail "$p: the download of ${dl[0]}"
  done
  say "$p: page cache $(podpy evict < /dev/null)"
  cell "src-$p-cold" "./$p/$fn" "$seed" "$c"
  say "$p: page cache $(podpy evict < /dev/null)"
  cell "hub-$p-cold" "$p/$fn@$rel" "$seed" "$c"
  cell "src-$p-warm" "./$p/$fn" "$((seed + 1))" "$c"
  cell "hub-$p-warm" "$p/$fn@$rel" "$((seed + 1))" "$c"
  for k in cold warm; do
    a=$(digests "src-$p-$k") b=$(digests "hub-$p-$k")
    if [ -n "$a" ] && [ "$a" = "$b" ]; then say "$p $k: IDENTICAL output ($a)"; else say "$p $k: DIFFERENT output: source $a, Hub $b"; bad=$((bad + 1)); fi
  done
  s=$(exec_ms "src-$p-cold") h=$(exec_ms "hub-$p-cold")
  if awk -v s="$s" -v h="$h" -v k="$SLACK" 'BEGIN { exit !(h > 0 && s <= h * k) }'; then say "$p cold execution: source ${s} ms, Hub ${h} ms (within ${SLACK}x)"
  else say "$p cold execution: source ${s} ms, Hub ${h} ms: SLOWER than ${SLACK}x"; bad=$((bad + 1)); fi
done
mods=$(for p in "${PKGS[@]}"; do sed -n '/cozy\.application/,/^\[/s/^default = "\([A-Za-z0-9_]*\)[.:].*/\1/p' "$SRC/$p/pyproject.toml"; done | paste -sd' ')
podpy envs $mods < /dev/null > "$OUT/envs.jsonl" || fail "the environments: $(tail -c 300 "$OUT/envs.jsonl")"
python3 -I - "$OUT/envs.jsonl" "${PKGS[@]}" <<'PY' | tee -a "$R"
import json, re, sys
rows = [json.loads(line) for line in open(sys.argv[1]) if line.strip()]
norm = lambda n: re.sub(r"[-_.]+", "-", n).lower()
failed = 0
for package in sys.argv[2:]:
    mine = [r for r in rows if norm(r["record"].get("package", "")) == norm(package) and "dists" in r]
    # A Hub install's module is in its environment's site-packages; a source run's, in its generation's tree.
    source = [r for r in mine if any(t.get("at") and "site-packages" not in t["at"] for t in r["triton"].values())]
    hub = [r for r in mine if r not in source]
    if not source or not hub:
        print(f"{package}: environments not found (source {len(source)}, Hub {len(hub)}): {[r['record'] for r in rows]}"); failed += 1; continue
    s, h = source[-1], hub[-1]
    ds = {k: v for k, v in s["dists"].items() if k != norm(package)}
    dh = {k: v for k, v in h["dists"].items() if k != norm(package)}
    differ = sorted(f"{k}: source {ds.get(k)}, Hub {dh.get(k)}" for k in set(ds) | set(dh) if ds.get(k) != dh.get(k))
    print(f"{package}: {len(ds)} distributions besides itself, " + ("the SAME versions" if not differ else f"{len(differ)} DIFFER: " + "; ".join(differ)))
    for module, ts in s["triton"].items():
        th = h["triton"].get(module, {})
        same = ts.get("kernels") == th.get("kernels")
        n = len(ts.get("kernels") or [])
        print(f"{package} {module}/_triton: source {n if ts.get('kernels') is not None else 'none'} kernels at {ts.get('at')}, "
              f"Hub {len(th.get('kernels') or []) if th.get('kernels') is not None else 'none'} at {th.get('at')}: {'the SAME keys' if same else 'DIFFERENT keys'}")
        failed += not same
    failed += bool(differ)
sys.exit(1 if failed else 0)
PY
envs=${PIPESTATUS[0]}
[ "$bad" = 0 ] && [ "$envs" = 0 ] || fail "$bad output or time differences, environments $([ "$envs" = 0 ] && echo same || echo differ)"
say "parity: source and Hub install alike for ${PKGS[*]}"
