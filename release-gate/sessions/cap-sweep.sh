# cap-sweep (var-030): a list of package functions, each run once on one rental whose GPU is capped
# (`cozy machine update --gpu-memory-limit`, as suites escaped and varena cap theirs), to its end. Paul, 11-18: "inference
# of every model we have should work on a 6GB VRAM budget. It might be slow, but it won't OOM and it'll work. Make sure
# that's the case". A cell that fails is a finding, not the session's failure: every cell runs, and the session passes
# when the sweep reached its end. Slow is not failed: a run is canceled only on evidence it is stuck (its progress has
# not moved for `stall_s` and the pod's GPU and disk were then idle for a minute), at its own `max_s`, or when the
# session's `need_s` are used; the last two are said as unfinished, with where the run stood.
# One line per cell in results.txt: `CELL <name> | run | status | total, execution | step 1, steady | peak VRAM by the
# run's record and by the pod's nvidia-smi | memoized | throttled | where it stood | why`.
# params:
#   tree       the packages_refs checkout the functions run from (required)
#   cap        the GPU's limit (6GiB)
#   runtime    the Runtime wheel, https://…#sha256=<hex>, or a cozy-runtime dev-build run id (required)
#   tensorfs   the TensorFS wheel, likewise (required); the pod's TensorD is this session's cozy
#   cells      [{"name", "target" ("<package>/<function>"), "args" ([inputs and flags]),
#              "max_s" (optional: the run is canceled this long in, and said to be unfinished, not failed),
#              "after" (optional: runs only if that cell completed), "unless" (optional: only if that cell did not)}]
#              in args: {seed} {lighthouse} {teapot} are 1024x1024 pictures, {pic1}..{pic16} 2048x2048 ones, {depth} and
#              {lines} a grey and an edge map of {seed}, {long} an 8192x1024 picture, {clip1} 1 s of 640x360, {clip5} and
#              {clip15} 1344x768 at 24 fps with a tone, {clip5q} {clip5} at 2560x1440, {digits} 65,000 characters of
#              digit groups (a long prompt), and {out:<cell>|<file>} an earlier cell's picture or video, else <file>
#   need_s     the session's seconds from its start, which the cap in USD must buy (3600), at most 19800: a hosted job
#              ends at 6 h with nothing kept
#   stall_s    how long a run's progress may stand before the pod's GPU and disk are read (1800)
#   twin_s     an uncapped twin's time cap (2400)
#   provider   the GPU provider the rental is ordered from (runpod or vast; empty: the Hub's choice)
#   quiet_s    lib.sh's quiet window; set it above stall_s, which judges a quiet run here by the pod's GPU and disk
#   file       a JSON file in this checkout holding all of these (lib.sh)
# A cell that completed under the cap runs once more uncapped on the same pod (`<cell>-uncapped`, another seed) unless its
# `twin` is false: what the cap costs in time. Every record must name the Runtime and TensorFS the pod was moved to: a cell
# that ran another pair measured nothing, and the session stops there (so would every cell after it).
. "$(dirname "$0")/lib.sh"
TREE=$(param tree '') CAPV=$(param cap 6GiB) RW=$(param runtime '') TW=$(param tensorfs '') CELLS=$(param cells '[]')
STALL=$(param stall_s 1800) NEED=$(param need_s 3600) PROVIDER=$(param provider '')
[[ $PROVIDER =~ ^(|runpod|vast)$ ]] || fail "provider $PROVIDER"
[ "$QUIET_S" -gt "$STALL" ] || fail "quiet_s $QUIET_S: lib.sh's step would end a quiet run before stall_s $STALL judges it"
[[ $CAPV =~ ^[0-9]+[MG]iB$ ]] || fail "cap $CAPV"
[[ $STALL =~ ^[0-9]+$ && $NEED =~ ^[0-9]+$ ]] || fail "stall_s $STALL, need_s $NEED"
[ "$NEED" -le 19800 ] || fail "need_s $NEED: a hosted job ends at 6 h with nothing kept; split the session"
END_AT=$(( $(date +%s) + NEED ))
[ -n "$TREE" ] && [ -d "$PACKAGES/$TREE" ] || fail "no packages checkout: dispatch with -f packages_refs=<name>=<ref> and params.tree=<name>"
for tool in jq python3 ffmpeg; do command -v "$tool" > /dev/null || fail "no $tool"; done
V=$(cozy --version | awk '{print $2}')
# wheel <URL#sha256=hex>: the wheel under its own name (a URL's %2B is a version's +), held to its digest.
# A dev build's wheel (a cozy-runtime dev-build run id, or `artifact:<run id>/<name>` of a cozy-creator run): lib.sh's
# wheel_from, under its version's own name.
wheel() {
  local n f
  case $1 in
    https://*'#sha256='*) ;;
    https://*) return 1 ;;
    *) f=$(wheel_from "${RUNNER_TEMP:-$OUT}/wheels/${1//[:\/]/_}" "$1") && [ -n "$f" ] && echo "$f"; return ;;
  esac
  n=$(basename "${1%%#*}"); f=${RUNNER_TEMP:-$OUT}/wheels/$(printf '%b' "${n//%/\\x}"); mkdir -p "${f%/*}"
  curl -fsSL --retry 5 --retry-all-errors "${1%%#*}" -o "$f" && [ "$(sha256sum "$f" | cut -c1-64)" = "${1##*sha256=}" ] && echo "$f"
}
RWF=$(wheel "$RW") || fail "runtime is not a wheel at https://…#sha256=<hex> that hashes to it, nor a dev build's"
TWF=$(wheel "$TW") || fail "tensorfs is not a wheel at https://…#sha256=<hex> that hashes to it"
say "candidate: cozy $V · ${RWF##*/} · ${TWF##*/} · packages $(git -C "$PACKAGES/$TREE" rev-parse --short HEAD) · cap $CAPV"

