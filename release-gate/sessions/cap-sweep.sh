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
#   runtime    the Runtime wheel, https://…#sha256=<hex> (required)
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
#   file       a JSON file in this checkout holding all of these (lib.sh)
# PROOF ONLY (var-030 section 8), on proof/var-030-managed-working:
#   cap        also `off`, with free_gib: the card is held to free_gib physically instead
#   free_gib   suites/bench/vram.py from a root shell holds all of GPU 0 but this many GiB while the capped cells (and red
#              ones) run; it ends before each update's restart (ab.sh `hold`) and for the twins
#   managed    true: the capped cells' TensorFS takes its eager working memory as CUDA managed memory (ab.sh
#              `managed_working`: `dev = true`, `[machine] managed_working` in the pod's config.toml)
#   twin_tensorfs  a second wheel (https…#sha256= or artifact:<run>/<name>): every cell that ran runs again on it, uncapped
#              and unheld, managed per `twin_managed` (false), with its own seed: another build is no call memo's to answer
#   red_tensorfs   a wheel every cell that completed (and each marked `"red": true`, but none marked `"no_red": true`) runs
#              again on, held, unmanaged: what the release does there. A cell marked `"no_twin": true` has no twin.
#   The runtime and tensorfs wheels may be artifact:<run>/<name> too (a runtime also a cozy-runtime dev-build run id), and
#   a cell may name its own `tree`. Rows stay in measured.jsonl: none goes to the ledger,
#   whose cap field cannot say "held physically".
. "$(dirname "$0")/lib.sh"
TREE=$(param tree '') CAPV=$(param cap 6GiB) RW=$(param runtime '') TW=$(param tensorfs '') CELLS=$(param cells '[]')
STALL=$(param stall_s 1800) NEED=$(param need_s 3600)
FREE_GIB=$(param free_gib '') MANAGED=$(param managed false) TW2=$(param twin_tensorfs '') TWM=$(param twin_managed false) TWR=$(param red_tensorfs '')
[[ $CAPV =~ ^([0-9]+[MG]iB|off)$ ]] || fail "cap $CAPV"
[[ -z $FREE_GIB || $FREE_GIB =~ ^[0-9]+([.][0-9]+)?$ ]] || fail "free_gib $FREE_GIB"
[[ $MANAGED =~ ^(true|false)$ && $TWM =~ ^(true|false)$ ]] || fail "managed $MANAGED, twin_managed $TWM"
VRAM=$(cd "$(dirname "$0")/../suites/bench" && pwd)/vram.py
[ -z "$FREE_GIB" ] || [ -f "$VRAM" ] || fail "no suites/bench/vram.py"
[[ $STALL =~ ^[0-9]+$ && $NEED =~ ^[0-9]+$ ]] || fail "stall_s $STALL, need_s $NEED"
[ "$NEED" -le 19800 ] || fail "need_s $NEED: a hosted job ends at 6 h with nothing kept; split the session"
END_AT=$(( $(date +%s) + NEED ))
[ -n "$TREE" ] && [ -d "$PACKAGES/$TREE" ] || fail "no packages checkout: dispatch with -f packages_refs=<name>=<ref> and params.tree=<name>"
for tool in jq python3 ffmpeg; do command -v "$tool" > /dev/null || fail "no $tool"; done
V=$(cozy --version | awk '{print $2}')
# wheel <URL#sha256=hex>: the wheel under its own name (a URL's %2B is a version's +), held to its digest.
wheel() {
  local n f
  if [[ $1 == artifact:*/* || $1 =~ ^[0-9]+$ ]]; then
    f=$(wheel_from "${RUNNER_TEMP:-$OUT}/wheels/${1//[^A-Za-z0-9]/_}" "$1") && [ -f "$f" ] || return 1
    say "wheel ${f##*/} from $1: sha256 $(sha256sum "$f" | cut -c1-64)" > /dev/null; echo "$f"; return 0
  fi
  [[ $1 == https://*'#sha256='* ]] || return 1
  n=$(basename "${1%%#*}"); f=${RUNNER_TEMP:-$OUT}/wheels/$(printf '%b' "${n//%/\\x}"); mkdir -p "${f%/*}"
  curl -fsSL --retry 5 --retry-all-errors "${1%%#*}" -o "$f" && [ "$(sha256sum "$f" | cut -c1-64)" = "${1##*sha256=}" ] && echo "$f"
}
RWF=$(wheel "$RW") || fail "runtime is not a wheel at https://…#sha256=<hex> (or artifact:<run>/<name>) that hashes to it"
TWF=$(wheel "$TW") || fail "tensorfs is not a wheel at https://…#sha256=<hex> (or artifact:<run>/<name>) that hashes to it"
TWF2='' TWFR=''
[ -z "$TW2" ] || TWF2=$(wheel "$TW2") || fail "twin_tensorfs is not a wheel: $TW2"
[ -z "$TWR" ] || TWFR=$(wheel "$TWR") || fail "red_tensorfs is not a wheel: $TWR"
[ -z "$TWF2" ] || [ "${TWF2##*/}" != "${TWF##*/}" ] || fail "twin_tensorfs is the capped cells' own build: the call memo would answer its twins"
say "candidate: cozy $V · ${RWF##*/} · ${TWF##*/} · packages $(git -C "$PACKAGES/$TREE" rev-parse --short HEAD) · cap $CAPV${FREE_GIB:+ · GPU 0 held to $FREE_GIB GiB} · managed working memory $MANAGED"
for f in "$TWF" ${TWF2:+"$TWF2"} ${TWFR:+"$TWFR"}; do say "tensorfs ${f##*/} sha256 $(sha256sum "$f" | cut -c1-64)"; done
[ -z "$TWF2" ] || say "twins: ${TWF2##*/}, uncapped and unheld, managed working memory $TWM"
[ -z "$TWFR" ] || say "red cells: ${TWFR##*/}, held, unmanaged"

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
  t=$(jq -r .target <<< "$c") tr=$(jq -r --arg d "$TREE" '.tree // $d' <<< "$c")
  [ -f "$PACKAGES/$tr/${t%/*}/pyproject.toml" ] || fail "cell $k: no package ${t%/*} in $tr"
  for other in $(jq -r '.after // empty, .unless // empty' <<< "$c"); do
    [[ " ${names[*]} " == *" $other "* ]] || fail "cell $k waits on $other, which is no cell"
  done
  while IFS= read -r a; do
    for f in $(expand "$a" | grep -oE "($M|$FX|$REF)/[A-Za-z0-9._-]+"); do [ -f "$f" ] || fail "cell $k: no $f"; done
  done < <(jq -r '.args[]' <<< "$c")
  # ledger.py reads these two lines: the cell's package code and its cap.
  say "cell $k: $(git -C "$PACKAGES/$tr" rev-parse --short HEAD) ./$t $(jq -r '.args | join(" ")' <<< "$c" | cut -c1-600)"
  say "cell $k: caps VRAM at $CAPV"
  kernels "$PACKAGES/$tr/${t%/*}"
done
printf 'run 5 · sync local/x (source changed)\nrun 6 · local/x/f\nrun      6\n' > "$OUT/probe-cell.txt"
current() { awk '$1=="run" && $3=="·" && $4!="sync"{n=$2} END{print n}' "$OUT/$1.txt" 2>/dev/null; }
[ "$(runno probe-cell)" = 6 ] && [ "$(current probe-cell)" = 6 ] || fail "parsers"
[ -f ~/.ssh/id_ed25519.pub ] || ssh-keygen -q -t ed25519 -N '' -f ~/.ssh/id_ed25519 || fail "ssh-keygen"
RENT_ARGS=--ssh
quote "$NEED"
rent
# PROOF ONLY (var-030 section 8): paging is judged on a host with room for it. `min_cores` and `min_ram_gib` (the smaller of
# MemTotal and the container's memory.max): a host below them is ended by name and another rented, `host_tries` in all.
# `cozy machine rent` takes no host size, so the session looks after the boot.
MINC=$(param min_cores 0) MINR=$(param min_ram_gib 0) TRIES=$(param host_tries 3) HOST_SIZE='? ?'
for try in $(seq "$TRIES"); do
  hw=''
  for _ in 1 2 3 4 5 6; do
    hw=$(timeout 90 cozy machine ssh "$NAME" 'echo "$(nproc) $(awk "/^MemTotal/ {printf \"%d\", \$2 / 1048576}" /proc/meminfo) $(cat /sys/fs/cgroup/memory.max 2> /dev/null || echo max)"' < /dev/null 2> /dev/null |
      grep -E '^[0-9]+ [0-9]+ ' | tail -1)
    [ -z "$hw" ] || break; sleep 10
  done
  read -r cores ram cg <<< "${hw:-0 0 max}"
  [[ $cg =~ ^[0-9]+$ ]] && [ $((cg >> 30)) -lt "$ram" ] && ram=$((cg >> 30))
  HOST_SIZE="$cores $ram"
  say "host $NAME: $cores cores, $ram GiB of RAM for the container (asked at least $MINC cores, $MINR GiB)"
  if [ "$cores" -ge "$MINC" ] && [ "$ram" -ge "$MINR" ]; then break; fi
  [ "$try" -lt "$TRIES" ] || fail "no host with $MINC cores and $MINR GiB in $TRIES rentals"
  timeout 120 cozy machine end "$NAME" -y > "$OUT/end-small-$try.txt" 2>&1
  for _ in $(seq 36); do gone "$RID" && break; sleep 5; done
  gone "$RID" || fail "$NAME, too small, did not end"
  say "ended $NAME (too small) and verified gone; renting again"
  order; ready
done
jq -e '.status.gpus | length == 1' "$OUT/show.json" > /dev/null || fail "not one GPU"
# A dev home on an RC build moves its rental to the RC's builds once it's ready: nothing starts before that ends.
cozy machine show "$NAME" --json > "$OUT/show-rc.json" 2> /dev/null
if [ -n "$(jq -r '.rc.state // empty' "$OUT/show-rc.json")" ]; then
  for _ in $(seq 180); do
    [ "$(jq -r '.rc.state // empty' "$OUT/show-rc.json")" = current ] && [ "$(jq -r '.state // empty' "$OUT/show-rc.json")" = ready ] && break
    sleep 5; cozy machine show "$NAME" --json > "$OUT/show-rc.json" 2> /dev/null
  done
fi
# managed_working <true|false>: as ab.sh's, read by the executors after the next restart.
managed_working() {
  local script
  script=$(base64 -w0 <<'POD'
set -e
f=~/.cozy/config.toml
mkdir -p ~/.cozy; touch "$f"
grep -q '^\[machine\]' "$f" || printf '\n[machine]\n' >> "$f"
{ echo 'dev = true'
  grep -v -e '^dev *=' -e '^managed_working *=' -e '^managed_rebind *=' "$f" |
    awk -v a="managed_working = $1" -v b="managed_rebind = true" '{print} /^\[machine\]$/ {print a; print b}'
} > "$f.new"
mv "$f.new" "$f"
grep -x -e 'dev = true' -e "managed_working = $1" -e "managed_rebind = true" "$f" | paste -sd, -
POD
)
  local try
  for try in 1 2 3 4 5 6; do
    timeout 120 cozy machine ssh "$NAME" -- "echo $script | base64 -d | sh -s -- $1" < /dev/null 2> /dev/null |
      grep -qx "dev = true,managed_working = $1,managed_rebind = true" && { say "the pod's config.toml: dev = true, managed_working = $1"; return 0; }
    sleep 10
  done
  fail "the pod's config.toml did not take managed_working $1"
}
# hold <on|off>: ab.sh's holder: all of GPU 0 but FREE_GIB, from a root shell; TensorD takes a GPU another program holds
# memory on out of service when it starts, so it ends before every update.
hold() {
  local script try held='' free='' code
  [ -n "$FREE_GIB" ] || return 0
  if [ "$1" = off ]; then
    script='pgrep -f /var/tmp/ab-vram.py | xargs -r kill; rm -f /var/tmp/ab-vram.ready; sleep 2; pgrep -f /var/tmp/ab-vram.py > /dev/null && echo held || echo none'
  else
    script=$(cat <<EOS
[ -s /var/tmp/ab-vram.ready ] && kill -0 "\$(cut -d' ' -f1 /var/tmp/ab-vram.ready)" 2> /dev/null && { cat /var/tmp/ab-vram.ready; exit 0; }
if ! pgrep -f /var/tmp/ab-vram.py > /dev/null; then
  echo $(base64 -w0 "$VRAM") | base64 -d > /var/tmp/ab-vram.py && rm -f /var/tmp/ab-vram.ready
  setsid nohup python3 /var/tmp/ab-vram.py $(awk -v g="$FREE_GIB" 'BEGIN { printf "%d", g * 1073741824 }') /var/tmp/ab-vram.ready > /var/tmp/ab-vram.out 2>&1 < /dev/null &
fi
i=0; while [ ! -s /var/tmp/ab-vram.ready ] && [ \$i -lt 240 ]; do sleep 0.5; i=\$((i + 1)); done
cat /var/tmp/ab-vram.ready 2> /dev/null || tail -c 300 /var/tmp/ab-vram.out
EOS
)
  fi
  for try in 1 2 3 4 5 6; do
    cozy machine ssh "$NAME" "echo $(base64 -w0 <<< "$script") | base64 -d | sh" > "$OUT/hold-$1.txt" 2>&1; code=$?
    if [ "$1" = off ]; then grep -qx none "$OUT/hold-$1.txt" && { say "holder ended"; return 0; }; else
      read -r _ held free _ < <(tail -1 "$OUT/hold-$1.txt"); [[ ${held:-x} =~ ^[0-9]+$ ]] && break
    fi
    say "holder $1, try $try: exit $code $(head -c 200 "$OUT/hold-$1.txt")"; sleep 10
  done
  [ "$1" = on ] || fail "the holder did not end"
  [[ ${held:-x} =~ ^[0-9]+$ ]] || fail "no holder: $(head -c 300 "$OUT/hold-$1.txt")"
  say "holder $((held >> 20)) MiB held, $((free >> 20)) MiB left free (nvidia-smi: $(timeout 30 cozy machine ssh "$NAME" 'nvidia-smi --query-gpu=memory.used,memory.free,memory.total --format=csv,noheader' 2> /dev/null | tail -1))"
}
managed_working "$MANAGED"
step update machine update "$NAME" --tensord "$(command -v cozy)" --runtime "$RWF" --tensorfs "$TWF" --gpu-memory-limit "$CAPV" -y ||
  fail "update: $(tail -c 300 "$OUT/update.txt")"
WANT=0
[ "$CAPV" = off ] || WANT=$(( ${CAPV%[MG]iB} << 20 )); [[ $CAPV != *GiB ]] || WANT=$(( ${CAPV%GiB} << 30 ))
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
hold on
# The pod's own logs leave with it.
before_end() {
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
}
ran=0 completed=0 failed=0 unfinished=0 skipped=0 LOST=''
declare -A RUNS=() DONE=()
# run <cell> <name> <budget>: the cell's request once under <budget> (the cap, or `off` for its uncapped twin), as <name>.
run() {
  local k=$1 name=$2 budget=$3 c target most limit left N st smi steps total='' ex='' peak='' memo='' throttled='' stood='' why='' verdict code i
  local mem procmib hostgrow hostpeak hostsys h2d d2h fills ceil mg
  local -a args
  c=$(jq -c --arg k "$k" 'map(select(.name == $k)) | first' <<< "$CELLS")
  mapfile -t args < <(jq -r '.args[]' <<< "$c")
  for i in "${!args[@]}"; do
    args[$i]=$(expand "${args[$i]}")
    # A twin of a cell that completed asks with another seed: the same one is the call memo's to answer.
    if [ "$name" = "$k-uncapped" ] && [ -z "$TWF2" ] && [ "${ENDED[$k]:-}" = completed ]; then
      [[ ${args[$i]} =~ ^seed:=([0-9]+)$ ]] && args[$i]="seed:=$((BASH_REMATCH[1] + 500000))"
      [[ ${args[$i]} =~ \"seed\":([0-9]+) ]] && args[$i]=${args[$i]/"\"seed\":${BASH_REMATCH[1]}"/"\"seed\":$((BASH_REMATCH[1] + 500000))"}
    fi
  done
  target=$(jq -r .target <<< "$c") left=$(( END_AT - $(date +%s) ))
  most=$(jq -r '.max_s // 0' <<< "$c"); limit="its $most s cap"
  [ "$name" != "$k-uncapped" ] || { most=$TWIN_S; limit="a twin's $TWIN_S s"; }
  if [ "$most" = 0 ] || [ "$most" -gt $((left - 300)) ]; then most=$((left - 300)); limit="the session's $NEED s"; fi
  rm -f "$OUT/$name.capped" "$OUT/$name.stuck"
  pod 'wc -l < /var/tmp/ab-gpu.log > /var/tmp/cap-sweep.mark' > /dev/null
  watch "$name" "$most" "$limit"
  cd "$PACKAGES/$(jq -r --arg d "$TREE" '.tree // $d' <<< "$c")" || fail "no tree for $name"
  step "$name" run "./$target" "${args[@]}" --on "$NAME"; code=$?
  pkill -P "$BG" 2> /dev/null; kill "$BG" 2> /dev/null; BG=''
  N=$(runno "$name"); [ -n "$N" ] || N=$(current "$name")
  cd "$RUN_PROJECT" || fail "no $RUN_PROJECT"
  [ -z "$N" ] || cozy run watch "$N" --once --json --full > "$OUT/$name.json" 2> /dev/null
  cd "$OUT" || fail "no $OUT"
  st=$(jq -r '.status // "none"' "$OUT/$name.json" 2> /dev/null); st=${st:-none}
  smi=$(pod 'tail -n +"$(cat /var/tmp/cap-sweep.mark)" /var/tmp/ab-gpu.log | awk "\$2 == \"card\" {gsub(\",\", \"\"); if (\$4 + 0 > m) m = \$4 + 0} END {print m + 0}"')
  [[ ${smi:-0} =~ ^[1-9][0-9]*$ ]] && echo "$smi" > "$OUT/$name.smi" || smi=unread
  # The cell's own process on the card (nvidia-smi's per-process MiB, the holder left out), and host memory that cannot be
  # given back: the container's growth over the cell, its peak, and the machine's peak (MiB).
  mem=$(pod 'tail -n +"$(cat /var/tmp/cap-sweep.mark)" /var/tmp/ab-gpu.log | awk "\$2 == \"pid\" && \$0 !~ /ab-vram/ {if (\$4 + 0 > p) p = \$4 + 0} \$2 == \"host\" {if (b == \"\") b = \$3; if (\$3 + 0 > h) h = \$3 + 0; if (\$4 + 0 > n) n = \$4 + 0; split(\$1, c, \":\"); s = c[1] * 3600 + c[2] * 60 + c[3]; if (l != \"\" && s > l && s - l < 30) {r += \$5 * (s - l); x += \$6 * (s - l)}; l = s} END {printf \"%d %d %d %d %.1f %.1f\", p, (h - b) / 1048576, h / 1048576, n / 1048576, r / 1000, x / 1000}"')
  read -r procmib hostgrow hostpeak hostsys h2d d2h <<< "${mem:-? ? ? ? ? ?}"
  echo "$procmib $hostgrow $hostpeak $hostsys $h2d $d2h" > "$OUT/$name.mem"
  mg=$(jq -r '[.measurements, (.calls[]?.measurements)] | map(select(. != null) | .memory.stats.varena[]?.stats.working_allocator // empty)
    | if length == 0 then "none" else "on \(map(.managed_on // 0) | max), peak \(map(.managed_peak // 0) | max / 1048576 | floor) MiB, past the card \(map(.managed_over_peak // 0) | max / 1048576 | floor) MiB, forced \(map(.managed_forced // 0) | add), host refusals \(map(.managed_host_refusals // 0) | add), rebinds \(map(.managed_rebinds // 0) | add) (\(map(.managed_rebound_bytes // 0) | add / 1073741824 | floor) GiB), discards \(map(.managed_discards // 0) | add) (\(map(.managed_discarded_bytes // 0) | add / 1073741824 | floor) GiB), ooms \(map(.ooms // 0) | add)" end' "$OUT/$name.json" 2> /dev/null)
  ceil=$(jq -r '[.measurements, (.calls[]?.measurements)] | map(select(. != null) | .memory.stats.varena[]?.ceiling.h2d_gbps // empty) | max // "?"' "$OUT/$name.json" 2> /dev/null)
  fills=$(jq -r '[.measurements, (.calls[]?.measurements)] | map(select(. != null) | .memory.stats.varena[]?.stats.moved // empty | to_entries[] | .value.bytes // 0) | add // 0 | . / 1e9 | . * 10 | floor / 10' "$OUT/$name.json" 2> /dev/null)
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
  [ "$name" != "$k" ] || ENDED[$k]=$st
  RUNS[$name]=${N:-}; DONE[$name]=$st
  ran=$((ran + 1))
  case $verdict in completed) completed=$((completed + 1)) ;; UNFINISHED) unfinished=$((unfinished + 1)) ;; *) failed=$((failed + 1)) ;; esac
  say "CELL $name | $budget | run ${N:-none} | $verdict (exit $code) | total ${total:-?} s, execution ${ex:-?} s | ${steps:-no steps} | peak ${peak:-?} GiB by its record, $smi MiB on the card by nvidia-smi | host ${HOST_SIZE% *} cores ${HOST_SIZE#* } GiB | process ${procmib:-?} MiB on the card | host +${hostgrow:-?} MiB over the cell, peak ${hostpeak:-?} MiB in the container, ${hostsys:-?} MiB on the machine | link ${h2d:-?} GB in, ${d2h:-?} GB out (weight fills ${fills:-?} GB; the link measured ${ceil:-?} GB/s in) | managed ${mg:-?} | memoized ${memo:-?} | throttled ${throttled:-?} | at ${stood:-?} | $(printf '%s' "$why" | head -c 900)"
  # The switch must have taken: a managed cell whose record says it was off measures nothing this session is for.
  if [ "$name" = "$k" ] && [ "$MANAGED" = true ] && [[ ${mg:-none} == "on 0,"* ]]; then fail "$name ran with managed working memory off: $mg"; fi
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
# rebuild <wheel> <managed> <cap> <label>: the holder ends, the config takes the switch, the machine updates to <wheel> under
# <cap>, and is checked to run it.
rebuild() {
  local said='' tfs
  hold off
  managed_working "$2"
  step "update-$4" machine update "$NAME" --tensorfs "$1" --gpu-memory-limit "$3" -y || { say "$4: the update: $(tail -c 300 "$OUT/update-$4.txt")"; return 1; }
  local want=0; [ "$3" = off ] || want=$WANT
  for _ in $(seq 60); do
    cozy machine show "$NAME" --json > "$OUT/show-$4.json" 2> /dev/null
    said=$(jq -r '[.status.gpus[]? | .memory_limit_bytes // 0 | tostring] | unique | join(",")' "$OUT/show-$4.json" 2> /dev/null)
    [ "$said" = "$want" ] && [ "$(jq -r '.state // empty' "$OUT/show-$4.json")" = ready ] && break
    sleep 5
  done
  tfs=$(jq -r '.status.tensorfs // empty' "$OUT/show-$4.json")
  [ "$said" = "$want" ] && [[ ${1##*/} == *"-${tfs}-"* ]] || { say "$4: machine show says ${said:-nothing} bytes and TensorFS ${tfs:-none}, not ${1##*/}"; return 1; }
  say "$4: TensorFS $tfs, cap $3, managed working memory $2"
  gpuwatch
}
# The red arm (red_tensorfs, the release's TensorFS): every cell that completed held, and each marked `red`, held again with
# no managed working memory, so which cells the switch turns from failed to completed reads plainly.
reds=()
if [ -n "$TWFR" ]; then
  for k in "${names[@]}"; do
    [ -n "${ENDED[$k]:-}" ] || continue
    [ "$(jq -r --arg k "$k" 'map(select(.name == $k)) | first | .no_red // false' <<< "$CELLS")" != true ] || continue
    if [ "${ENDED[$k]}" = completed ] || [ "$(jq -r --arg k "$k" 'map(select(.name == $k)) | first | .red // false' <<< "$CELLS")" = true ]; then reds+=("$k"); fi
  done
fi
if [ "${#reds[@]}" -gt 0 ] && [ -z "$LOST" ] && [ $(( END_AT - $(date +%s) )) -gt 900 ] && rebuild "$TWFR" false "$CAPV" red; then
  hold on
  for k in "${reds[@]}"; do
    why=$LOST; [ -n "$why" ] || [ $(( END_AT - $(date +%s) )) -ge 600 ] || why="the session's $NEED s are used"
    if [ -n "$why" ]; then say "CELL $k-off | NOT RUN: $why"; skipped=$((skipped + 1)); continue; fi
    say "cell $k-off: caps VRAM at $CAPV"
    run "$k" "$k-off" "$CAPV"
  done
fi
# The uncapped twins, on the same pod after one restart. With twin_tensorfs: every cell that ran, on that build, unheld, with
# its own seed, and its output set beside the capped run's. Else each cell that failed under the cap (a capacity failure passes
# here and shows what the request wants), and each marked `twin` (no uncapped time of its function is recorded).
twins=()
for k in "${names[@]}"; do
  [ -n "${ENDED[$k]:-}" ] || continue
  [ "$(jq -r --arg k "$k" 'map(select(.name == $k)) | first | .no_twin // false' <<< "$CELLS")" != true ] || continue
  if [ -n "$TWF2" ] || [ "${ENDED[$k]}" = failed ] || [ "$(jq -r --arg k "$k" 'map(select(.name == $k)) | first | .twin // false' <<< "$CELLS")" = true ]; then twins+=("$k"); fi
done
if [ "${#twins[@]}" -gt 0 ] && [ -z "$LOST" ] && [ $(( END_AT - $(date +%s) )) -gt 900 ] &&
  if [ -n "$TWF2" ]; then rebuild "$TWF2" "$TWM" off twin; else hold off; rebuild "$TWF" "$MANAGED" off twin; fi; then
  for k in "${twins[@]}"; do
    why=$LOST; [ -n "$why" ] || [ $(( END_AT - $(date +%s) )) -ge 600 ] || why="the session's $NEED s are used"
    if [ -n "$why" ]; then say "CELL $k-uncapped | NOT RUN: $why"; skipped=$((skipped + 1)); continue; fi
    say "cell $k-uncapped: caps VRAM at off"
    run "$k" "$k-uncapped" off
  done
fi
# same <cell> <other>: the two runs' results, every digest (video, picture, latents); where they differ, each stream's PSNR.
same() {
  local a b d
  a=$(jq -c '[.result | .. | objects | select(has("digest")) | "\(.kind // "?"):\(.digest)"] | sort' "$OUT/$1.json" 2> /dev/null)
  b=$(jq -c '[.result | .. | objects | select(has("digest")) | "\(.kind // "?"):\(.digest)"] | sort' "$OUT/$2.json" 2> /dev/null)
  if [ -n "$a" ] && [ "$a" != '[]' ] && [ "$a" = "$b" ]; then say "SAME $1 $2 | every digest equal: $a"; return 0; fi
  for x in "$1" "$2"; do
    [ -d "$OUT/$x.out" ] || (cd "$RUN_PROJECT" && cozy run watch "${RUNS[$x]}" --once -o "$OUT/$x.out") > /dev/null 2>&1
  done
  local fa fb
  fa=$(ls "$OUT/$1.out"/* 2> /dev/null | grep -E '[.](png|webp|jpg|mp4)$' | sort | head -1)
  fb=$(ls "$OUT/$2.out"/* 2> /dev/null | grep -E '[.](png|webp|jpg|mp4)$' | sort | head -1)
  d=''
  if [ -n "$fa" ] && [ -n "$fb" ]; then
    nice -n 19 ffmpeg -nostdin -v error -i "$fa" -i "$fb" -lavfi "[0:v][1:v]psnr=stats_file=$OUT/psnr-$1-$2.log" -f null - 2> /dev/null
    d=$(awk '{for (i = 1; i <= NF; i++) if ($i ~ /^psnr_avg:/) {v = substr($i, 10); n++; s += (v == "inf" ? 100 : v); if (min == "" || (v != "inf" && v + 0 < min)) {min = v + 0; at = n}}}
      END {if (n) printf "mean %.2f dB over %d frames, worst %s dB at frame %d", s / n, n, (min == "" ? "inf" : min), at}' "$OUT/psnr-$1-$2.log" 2> /dev/null)
  fi
  say "DIFFERS $1 $2 | $a against $b | video/picture PSNR (inf as 100) ${d:-unread}"
}
for k in "${names[@]}"; do
  [ "${DONE[$k]:-}" = completed ] || continue
  [ "${DONE[$k-uncapped]:-}" != completed ] || same "$k" "$k-uncapped"
  [ "${DONE[$k-off]:-}" != completed ] || same "$k" "$k-off"
done
# compare: [[a, b], ...] of any two runs by name (a cell, `<cell>-uncapped`, `<cell>-off`), each pair that both completed.
while read -r a b; do
  [ -n "$a" ] || continue
  if [ "${DONE[$a]:-}" = completed ] && [ "${DONE[$b]:-}" = completed ]; then same "$a" "$b"; else say "COMPARE $a $b | not both completed (${DONE[$a]:-none}, ${DONE[$b]:-none})"; fi
done < <(param compare '[]' | jq -r '.[] | "\(.[0]) \(.[1])"')
say "SWEEP at $CAPV on $NAME: $ran ran: $completed completed, $failed failed, $unfinished unfinished at their caps; $skipped not run"
say "measured.jsonl: $(wc -l < "$OUT/measured.jsonl") rows"