# The cells' inputs, made here.
FX=$(cd "$(dirname "$0")/../suites/bench/fixtures" && pwd) M=$OUT/media
mkdir -p "$M"
clip() {
  ffmpeg -nostdin -v error -y -loop 1 -i "$REF/seed.jpg" -f lavfi -i "sine=frequency=440:duration=$1" \
    -vf "scale=1792:-2,crop=1344:768:'(iw-1344)*t/$1':'(ih-768)/2',fps=24,format=yuv420p" -t "$1" -c:v libx264 -crf 18 -c:a aac -shortest "$M/clip$1.mp4"
}
clip 5 && clip 15 || fail "the clips"
ffmpeg -nostdin -v error -y -i "$M/clip5.mp4" -vf "scale=2560:1440:flags=lanczos" -c:v libx264 -crf 18 -c:a copy "$M/clip5q.mp4" || fail "the 1440p clip"
ffmpeg -nostdin -v error -y -loop 1 -i "$REF/seed.jpg" -vf "scale=1024:1024,tile=8x1" -frames:v 1 -q:v 2 "$M/long.jpg" || fail "the long picture"
DIGITS=$(awk 'BEGIN { x = 20261118; for (i = 0; i < 16250; i++) { x = (x * 48271) % 2147483647; printf "%03d ", x % 1000 } }')
SRC=("$REF/seed.jpg" "$FX/lighthouse.jpg" "$FX/teapot.jpg")
for i in $(seq 16); do
  ffmpeg -nostdin -v error -y -i "${SRC[$((i % 3))]}" -vf "hue=h=$((i * 22)),scale=2048:2048:flags=lanczos" -q:v 2 "$M/pic$i.jpg" || fail "picture $i"
done
ffmpeg -nostdin -v error -y -i "$REF/seed.jpg" -vf "format=gray,gblur=sigma=8" "$M/depth.png" &&
  ffmpeg -nostdin -v error -y -i "$REF/seed.jpg" -vf "edgedetect=low=0.1:high=0.3,negate" "$M/lines.png" || fail "the control maps"
declare -A MADE=() ENDED=()
# expand <arg>: its placeholders as files.
expand() {
  local a=$1
  a=${a//\{seed\}/$REF/seed.jpg}; a=${a//\{lighthouse\}/$FX/lighthouse.jpg}; a=${a//\{teapot\}/$FX/teapot.jpg}
  a=${a//\{clip1\}/$FX/clip-1s-360p.mp4}; a=${a//\{clip5\}/$M/clip5.mp4}; a=${a//\{clip15\}/$M/clip15.mp4}
  a=${a//\{depth\}/$M/depth.png}; a=${a//\{lines\}/$M/lines.png}; a=${a//\{long\}/$M/long.jpg}
  a=${a//\{clip5q\}/$M/clip5q.mp4}; a=${a//\{digits\}/$DIGITS}
  while [[ $a =~ \{pic([0-9]+)\} ]]; do a=${a//"${BASH_REMATCH[0]}"/$M/pic${BASH_REMATCH[1]}.jpg}; done
  while [[ $a =~ \{out:([A-Za-z0-9_-]+)\|([^}]+)\} ]]; do a=${a//"${BASH_REMATCH[0]}"/${MADE[${BASH_REMATCH[1]}]:-${BASH_REMATCH[2]}}}; done
  printf '%s' "$a"
}
mapfile -t names < <(jq -r '.[].name' <<< "$CELLS")
[ "${#names[@]}" -gt 0 ] || fail "no cells"
for k in "${names[@]}"; do
  c=$(jq -c --arg k "$k" 'map(select(.name == $k)) | first' <<< "$CELLS")
  [[ $k =~ ^[A-Za-z0-9_-]+$ ]] && [ "$(jq --arg k "$k" 'map(select(.name == $k)) | length' <<< "$CELLS")" = 1 ] || fail "cell names must be unique words: $k"
  t=$(jq -r .target <<< "$c")
  [ -f "$PACKAGES/$TREE/${t%/*}/pyproject.toml" ] || fail "cell $k: no package ${t%/*} in $TREE"
  for other in $(jq -r '.after // empty, .unless // empty' <<< "$c"); do
    [[ " ${names[*]} " == *" $other "* ]] || fail "cell $k waits on $other, which is no cell"
  done
  while IFS= read -r a; do
    for f in $(expand "$a" | grep -oE "($M|$FX|$REF)/[A-Za-z0-9._-]+"); do [ -f "$f" ] || fail "cell $k: no $f"; done
  done < <(jq -r '.args[]' <<< "$c")
  # ledger.py reads these two lines: the cell's package code and its cap.
  say "cell $k: $(git -C "$PACKAGES/$TREE" rev-parse --short HEAD) ./$t $(jq -r '.args | join(" ")' <<< "$c" | cut -c1-600)"
  say "cell $k: caps VRAM at $CAPV"
  kernels "$PACKAGES/$TREE/${t%/*}"
done
printf 'run 5 · sync local/x (source changed)\nrun 6 · local/x/f\nrun      6\n' > "$OUT/probe-cell.txt"
current() { awk '$1=="run" && $3=="·" && $4!="sync"{n=$2} END{print n}' "$OUT/$1.txt" 2>/dev/null; }
[ "$(runno probe-cell)" = 6 ] && [ "$(current probe-cell)" = 6 ] || fail "parsers"
[ -f ~/.ssh/id_ed25519.pub ] || ssh-keygen -q -t ed25519 -N '' -f ~/.ssh/id_ed25519 || fail "ssh-keygen"
# `--provider` is a dev flag: on a hosted runner its own cozy home says `dev = true` while the rental is quoted and ordered,
# and no longer once it is placed. Calcifer read its config when it started, before this, so it is no dev home: a dev
# home's rental follows the RC's builds (rc.rs `follows`), which would move the pod off the builds under test.
DEVC=~/.cozy/config.toml
if [ -n "$PROVIDER" ]; then
  [ -n "${GITHUB_ACTIONS:-}" ] || fail "provider: only on a hosted runner, whose cozy home is its own"
  mkdir -p ~/.cozy; touch "$DEVC"; cp "$DEVC" "${RUNNER_TEMP:-/tmp}/config.toml.kept"
  { echo 'dev = true'; cat "${RUNNER_TEMP:-/tmp}/config.toml.kept"; } > "$DEVC"
fi
RENT_ARGS="--ssh${PROVIDER:+ --provider $PROVIDER}"
quote "$NEED"
rent
[ -z "$PROVIDER" ] || cp "${RUNNER_TEMP:-/tmp}/config.toml.kept" "$DEVC"
jq -e '.status.gpus | length == 1' "$OUT/show.json" > /dev/null || fail "not one GPU"
# A dev home on an RC build moves its rental to the RC's builds once it's ready: nothing starts before that ends.
cozy machine show "$NAME" --json > "$OUT/show-rc.json" 2> /dev/null
if [ -n "$(jq -r '.rc.state // empty' "$OUT/show-rc.json")" ]; then
  for _ in $(seq 180); do
    [ "$(jq -r '.rc.state // empty' "$OUT/show-rc.json")" = current ] && [ "$(jq -r '.state // empty' "$OUT/show-rc.json")" = ready ] && break
    sleep 5; cozy machine show "$NAME" --json > "$OUT/show-rc.json" 2> /dev/null
  done
fi
step update machine update "$NAME" --tensord "$(command -v cozy)" --runtime "$RWF" --tensorfs "$TWF" --gpu-memory-limit "$CAPV" -y ||
  fail "update: $(tail -c 300 "$OUT/update.txt")"
WANT=$(( ${CAPV%[MG]iB} << 20 )); [[ $CAPV != *GiB ]] || WANT=$(( ${CAPV%GiB} << 30 ))
said=''
for _ in $(seq 60); do
  cozy machine show "$NAME" --json > "$OUT/show-capped.json" 2> /dev/null
  said=$(jq -r '[.status.gpus[]? | .memory_limit_bytes // 0 | tostring] | unique | join(",")' "$OUT/show-capped.json" 2> /dev/null)
  [ "$said" = "$WANT" ] && [ "$(jq -r '.state // empty' "$OUT/show-capped.json")" = ready ] && break
  sleep 5
done
[ "$said" = "$WANT" ] || fail "the cap: machine show says ${said:-nothing} bytes, not $WANT"
on=$(jq -r '.status.version // empty' "$OUT/show-capped.json") rt=$(jq -r '.status.runtime // empty' "$OUT/show-capped.json")
tfs=$(jq -r '.status.tensorfs // empty' "$OUT/show-capped.json")
[ "$on" = "$V" ] || fail "$NAME runs TensorD $on, not this session's $V"
[[ ${RWF##*/} == *"-${rt}-"* ]] || fail "$NAME runs Runtime $rt, not ${RWF##*/}"
[[ ${TWF##*/} == *"-${tfs}-"* ]] || fail "$NAME runs TensorFS $tfs, not ${TWF##*/}"
say "under test: cozy $on · Runtime $rt · TensorFS $tfs · cap $CAPV ($said bytes of $(jq -r '.status.gpus[0] | "\(.name) \(.memory_bytes)"' "$OUT/show-capped.json"))"
# pod <script> [seconds]: on the pod's root shell, its last line; nothing when ssh does not answer.
pod() {
  timeout "${2:-30}" cozy machine ssh "$NAME" "echo $(base64 -w0 <<< "$1") | base64 -d | sh" < /dev/null 2> /dev/null |
    grep -v '^Warning: Permanently added' | tail -1
}
say "host: $(pod 'echo "$(nproc) cores, $(awk "/^MemTotal/ {printf \"%.0f\", \$2 / 1048576}" /proc/meminfo) GiB RAM, driver and card MiB $(nvidia-smi --query-gpu=driver_version,memory.total --format=csv,noheader,nounits | head -1)"' 60)"
. "$(dirname "$0")/gpu-reader.sh"
# The pod's own logs leave with it.
before_end() {
  # A run left pinned to a pod that ends is lost to its owner (cl-1316): each one still queued or running is canceled first.
  local clock r
  if [ -n "$NAME" ] && clock=$(run_clock "$NAME"); then
    for r in ${clock#* }; do [ "$r" = "${clock%% *}" ] || { cozy run cancel "$r" > /dev/null 2>&1; say "canceled $r before $NAME ends"; }; done
  fi
  readings 8 || say "the pod's GPU readings were not fetched before it ended"
  timeout 300 cozy machine logs "$NAME" -n 0 > "$OUT/tensord-end.log" 2>&1
  timeout 120 cozy machine logs "$NAME" memory -n 0 > "$OUT/memory-end.log" 2>&1
}
# watch <cell> <seconds> <why>: in the background while the cell runs. Its run is canceled <seconds> in (0: never), with
# where it stood in <cell>.capped; or when no run on the pod has moved for STALL s and the pod then shows a minute of an
# idle GPU (at most 2% busy in six readings) and an idle disk (under 50 MB read and written), the readings in
# <cell>.stuck. A busy GPU or disk is progress: the clock starts again.
IDLE='a=$(awk "{s += \$6 + \$10} END {printf \"%d\", s * 512}" /proc/diskstats); g=0
for i in 1 2 3 4 5 6; do
  u=$(nvidia-smi --query-gpu=utilization.gpu --format=csv,noheader,nounits | sort -n | tail -1)
  [ "${u:-0}" -gt "$g" ] && g=$u
  sleep 10
done
b=$(awk "{s += \$6 + \$10} END {printf \"%d\", s * 512}" /proc/diskstats)
echo "$g $(( (b - a) / 1000000 ))"'
watch() {
  ( seen='' moved=$SECONDS began=$SECONDS why='' busy='' mb='' at='' active=''
    while sleep 60; do
      clock=$(run_clock "$NAME") || continue
      read -r at active <<< "$clock"
      [ "$at" = "$seen" ] || { seen=$at moved=$SECONDS; }
      if [ "$2" != 0 ] && [ $((SECONDS - began)) -ge "$2" ]; then
        why="canceled $((SECONDS - began)) s in ($3), at: $(run_stands "$NAME" | paste -sd';' | cut -c1-160)"; echo "$why" > "$OUT/$1.capped"
      elif [ $((SECONDS - moved)) -ge "$STALL" ] && [ -n "$active" ]; then
        read -r busy mb <<< "$(pod "$IDLE" 120)"
        [[ ${busy:-x} =~ ^[0-9]+$ && ${mb:-x} =~ ^[0-9]+$ ]] || continue
        if [ "$busy" -gt 2 ] || [ "$mb" -ge 50 ]; then moved=$SECONDS; continue; fi
        why="stuck: no progress for $((SECONDS - moved)) s, then a minute with the GPU at most ${busy}% busy and ${mb} MB of disk I/O"
        echo "$why" > "$OUT/$1.stuck"
      fi
      [ -n "$why" ] || continue
      for r in $active; do cozy run cancel "$r" > /dev/null 2>&1; done
      exit 0
    done ) 9>&- & BG=$!
}
ONE=${RUNNER_TEMP:-/tmp}/cap-sweep-one TWIN_S=$(param twin_s 2400)
# measured.py <out> <run> <verdict> <why>: ledger.py rows on stdin, each out with a package benchmark's fields (bench.py
# `references`: function, gpu, gpus, request, cold_ms or warm_ms, ran_on, note) and what a measured memory and time table needs
# beside them: budget, card, lanes, memory.*, outcome.{reason, stood, sweep, why}.
cat > "$ONE.py" << 'PY'
import json, os, sys

out, run, verdict, why = sys.argv[1:5]
card = (json.load(open(os.path.join(out, 'show-capped.json'))).get('status', {}).get('gpus') or [{}])[0]


def walk(v):
    if isinstance(v, dict):
        yield v
        for x in v.values():
            yield from walk(x)
    elif isinstance(v, list):
        for x in v:
            yield from walk(x)


for line in sys.stdin:
    row = json.loads(line)
    name = row['source']['cell']
    rec = json.load(open(os.path.join(out, name + '.json')))
    inp = (rec.get('request') or {}).get('input') or {}
    res = rec.get('result') if isinstance(rec.get('result'), dict) else {}
    ms = [m for m in [rec.get('measurements')] + [c.get('measurements') for c in rec.get('calls') or []] if m]
    rows = [v for m in ms for v in ((m.get('memory') or {}).get('stats') or {}).get('varena') or [] if isinstance(v, dict)]
    stats = [v.get('stats') or {} for v in rows]
    ph, w, st = row.get('phases') or {}, row.get('workload') or {}, row.get('state') or {}
    size = {'refs': len(inp.get('references') or [])}
    if res.get('width') and res.get('height'):
        size['megapixels'] = round(res['width'] * res['height'] / 1e6, 2)
    elif isinstance(inp.get('megapixels'), (int, float)):
        size['megapixels'] = float(inp['megapixels'])
    for key, value in (('frames', res.get('frame_count')), ('steps', ph.get('steps')), ('tokens', inp.get('max_tokens')),
                       ('seconds', (w.get('shape') or {}).get('seconds') or inp.get('duration_s'))):
        if value:
            size[key] = value
    row['function'], row['gpu'], row['gpus'], row['request'] = w.get('function'), rec.get('gpu'), w.get('gpus'), size
    if ph.get('execution_s') is not None and (row.get('outcome') or {}).get('status') == 'completed' and not st.get('memoized'):
        row['cold_ms' if st.get('temperature') == 'cold' else 'warm_ms'] = round(ph['execution_s'] * 1000)
    v = row.get('versions') or {}
    row['ran_on'] = {'cozy': v.get('tensord'), 'runtime': v.get('runtime'), 'tensorfs': v.get('tensorfs')}
    row['note'] = f'pod-session {run} cap-sweep {name}'
    row['budget'] = w.get('cap')
    row['card'] = {k: card.get(k) for k in ('name', 'memory_bytes', 'family', 'driver')}
    row['lanes'] = {str(m.get('slot')): m.get('lane') for x in ms for m in x.get('models') or [] if isinstance(m, dict)}
    mem = row.setdefault('memory', {})
    mem['peak_reserved_gib'] = round(max([(m.get('memory') or {}).get('peak_reserved_bytes') or 0 for m in ms] or [0]) / 2**30, 2)
    mem['torch_peak_gib'] = round(max([x.get('torch_peak') or 0 for x in rows if isinstance(x.get('torch_peak'), (int, float))] or [0]) / 2**30, 2)
    mem['host_peak_gib'] = round(max([s.get('host') or 0 for s in stats if isinstance(s.get('host'), (int, float))] or [0]) / 2**30, 2)
    try:
        mem['card_peak_mib'] = int(open(os.path.join(out, name + '.smi')).read().strip())
    except (OSError, ValueError):
        mem['card_peak_mib'] = None
    allocs = [s.get('working_allocator') or {} for s in stats]
    mem['ooms'] = sum(a.get('ooms') or 0 for a in allocs if isinstance(a, dict))
    mem['reclaim_s'] = round(sum(a.get('reclaim_ns') or 0 for a in allocs if isinstance(a, dict)) / 1e9, 1)
    mem['largest_allocation_bytes'] = max([d[k] for s in stats for d in walk(s) for k in d
                                           if 'largest' in k and isinstance(d[k], (int, float)) and not isinstance(d[k], bool)] or [0]) or None
    reason = rec.get('reason')
    row.setdefault('outcome', {})['reason'] = reason if isinstance(reason, (dict, str)) else None
    row['outcome']['stood'] = rec.get('progress')
    row['outcome']['sweep'], row['outcome']['why'] = verdict, why[:2000]
    # A capped 24 GB card leaves about 4.9 GiB to varena and a real 6 GB card about 4.4 (var-030 5.1): a run that completed
    # with its allocator's high-water above that would not fit the real card.
    row['outcome']['tight'] = verdict == 'completed' and w.get('cap') not in (None, 'off') and mem['torch_peak_gib'] > 4.4
    print(json.dumps(row, sort_keys=True))
PY
: > "$OUT/measured.jsonl"
# publish <name> <verdict> <why>: the run's row into measured.jsonl and, as the cell ends, into the tracker's ledger
# (ledger.py append: benchmarks/<sku>/, by its deploy key; a row there once is never written again), so a failure reads
# from a `git fetch` while the session goes on. ROW is its ledger row.
ROW='{}'
row() {
  ROW=$(python3 -I "$LEDGER" rows "$OUT" --source session:cap-sweep --packages "$PACKAGES" 2> "$OUT/rows.err" | jq -c --arg n "$1" 'select(.source.cell == $n)' | head -1)
  [ -n "$ROW" ] || ROW='{}'
}
publish() {
  [ "$ROW" != '{}' ] || { say "$1: no ledger row of it ($(tail -c 200 "$OUT/rows.err" | tr '\n' ' '))"; return 0; }
  python3 -I "$ONE.py" "$OUT" "${GITHUB_RUN_ID:-local}" "$2" "$3" <<< "$ROW" > "$ONE.row" 2>> "$OUT/rows.err" && [ -s "$ONE.row" ] ||
    { say "$1: its row was not made ($(tail -c 200 "$OUT/rows.err" | tr '\n' ' '))"; return 0; }
  cat "$ONE.row" >> "$OUT/measured.jsonl"
  timeout 240 python3 -I "$LEDGER" append "$ONE.row" >> "$OUT/ledger-cells.log" 2>&1 ||
    say "$1: its row was not pushed ($(tail -c 200 "$OUT/ledger-cells.log" | tr '\n' ' '))"
}
ran=0 completed=0 failed=0 unfinished=0 skipped=0 LOST=''
# run <cell> <name> <budget>: the cell's request once under <budget> (the cap, or `off` for its uncapped twin), as <name>.
run() {
  local k=$1 name=$2 budget=$3 c target most limit left N st smi steps total='' ex='' peak='' memo='' throttled='' stood='' why='' verdict code i sw
  local -a args
  c=$(jq -c --arg k "$k" 'map(select(.name == $k)) | first' <<< "$CELLS")
  mapfile -t args < <(jq -r '.args[]' <<< "$c")
  for i in "${!args[@]}"; do
    args[$i]=$(expand "${args[$i]}")
    # A twin of a cell that completed asks with another seed: the same one is the call memo's to answer.
    if [ "$budget" = off ] && [ "${ENDED[$k]:-}" = completed ]; then
      [[ ${args[$i]} =~ ^seed:=([0-9]+)$ ]] && args[$i]="seed:=$((BASH_REMATCH[1] + 500000))"
      [[ ${args[$i]} =~ \"seed\":([0-9]+) ]] && args[$i]=${args[$i]/"\"seed\":${BASH_REMATCH[1]}"/"\"seed\":$((BASH_REMATCH[1] + 500000))"}
    fi
  done
  target=$(jq -r .target <<< "$c") left=$(( END_AT - $(date +%s) ))
  most=$(jq -r '.max_s // 0' <<< "$c"); limit="its $most s cap"
  [ "$budget" != off ] || { most=$TWIN_S; limit="a twin's $TWIN_S s"; }
  if [ "$most" = 0 ] || [ "$most" -gt $((left - 300)) ]; then most=$((left - 300)); limit="the session's $NEED s"; fi
  rm -f "$OUT/$name.capped" "$OUT/$name.stuck"
  pod 'wc -l < /var/tmp/ab-gpu.log > /var/tmp/cap-sweep.mark' > /dev/null
  watch "$name" "$most" "$limit"
  cd "$PACKAGES/$TREE" || fail "no tree for $name"
  step "$name" run "./$target" "${args[@]}" --on "$NAME"; code=$?
  pkill -P "$BG" 2> /dev/null; kill "$BG" 2> /dev/null; BG=''
  N=$(runno "$name"); [ -n "$N" ] || N=$(current "$name")
  cd "$RUN_PROJECT" || fail "no $RUN_PROJECT"
  [ -z "$N" ] || cozy run watch "$N" --once --json --full > "$OUT/$name.json" 2> /dev/null
  cd "$OUT" || fail "no $OUT"
  st=$(jq -r '.status // "none"' "$OUT/$name.json" 2> /dev/null); st=${st:-none}
  smi=$(pod 'tail -n +"$(cat /var/tmp/cap-sweep.mark)" /var/tmp/ab-gpu.log | awk "\$2 == \"card\" {gsub(\",\", \"\"); if (\$4 + 0 > m) m = \$4 + 0} END {print m + 0}"')
  [[ ${smi:-0} =~ ^[1-9][0-9]*$ ]] && echo "$smi" > "$OUT/$name.smi" || smi=unread
  row "$name"
  steps=$(jq -r '"step 1 \(.phases.step1_s // "-") s, steady \(.phases.steady_s // "-") s x\(.phases.steps // "-")"' <<< "$ROW" 2> /dev/null)
  read -r total ex peak memo throttled stood < <(jq -r '[.measurements, (.calls[]?.measurements)] as $m
    | [(((.settled_at_ms // 0) - (.created_at_ms // 0)) / 1000 | floor), ((.execution_ms // 0) / 1000 | floor),
       ([$m[]? | .memory.driver[]?.peak_bytes] | max // 0 | . / 1073741824 * 100 | floor / 100),
       ((.memo.memoized // false) or ([.calls[]?.memoized] | any)), ([$m[]? | .memory.driver[]?.throttled_share // 0] | max // 0),
       ("\(.progress.stage // "-") \(.progress.completed // 0)/\(.progress.total // 0)" | gsub(" "; "_"))] | @tsv' "$OUT/$name.json" 2> /dev/null)
  if [ "$st" != completed ]; then
    why=$(jq -r '.reason | if type == "object" then "\(.code // "?"): \(.message // "")" else (. // "" | tostring) end' "$OUT/$name.json" 2> /dev/null | tr '\n' ' ')
    [ -n "$why" ] || why=$(tr '\r' '\n' < "$OUT/$name.txt" | grep -m1 -E '^error:' || tail -c 400 "$OUT/$name.txt" | tr '\n' ' ')
    timeout 300 cozy machine logs "$NAME" -n 5000 > "$OUT/$name-tensord.log" 2>&1
    timeout 120 cozy machine logs "$NAME" memory -n 2000 > "$OUT/$name-memory.log" 2>&1
  fi
  verdict=$st
  case $(state "$RID") in ended | ending | closing | closed) LOST="$NAME ended under $name (the guard at \$$CAP, or its provider)" ;; esac
  if [ -n "$LOST" ] && [ "$st" != completed ]; then verdict=POD-LOST; why="$LOST; $why"
  elif [ -s "$OUT/$name.stuck" ]; then verdict=STUCK; why="$(cat "$OUT/$name.stuck"); $why"
  elif [ -s "$OUT/$name.capped" ] && [ "$st" != completed ]; then
    # Still moving at its cap: its step rate and what the stage it stood in would still take.
    verdict=UNFINISHED
    why="$(cat "$OUT/$name.capped"); $(jq -rn --argjson r "$ROW" --arg at "${stood:-}" '($at | capture("(?<a>[0-9]+)/(?<b>[0-9]+)$")) as $p
      | ($r.phases.steady_s // $r.phases.step1_s) as $s | (($p.b | tonumber) - ($p.a | tonumber)) as $n
      | if $s and $n > 0 then "\($n) steps of this stage left at \($s) s each: about \($n * $s | floor) s more" else "no step rate in its record" end' 2> /dev/null); $why"
  fi
  # The pair its executor ran (none in a record that never started one) must be the one the pod was moved to.
  sw=$(jq -r '[.measurements, (.calls[]?.measurements)] | map(select(type == "object") | .software // empty
    | "Runtime \(.runtime // "?") · TensorFS \(.tensorfs // "?")") | unique | join(", ")' "$OUT/$name.json" 2> /dev/null)
  if [ -n "$sw" ] && [ "$sw" != "Runtime $rt · TensorFS $tfs" ]; then
    verdict=VOID why="it ran $sw, not Runtime $rt · TensorFS $tfs; $why"
    LOST="$name ran $sw, not the builds under test (Runtime $rt · TensorFS $tfs)"
  fi
  [ "$budget" = off ] || ENDED[$k]=$st
  ran=$((ran + 1))
  case $verdict in completed) completed=$((completed + 1)) ;; UNFINISHED) unfinished=$((unfinished + 1)) ;; *) failed=$((failed + 1)) ;; esac
  say "CELL $name | $budget | run ${N:-none} | $verdict (exit $code) | total ${total:-?} s, execution ${ex:-?} s | ${steps:-no steps} | peak ${peak:-?} GiB by its record, $smi MiB on the card by nvidia-smi | memoized ${memo:-?} | throttled ${throttled:-?} | ${sw:-no software in its record} | at ${stood:-?} | $(printf '%s' "$why" | head -c 900)"
  publish "$name" "$verdict" "$why"
  if [ "$st" = completed ] && jq -e --arg k "{out:$name|" 'any(.[].args[]; contains($k))' <<< "$CELLS" > /dev/null; then
    (cd "$RUN_PROJECT" && cozy run watch "$N" --once -o "$OUT/$name.out") > /dev/null 2>&1
    MADE[$name]=$(ls "$OUT/$name.out"/* 2> /dev/null | grep -E '[.](png|webp|jpg|mp4)$' | sort | head -1)
  fi
}
for k in "${names[@]}"; do
  c=$(jq -c --arg k "$k" 'map(select(.name == $k)) | first' <<< "$CELLS")
  after=$(jq -r '.after // empty' <<< "$c") unless=$(jq -r '.unless // empty' <<< "$c")
  why=''
  if [ -n "$LOST" ]; then why=$LOST
  elif [ $(( END_AT - $(date +%s) )) -lt 600 ]; then why="the session's $NEED s are used"
  elif [ -n "$after" ] && [ "${ENDED[$after]:-}" != completed ]; then why="$after ${ENDED[$after]:-did not run}"
  elif [ -n "$unless" ] && [ "${ENDED[$unless]:-}" = completed ]; then why="$unless completed"; fi
  if [ -n "$why" ]; then say "CELL $k | NOT RUN: $why"; skipped=$((skipped + 1)); continue; fi
  run "$k" "$k" "$CAPV"
done
# The uncapped twins, on the same pod after one restart: each cell that completed under the cap, unless its `twin` is false
# (an uncapped time of that request on this code is already in the ledger).
twins=()
for k in "${names[@]}"; do
  [ "${ENDED[$k]:-}" = completed ] || continue
  [ "$(jq -r --arg k "$k" 'map(select(.name == $k)) | first | .twin // true' <<< "$CELLS")" = false ] || twins+=("$k")
done
if [ "${#twins[@]}" -gt 0 ] && [ -z "$LOST" ] && [ $(( END_AT - $(date +%s) )) -gt 900 ]; then
  step uncap machine update "$NAME" --gpu-memory-limit off -y || say "lifting the cap: $(tail -c 300 "$OUT/uncap.txt")"
  said=''
  for _ in $(seq 60); do
    cozy machine show "$NAME" --json > "$OUT/show-uncapped.json" 2> /dev/null
    said=$(jq -r '[.status.gpus[]? | .memory_limit_bytes // 0 | tostring] | unique | join(",")' "$OUT/show-uncapped.json" 2> /dev/null)
    [ "$said" = 0 ] && [ "$(jq -r '.state // empty' "$OUT/show-uncapped.json")" = ready ] && break
    sleep 5
  done
  if [ "$said" = 0 ]; then
    gpuwatch
    for k in "${twins[@]}"; do
      why=$LOST; [ -n "$why" ] || [ $(( END_AT - $(date +%s) )) -ge 600 ] || why="the session's $NEED s are used"
      if [ -n "$why" ]; then say "CELL $k-uncapped | NOT RUN: $why"; skipped=$((skipped + 1)); continue; fi
      say "cell $k-uncapped: caps VRAM at off"
      run "$k" "$k-uncapped" off
    done
  else
    say "the cap was not lifted (machine show says ${said:-nothing} bytes): no uncapped twin ran"
  fi
fi
say "SWEEP at $CAPV on $NAME: $ran ran: $completed completed, $failed failed, $unfinished unfinished at their caps; $skipped not run"
say "measured.jsonl: $(wc -l < "$OUT/measured.jsonl") rows"
