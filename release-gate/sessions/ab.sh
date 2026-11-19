# ab: one rental, a list of cells (each one run of a package function from a packages checkout, run in order), and
# pairs of cells compared for identical output: a refactor's proof is the same request giving the same bytes (xs-101).
# While cells run, `machine show` and the current run's `run watch --once` are timed every 15 s (probe.tsv): a busy
# machine keeps answering and is never "unreachable" (cl-671). Each minute the pod's TCP listeners are kept with the cell
# running (listeners.log), and the summary names those beyond loopback per cell: a GPU group opens none (xs-196).
# Arms. Each cell with an `update` starts an arm.
# - The pod's derived state goes first (lib.sh `derived_state`: LoRA bakes, memos, learned peaks, kernels, page cache),
#   then the update restarts the machine. No arm reads another's: in 44006261838 the candidate read the 0.25.0 arm's
#   bakes and looked 9x faster cold. A settle after the restart (what the ended executors still wrote, 44038983465) must
#   leave no derived page, memo or peak, or the session
#   fails.
# - In an arm a cell is `cold`, its function's first request since the arm began, or `warm`, a later one. A cell marked
#   `"warmup": true` is the arm's own warm-up and is never compared. Its first call still serves what has not compiled
#   yet (the LoRA per step until the bake kernel is ready, a fused block eager until it is built).
# - states.tsv gives each cell's arm, temperature, LoRA bake blocks, derived bytes filled, persisted pages, whether the
#   call memo answered it, and its seconds. pairs.tsv lists each compared pair, cold and warm apart.
# - A compared pair must be the same temperature, neither memoized, a warm-up nor fetching its weights, and a cold cell
#   must have made the derived pages it read. Else the session fails. Weights stay between arms, so give the session a
#   first arm that runs each function once: the arm after it starts cold with every weight on disk.
# - Two builds compared run in both orders (A B B A), checked before renting, unless params say `"order": "one-way"`
#   (an output-only proof).
# - TensorD's call memo answers a memoizable request it has answered before on the same Runtime and TensorFS, whatever the
#   memory cap or TensorD (call_memo.rs; no reset reaches it): a cell asking what an earlier one asked of the same pair takes
#   the next seed (each `seed:=N` + the times it was asked), a compared pair asking the same of the same pair is refused
#   before renting, and any cell the memo still answered fails the session: its seconds are no measure.
# Each cell also keeps the pod's model listing before and after it (<cell>-models-before/after.json), its run's
# has/needs/extras, and the pod's TensorD log after it (<cell>-tensord.log); tensord-now.log is that log as it stood a minute ago.
# params:
#   cells      [{"name", "tree" (a packages_refs name; absent, `target` is a published `<package>/<function>[@release]`,
#              run from the packages directory: an unchanged release on a candidate Runtime), "target"
#              ("<package>/<function>"), "args" ([inputs and flags]),
#              "update" (optional: {"runtime", "tensorfs"}, each a wheel URL, `artifact:<run id>/<name>` of a
#              cozy-creator run (a dev-build's tensorfs-wheel-<sha>) or, for runtime, a cozy-runtime dev-build
#              run id, "tensord": true (this session's cozy as the pod's TensorD: a dev build's, `cozy` a run id,
#              which the open RC's Runtime needs, `rc_runtime_needs_rc_cozy`), and "gpu_memory_limit" (`10GiB`,
#              `off`: the development VRAM cap); the pod's `machine update` before this cell, so cells before and
#              after compare two pairs; it starts an arm: see Arms)}];
#              "warmup" (optional: true, the arm's own warm-up: run, never compared);
#              "keep_state" (optional, on an update cell: true, its arm keeps the derived state before it; such an arm is
#              compared only with another that keeps it);
#              "wait_s" (optional: seconds the machine is left idle before the cell); "until_idle" (optional:
#              {"region", "step_s", "max"}: while the cell's record shows the idle queue hadn't reached the region before it,
#              wait step_s more and run it again with the next seed, as <cell>-w<i>, up to max);
#              "expect" (optional: the status the cell must end in, `completed` by default; `failed` for a refusal it proves);
#              "uninstall" (optional: [org/name, …] removed from the pod first with `cozy model uninstall`, or `"*"` for every
#              model and LoRA it holds, so the cell's run starts cold: a before/after pair each fetches its own);
#              "download" (optional: [[model ref, `cozy model install` flags…], …] made present on the pod first, so
#              the cell's run starts with its weights on disk, as a machine whose package was installed earlier does);
#              in args, `{ref}` is release-gate/ref, `{clip}` a 4 s 640x360 24 fps clip of its seed.jpg panned (made on
#              the runner, for a video tool's input), `{long}` its seed.jpg eight times across, 8192x1024, `{clip5}` and
#              `{clip15}` 1344x768 at 24 fps with a tone (cap-sweep's) and
#              `{out:<cell>}` an earlier cell's image or video;
#              two failed cells stop the session
#              "floor_mb_s", "floor_after_s" (optional: this cell's slow-host floor, lib.sh `step`; the session's by default);
#              "max_s" (optional: its run is canceled this long in and said UNFINISHED, not failed; a compared pair
#              with an UNFINISHED cell compares nothing);
#              "host_free_gib" (optional: for this cell a root shell's program holds the pod's host RAM but this many GiB
#              free, as TensorD reads free (the tightest of MemAvailable and each cgroup limit on its path), as a computer
#              whose other programs hold its memory: a short host tier, cr-305 (see `ram`); it ends after the cell, which
#              fails if any of it ended before; a host with over 256 GiB more than that free is refused);
#   compare    [["A", "B"], …]: the result digests, else the decoded frames of every stream, and the video's PSNR; and
#              their seconds, cold or warm (pairs.tsv)
#   order      optional: `abba` (default: each two builds compared run in both orders) or `one-way`
#   need_s     seconds the cap must buy (3600), at most 19800: a hosted job ends at 6 h with nothing kept
#   max_failures  optional: cells that may fail before the session ends (2)
#   gpus       optional: the pod's GPU count (1); a cell's `--gpus=<n>` arg runs it on that many
#   file       optional: a JSON file in this checkout (from its root) holding all of these, for a matrix past the
#              dispatch input's 65,535 characters
#   free_gib   optional: before the first cell another program (suites/bench/vram.py, raw driver allocations by a
#              root shell over ssh) takes all of GPU 0 but this many GiB and holds it to the end, a card as full as
#              the 0.25.0 gate's RTX 5090 kazune was (about 3 GiB outside cozy; cr-293)
. "$(dirname "$0")/lib.sh"
CELLS=$(param cells '[]') COMPARE=$(param compare '[]') FREE_GIB=$(param free_gib '') GPUS=$(param gpus 1) MOST=$(param max_failures 2)
[[ $MOST =~ ^[1-9][0-9]*$ ]] || fail "max_failures $MOST"
[[ -z $FREE_GIB || $FREE_GIB =~ ^[0-9]+([.][0-9]+)?$ ]] || fail "free_gib $FREE_GIB"
[[ $GPUS =~ ^[1-8]$ ]] || fail "gpus $GPUS"
FLOOR0=$FLOOR_MBS AFTER0=$FLOOR_AFTER
# Absolute, before any cd: the cells run from $OUT, where a path from $0 (relative) names nothing.
SUITE=$(cd "$(dirname "$0")/../suites/bench" && pwd) VRAM=$SUITE/vram.py
[ -z "$FREE_GIB" ] || [ -f "$VRAM" ] || fail "no suites/bench/vram.py"
# ssh for the holder and for what holds the GPU when a cell fails (`gpu`)
[ -f ~/.ssh/id_ed25519.pub ] || ssh-keygen -q -t ed25519 -N '' -f ~/.ssh/id_ed25519 || fail "ssh-keygen"
RENT_ARGS=--ssh; [ "$GPUS" = 1 ] || RENT_ARGS="--ssh --gpus $GPUS"
command -v ffmpeg > /dev/null || fail "no ffmpeg (pod-session.yaml installs it from the tool cache)"
CLIP=$OUT/clip.mp4
if jq -e 'any(.[].args[]?; contains("{clip}"))' <<<"$CELLS" > /dev/null; then
  ffmpeg -v error -y -loop 1 -i "$REF/seed.jpg" -vf "scale=1280:-2,crop=640:360:'(iw-640)*t/4':'(ih-360)/2',fps=24,format=yuv420p" \
    -t 4 -c:v libx264 -crf 18 "$CLIP" || fail "the clip"
fi
# {digits}: cap-sweep's 65,000 characters of digit groups (a long prompt).
DIGITS=$(awk 'BEGIN { x = 20261118; for (i = 0; i < 16250; i++) { x = (x * 48271) % 2147483647; printf "%03d ", x % 1000 } }')
# {pic1}..{pic16} and {depth}: cap-sweep's 2048x2048 pictures and depth map, made here when a cell names one (var-030).
PICS=$OUT/media
if jq -e 'any(.[].args[]?; test("\\{pic[0-9]+\\}|\\{depth\\}"))' <<<"$CELLS" > /dev/null; then
  mkdir -p "$PICS"; FX=$(cd "$(dirname "$0")/../suites/bench/fixtures" && pwd); SRC=("$REF/seed.jpg" "$FX/lighthouse.jpg" "$FX/teapot.jpg")
  for i in $(seq 16); do
    ffmpeg -nostdin -v error -y -i "${SRC[$((i % 3))]}" -vf "hue=h=$((i * 22)),scale=2048:2048:flags=lanczos" -q:v 2 "$PICS/pic$i.jpg" || fail "picture $i"
  done
  ffmpeg -nostdin -v error -y -i "$REF/seed.jpg" -vf "format=gray,gblur=sigma=8" "$PICS/depth.png" || fail "the depth map"
fi
LONG=$OUT/long.jpg
if jq -e 'any(.[].args[]?; contains("{long}"))' <<<"$CELLS" > /dev/null; then
  ffmpeg -v error -y -loop 1 -i "$REF/seed.jpg" -vf "scale=1024:1024,tile=8x1" -frames:v 1 -q:v 2 "$LONG" || fail "the long picture"
fi
for n in 5 15; do
  jq -e --arg c "{clip$n}" 'any(.[].args[]?; contains($c))' <<<"$CELLS" > /dev/null || continue
  ffmpeg -nostdin -v error -y -loop 1 -i "$REF/seed.jpg" -f lavfi -i "sine=frequency=440:duration=$n" \
    -vf "scale=1792:-2,crop=1344:768:'(iw-1344)*t/$n':'(ih-768)/2',fps=24,format=yuv420p" -t "$n" -c:v libx264 -crf 18 -c:a aac -shortest "$OUT/clip$n.mp4" ||
    fail "the $n s clip"
done
mapfile -t names < <(jq -r '.[].name' <<<"$CELLS")
[ "${#names[@]}" -gt 0 ] || fail "no cells"
for k in "${names[@]}"; do
  c=$(jq -c --arg k "$k" 'map(select(.name==$k)) | first' <<<"$CELLS")
  [[ $k =~ ^[A-Za-z0-9_-]+$ ]] && [ "$(jq --arg k "$k" 'map(select(.name==$k)) | length' <<<"$CELLS")" = 1 ] || fail "cell names must be unique words: $k"
  if [ -z "$(jq -r '.tree // empty' <<<"$c")" ]; then say "cell $k: published $(jq -r .target <<<"$c") $(jq -r '.args | join(" ")' <<<"$c")"; continue; fi
  t=$PACKAGES/$(jq -r .tree <<<"$c")/$(jq -r .target <<<"$c")
  [ -d "${t%/*}" ] || fail "cell $k: no ${t#$PACKAGES/}"
  say "cell $k: $(git -C "${t%/*}" rev-parse --short HEAD) ./$(jq -r .target <<<"$c") $(jq -r '.args | join(" ")' <<<"$c")"
done
jq -e --argjson n "$(jq -c 'map(.name)' <<<"$CELLS")" 'all(.[]; length == 2 and all(.[]; . as $x | $n | index($x)))' <<<"$COMPARE" >/dev/null ||
  fail "compare names cells that are not there"
# Arms (see Arms): each update starts one; its build is the Runtime, TensorFS and TensorD its update names over the arm's
# before it, its cap aside. ARMS maps each cell to its arm, build, warm-up mark and whether its arm keeps derived state.
ARMS=$(jq -c 'reduce .[] as $c ({n: 0, build: {}, keep: false, out: {}};
  (if $c.update then .n += 1 | .build += ($c.update | del(.gpu_memory_limit)) | .keep = ($c.keep_state // false) else . end)
  | .out[$c.name] = {arm: .n, build: (.build | tojson), warmup: ($c.warmup // false), keep: .keep}) | .out' <<<"$CELLS")
ORDER=$(param order abba); [[ $ORDER =~ ^(abba|one-way)$ ]] || fail "order $ORDER: abba or one-way"
bad=$(jq -r --argjson a "$ARMS" '.[] | select($a[.[0]].warmup or $a[.[1]].warmup) | join(" vs ")' <<<"$COMPARE" | paste -sd';')
[ -z "$bad" ] || fail "compare names a warm-up cell: $bad"
# Each two builds compared, in the order their arms ran; with abba, both orders must be there.
oneway=$(jq -r --argjson a "$ARMS" '[.[] | [$a[.[0]], $a[.[1]]] | select(.[0].build != .[1].build)
  | if .[0].arm < .[1].arm then [.[0].build, .[1].build] else [.[1].build, .[0].build] end] | unique
  | . as $seen | map(select([.[1], .[0]] as $r | any($seen[]; . == $r) | not) | "\(.[0]) before \(.[1])") | join("; ")' <<<"$COMPARE")
[ -z "$oneway" ] || [ "$ORDER" = one-way ] ||
  fail "builds compared in one order only (run them A B B A, or say order one-way): $oneway"
# memokey <cell> [arg...]: what TensorD's call memo keys the cell's request on, as far as a session can say: its Runtime and
# TensorFS (not the cap, not TensorD), its packages tree, target and arguments (its own, or those given).
memokey() {
  local k=$1 p='[]'; shift
  # The arguments as one JSON list (NUL-separated): jq would read one that starts with `--` (`--lane`) as its own option.
  [ $# = 0 ] || p=$(printf '%s\0' "$@" | jq -Rsc 'split("\u0000")[:-1]')
  jq -r --arg k "$k" --argjson a "$ARMS" --argjson p "$p" 'map(select(.name == $k)) | first
    | [($a[$k].build | fromjson | del(.tensord) | tojson), (.tree // ""), .target,
       ((if ($p | length) > 0 then $p else .args end) | join("\u0001"))] | join("\u0002")' <<<"$CELLS"
}
same=$(jq -r '.[] | "\(.[0]) \(.[1])"' <<<"$COMPARE" | while read -r a b; do [ "$(memokey "$a")" != "$(memokey "$b")" ] || echo "$a vs $b"; done | paste -sd';')
[ -z "$same" ] || fail "compared pairs ask the same request of the same Runtime and TensorFS, which TensorD's call memo answers from the first (it keys no memory cap): $same"
# Each update's wheels, fetched before renting so a missing one costs nothing.
wheel() { wheel_from "$OUT/wheels/$1-$2" "$3"; }
declare -A UPDATE=()
for k in "${names[@]}"; do
  w=()
  for kind in runtime tensorfs; do
    src=$(jq -r --arg k "$k" --arg t "$kind" 'map(select(.name==$k)) | first | .update[$t] // empty' <<<"$CELLS"); [ -n "$src" ] || continue
    f=$(wheel "$k" "$kind" "$src") && [ -n "$f" ] || fail "cell $k: no $kind wheel from $src"
    say "cell $k: updates $kind to ${f##*/}"; w+=("--$kind" "$f")
  done
  if [ "$(jq -r --arg k "$k" 'map(select(.name==$k)) | first | .update.tensord // false' <<<"$CELLS")" = true ]; then
    say "cell $k: updates TensorD to this session's $(cozy --version)"; w+=(--tensord "$(command -v cozy)")
  fi
  cap=$(jq -r --arg k "$k" 'map(select(.name==$k)) | first | .update.gpu_memory_limit // empty' <<<"$CELLS")
  [[ -z $cap || $cap =~ ^([0-9]+[MG]iB|off)$ ]] || fail "cell $k: gpu_memory_limit $cap"
  [ -z "$cap" ] || { say "cell $k: caps VRAM at $cap"; w+=(--gpu-memory-limit "$cap"); }
  [ "${#w[@]}" = 0 ] || UPDATE[$k]="${w[*]}"
done
printf 'run 5 · sync local/x (source changed)\nrun 6 · local/x/f\nrun      6\n' > "$OUT/probe-cell.txt"
current() { awk '$1=="run" && $3=="·" && $4!="sync"{n=$2} END{print n}' "$OUT/$1.txt" 2>/dev/null; }
[ "$(runno probe-cell)" = 6 ] && [ "$(current probe-cell)" = 6 ] || fail "parsers"
# Each tree's package carries its Triton sets before renting: a build at its first cell bills the pod while the runner compiles.
for k in "${names[@]}"; do
  c=$(jq -c --arg k "$k" 'map(select(.name==$k)) | first' <<<"$CELLS"); tree=$(jq -r '.tree // empty' <<<"$c")
  [ -z "$tree" ] || kernels "$PACKAGES/$tree/$(jq -r '.target | split("/") | first' <<<"$c")"
done
NEED=$(param need_s 3600)
# A hosted job ends at 6 h and keeps no log and no artifact then (43459786225): a session must end well inside it.
[ "$NEED" -le 19800 ] || fail "need_s $NEED: past 5.5 h, a hosted job's 6 h end would lose every result; split the session"
quote "$NEED"
rent
# A dev home on an RC build moves its rental to the RC's builds once it's ready: no cell starts before that ends.
cozy machine show "$NAME" --json > "$OUT/show-rc.json" 2> /dev/null
if [ -n "$(jq -r '.rc.state // empty' "$OUT/show-rc.json")" ]; then
  for _ in $(seq 180); do
    [ "$(jq -r '.rc.state // empty' "$OUT/show-rc.json")" = current ] && [ "$(jq -r '.state // empty' "$OUT/show-rc.json")" = ready ] && break
    sleep 5; cozy machine show "$NAME" --json > "$OUT/show-rc.json" 2> /dev/null
  done
  [ "$(jq -r '.rc.state // empty' "$OUT/show-rc.json")" = current ] || fail "$NAME isn't on the RC's builds in 15 min: $(jq -c .rc "$OUT/show-rc.json")"
fi
say "pod: $(jq -r '"cozy \(.status.version) · Runtime \(.status.runtime) · TensorFS \(.status.tensorfs)"' "$OUT/show-rc.json" 2> /dev/null)"


# held <cell> <before|after>: the models on the pod's disk (<cell>-models-<when>.json), each with its bytes in results.txt: what
# landed for a request, and between cells what came in the background (cl-1103).
held() {
  cozy model ls --on "$NAME" --json --full > "$OUT/$1-models-$2.json" 2>&1
  say "$1 disk $2: $(jq -r '[.models[]? | "\(.model) \(.lane) \((.bytes // 0) / 1e8 | floor / 10) GB\(if .complete == false then " partial" else "" end)"] | join(", ")' "$OUT/$1-models-$2.json" 2>/dev/null)"
}
. "$(dirname "$0")/gpu-reader.sh"
# listeners: the pod's listening TCP sockets as `address:port process`, `ss -Hltnp`'s, else read from /proc.
LISTENERS='import glob, os, socket, struct
owners = {}
for fd in glob.glob("/proc/[0-9]*/fd/*"):
    try:
        link = os.readlink(fd)
    except OSError:
        continue
    if link.startswith("socket:["):
        pid = fd.split("/")[2]
        try:
            owners.setdefault(link[8:-1], open(f"/proc/{pid}/comm").read().strip() + "/" + pid)
        except OSError:
            pass
for name, family in (("/proc/net/tcp", socket.AF_INET), ("/proc/net/tcp6", socket.AF_INET6)):
    for row in open(name).read().splitlines()[1:]:
        f = row.split()
        if f[3] == "0A":
            host, port = f[1].split(":")
            raw = b"".join(struct.pack("<I", int(host[i:i + 8], 16)) for i in range(0, len(host), 8))
            print(f"{socket.inet_ntop(family, raw)}:{int(port, 16)} {owners.get(f[9], chr(63))}")'
listeners() { timeout 60 cozy machine ssh "$NAME" "if command -v ss > /dev/null; then ss -Hltnp | awk '{print \$4, \$6}'; else echo $(base64 -w0 <<< "$LISTENERS") | base64 -d | python3 -; fi" 2>&1 | grep -v '^Warning: Permanently added'; }
echo -e "utc\tshow_ms\tshow_rc\tstate\trun\trunshow_ms\trunshow_rc\tstatus\tunreachable" > "$OUT/probe.tsv"
( p=0; while :; do
    # Every 5 min the pod's own GPU readings (gpu-pod.log: a stalled cell's other holder is named there); each minute its listeners,
    # and its TensorD log as it stands (tensord-now.log): a cell cut by a cancel keeps no <cell>-tensord.log, and 44139328775's
    # 42 minutes at 0 B left none to read (cl-1189).
    [ $((p % 20)) != 10 ] || readings
    [ $((p++ % 4)) != 0 ] || { k=$(cat "$OUT/current" 2>/dev/null); [ -z "$k" ] || listeners | sed "s/^/$k /" >> "$OUT/listeners.log"
      [ -z "$NAME" ] || { timeout 60 cozy machine logs "$NAME" -n 0 > "$OUT/tensord-now.log.next" 2>&1 && mv "$OUT/tensord-now.log.next" "$OUT/tensord-now.log"; }; }
    t=$(date +%s%3N); s=$(timeout 120 cozy machine show "$RID" --json 2>&1); src=$?; sm=$(( $(date +%s%3N) - t ))
    k=$(cat "$OUT/current" 2>/dev/null); N=''; [ -z "$k" ] || N=$(current "$k")
    rs='' rms='' rrc='' rst=''
    if [ -n "$N" ]; then t=$(date +%s%3N); rs=$(cd "$RUN_PROJECT" && timeout 120 cozy run watch "$N" --once --json --full 2>&1); rrc=$?; rms=$(( $(date +%s%3N) - t ))
      rst=$(jq -r '.status // empty' <<<"$rs" 2>/dev/null | head -1); fi
    printf '%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\n' "$(date -u +%T)" "$sm" "$src" "$(jq -r '.state // empty' <<<"$s" 2>/dev/null | head -1)" \
      "$N" "$rms" "$rrc" "$rst" "$(printf '%s\n%s' "$s" "$rs" | grep -ci unreachable)" >> "$OUT/probe.tsv"
    sleep 15
  done ) & BG=$!

# summary <name>: execution, then each GPU call's denoise (else longest) step track, peak driver VRAM, lanes, kernels, fused installs.
summary() { jq -r '"exec \((.execution_ms // 0) / 1000 | floor)s" + ([.calls[]? | select(.measurements.execution? != null) | .measurements as $m
  | ($m.attribution.steps // {} | to_entries | (map(select(.key == "denoise")) + [max_by(.value.count // 0)])[0]) as $s | ($s.value.series // [] | map(.[1])) as $x
  | "; \(.function) memoized=\(.memoized): \($s.key // "-") step1 \(($x[0] // 0) / 100 | floor / 10)s steady \((if ($x | length) > 1 then ($x[1:] | add / length) else 0 end) / 100 | floor / 10)s x\($x | length), "
  + "peak VRAM \(([$m.memory.driver[]?.peak_bytes] | max // 0) / 107374182.4 | floor / 10) GiB, lanes \([$m.models[]? | "\(.slot)=\(.lane)"] | join(" ")), "
  + "attention \([$m.observations[]? | select(.name == "attention.selected") | "\(.fields.component | split("/") | last)=\(.value)"] | unique | join(" ")), "
  + "gemm \([$m.observations[]? | select(.name == "gemm.selected") | .value] | unique | join(",")), "
  + "ready \([$m.observations[]? | select(.name == "kernel.ready") | .value] | unique | join(",")), "
  + "plan \([$m.execution.plan_reports[]? | to_entries[] | "\(.key)=\(.value.applied)\(if (.value.code // "") != "" then "(\(.value.code))" else "" end)"] | join(" ")), "
  + "reruns \([$m.memory.stats.varena[]?.stats.reruns[]? | "\(.site): \(.error) (reserve \(.reserve / 1048576 | floor) MiB, gave \(.released / 1048576 | floor) MiB, paused \(.paused_ms) ms)"] | join("; "))"] | join(""))' "$OUT/$1.json" 2>/dev/null; }
# weights <name>: each GPU call's text encode, the pod's disk and host-to-GPU ceilings, its LoRA bakes, where its weight fills
# came from (GB/fills/s by origin), and its host tier's reads ahead and derived pages written (cr-277).
weights() { jq -r '[.calls[]? | select(.measurements.memory.stats.varena? != null) | .measurements as $m
  | $m.memory.stats.varena[0] as $v | ($v.stats.baked_pages // {}) as $t | "\(.function): text \(($m.attribution.stages.condition_text.total_ms // 0) / 100 | floor / 10)s, "
  + "disk \(($v.ceiling.disk_gbps // 0) * 10 | floor / 10) h2d \(($v.ceiling.h2d_gbps // 0) * 10 | floor / 10) GB/s, bake \($v.stats.moved.bake.blocks // 0), fills "
  + ([$v.stats.moved | to_entries[] | select(.key != "bake") | "\(.key) \((.value.bytes // 0) / 1e8 | floor / 10)/\(.value.fills // 0)/\((.value.ns // 0) / 1e8 | floor / 10)s"] | join(" "))
  + ", read ahead \($t.read_ahead // "-") \(($t.read_ahead_bytes // 0) / 1e8 | floor / 10) GB refused \($t.read_ahead_refused // "-"), persisted \($t.persisted // "-") "
  + "\(($t.persisted_bytes // 0) / 1e8 | floor / 10) GB skipped \($t.persist_skipped // "-") failed \($t.persist_failed // "-")"] | join("; ")' "$OUT/$1.json" 2>/dev/null; }
# hold <cell> <on|off>: suites/bench/vram.py from a root shell takes all of GPU 0 but FREE_GIB, or ends. A package can't
# hold it (the Runtime refuses one that starts a process), and TensorD takes a GPU out of service that another program
# holds memory on when it starts, so the holder ends before each update's restart and starts again after it.
hold() {
  local script
  if [ "$2" = off ]; then
    script='pgrep -f /var/tmp/ab-vram.py | xargs -r kill; rm -f /var/tmp/ab-vram.ready; sleep 2'
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
  # Right after an update's restart the pod's ssh can answer nothing (pod-session 43325104295): try again, and an `on`
  # that did start its holder says so the next time instead of starting another.
  local try held='' free=''
  for try in 1 2 3 4 5 6; do
    cozy machine ssh "$NAME" "echo $(base64 -w0 <<< "$script") | base64 -d | sh" > "$OUT/hold-$1-$2.txt" 2>&1; local code=$?
    if [ "$2" = off ]; then [ "$code" = 0 ] && return 0; else
      read -r _ held free _ < <(tail -1 "$OUT/hold-$1-$2.txt"); [[ ${held:-x} =~ ^[0-9]+$ ]] && break
    fi
    say "$1: holder $2, try $try: exit $code $(head -c 200 "$OUT/hold-$1-$2.txt")"; sleep 10
  done
  [ "$2" = on ] || fail "$1: the holder did not end"
  [[ ${held:-x} =~ ^[0-9]+$ ]] || fail "$1: no holder: $(head -c 300 "$OUT/hold-$1-$2.txt")"
  say "$1: holder $((held >> 20)) MiB held, $((free >> 20)) MiB left free"
}
# ram <cell> <GiB|off>: a root shell's program holds the pod's host RAM but GiB, or ends. Free is what TensorD reads
# (platform contain/host_memory_linux.rs `available_outside`): MemAvailable, or less where a cgroup limit on the holder's
# path (v1 or v2, any parent) leaves less room, inactive clean page cache counted free; the tightest is its bound. It
# fills against the live reading until free is the target, then holds still through the cell, logging free every 10 s.
# Each GiB is its own process, far below the score the Runtime gives an executor (>= 500, trampoline.py): the kernel's OOM
# victim is the run, as on a computer of many smaller programs, not the holder. `off` fails the cell if any of it ended.
RAM_HOLDER='import mmap, os, signal, sys, time
GIB, BLOCK = 1 << 30, 64 << 20
leave, ready, log = int(float(sys.argv[1]) * GIB), sys.argv[2], sys.argv[3]
def meminfo(field):
    return next(int(l.split()[1]) * 1024 for l in open("/proc/meminfo") if l.startswith(field + ":"))
def number(path):
    try:
        n = int(open(path).read().split()[0])
    except (OSError, ValueError, IndexError):
        return None
    return n if 0 < n < 1 << 60 else None
def cgroups():
    lines = open("/proc/self/cgroup").read().splitlines()
    if os.path.isfile("/sys/fs/cgroup/cgroup.controllers"):
        rel = next((l[3:] for l in lines if l.startswith("0::")), None)
        top, limits, usage = "/sys/fs/cgroup", ("memory.high", "memory.max"), "memory.current"
    else:
        rel = next((l.split(":", 2)[2] for l in lines if "memory" in l.split(":", 2)[1].split(",")), None)
        mount = next(((f[3].rstrip("/"), f[4]) for f in (l.split() for l in open("/proc/self/mountinfo"))
                      if f[f.index("-") + 1] == "cgroup" and "memory" in f[f.index("-") + 3].split(",")), None)
        if mount is None or rel is None:
            return []
        rel = rel[len(mount[0]):] if rel.startswith(mount[0]) else rel
        top, limits, usage = mount[1], ("memory.limit_in_bytes",), "memory.usage_in_bytes"
    if rel is None:
        return []
    path, at = [], os.path.normpath(os.path.join(top, rel.lstrip("/")))
    while True:
        path.append((at, limits, usage))
        if at == top or not at.startswith(top + "/"):
            return path
        at = os.path.dirname(at)
def ours(cgroup):  # the machine own containment: its limits are the machine, not the computer
    return any(c.startswith("cozy-executor-") or (len(c) == 21 and c.startswith("cozy-")
               and all(x in "0123456789abcdefABCDEF" for x in c[5:])) for c in cgroup.split("/"))
def cache(cgroup):
    try:
        s = dict(l.split()[:2] for l in open(cgroup + "/memory.stat") if len(l.split()) > 1)
    except OSError:
        return 0
    row = lambda *names: next((int(s[k]) for n in names for k in ("total_" + n, n) if k in s), 0)
    return max(row("inactive_file") - row("file_dirty", "dirty") - row("file_writeback", "writeback"), 0)
def free():
    total = meminfo("MemTotal")
    room, bound = meminfo("MemAvailable"), f"MemAvailable (MemTotal {total / GIB:.1f} GiB)"
    for cgroup, limits, usage in cgroups():
        limit = min((n for n in (number(f"{cgroup}/{name}") for name in limits) if n), default=None)
        if limit is not None and not ours(cgroup):
            r = max(limit - max((number(f"{cgroup}/{usage}") or 0) - cache(cgroup), 0), 0)
            if r < room:
                room, bound = r, f"cgroup {cgroup} limit {limit / GIB:.1f} GiB"
    return room, bound
PATTERN, pieces = b"\1" * BLOCK, []
def end(why):
    for pid, _ in pieces:
        os.kill(pid, signal.SIGKILL)
    sys.exit(why)
def take(n):
    r, w = os.pipe()
    pid = os.fork()
    if pid == 0:
        m = mmap.mmap(-1, n, flags=mmap.MAP_PRIVATE)
        for at in range(0, n, BLOCK):
            m[at:at + min(BLOCK, n - at)] = PATTERN[:min(BLOCK, n - at)]
        os.write(w, b"1")
        while True:
            signal.pause()
    os.close(w)
    wrote = os.read(r, 1)
    os.close(r)
    if not wrote:
        os.waitpid(pid, 0)
        end(f"the kernel ended a holder piece at {sum(n for _, n in pieces) / GIB:.1f} GiB held: a limit under {free()[1]}")
    pieces.append((pid, n))
def give(n):
    while n > 0 and pieces:
        pid, size = pieces.pop()
        os.kill(pid, signal.SIGKILL)
        os.waitpid(pid, 0)
        if size > n:
            take(size - n)
        n -= size
room, bound = free()
if room - leave > 256 * GIB:  # writing it takes minutes and can starve the pod: a host this large is the wrong host
    sys.exit(f"{(room - leave) / GIB:.0f} GiB to hold under {bound}: rent a host with less memory for this cell")
still, until = 0, None
while still < 3 and (until is None or time.time() < until):
    room, bound = free()
    over = (room - leave) // BLOCK * BLOCK
    if over > 0:
        take(min(GIB, over))
    elif over < -BLOCK and pieces:
        give(-over)
        until = until or time.time() + 30
    else:
        until, still = until or time.time() + 30, still + 1
        time.sleep(1)
        continue
    still = 0
held, gone = sum(n for _, n in pieces), set()
open(ready + ".tmp", "w").write(f"{os.getpid()} {held} {room} {len(pieces)} {bound}\n")
os.replace(ready + ".tmp", ready)
while True:
    gone |= {pid for pid, _ in pieces if pid not in gone and os.waitpid(pid, os.WNOHANG)[0]}
    with open(log, "a") as f:
        f.write(f"{int(time.time())} {free()[0]} {sum(n for pid, n in pieces if pid not in gone)}\n")
    time.sleep(10)'
ram() {
  local script try code held='' left='' pieces='' bound='' state lo hi n lost
  if [ "$2" = off ]; then
    # the first try's time is the end: a later try after a dropped ssh reads the same
    script='[ -s /var/tmp/ab-ram.end ] || date +%s > /var/tmp/ab-ram.end
pgrep -f /var/tmp/ab-ram.py | xargs -r kill; rm -f /var/tmp/ab-ram.ready; cat /var/tmp/ab-ram.end /var/tmp/ab-ram.log 2> /dev/null; sleep 2'
  else
    script="pgrep -f /var/tmp/ab-ram.py | xargs -r kill; i=0; while pgrep -f /var/tmp/ab-ram.py > /dev/null && [ \$i -lt 120 ]; do sleep 0.5; i=\$((i + 1)); done
echo $(base64 -w0 <<< "$RAM_HOLDER") | base64 -d > /var/tmp/ab-ram.py && rm -f /var/tmp/ab-ram.ready /var/tmp/ab-ram.log /var/tmp/ab-ram.end
setsid nohup python3 /var/tmp/ab-ram.py $2 /var/tmp/ab-ram.ready /var/tmp/ab-ram.log > /var/tmp/ab-ram.out 2>&1 < /dev/null &
i=0; while [ ! -s /var/tmp/ab-ram.ready ] && [ \$i -lt 600 ] && pgrep -f /var/tmp/ab-ram.py > /dev/null; do sleep 0.5; i=\$((i + 1)); done
cat /var/tmp/ab-ram.ready 2> /dev/null || tail -c 300 /var/tmp/ab-ram.out"
  fi
  gib() { awk -v b="$1" 'BEGIN { printf "%.1f", b / 1073741824 }'; }
  for try in 1 2 3; do
    cozy machine ssh "$NAME" "echo $(base64 -w0 <<< "$script") | base64 -d | sh" > "$OUT/ram-$1-$2.txt" 2>&1; code=$?
    if [ "$2" = off ]; then
      [ "$code" = 0 ] || { say "$1: host holder off, try $try: exit $code $(head -c 200 "$OUT/ram-$1-$2.txt")"; sleep 10; continue; }
      # Held to the end: a reading within 30 s of it, every piece still there in each.
      read -r state lo hi n lost < <(awk -v held="$RAM_HELD" '!/^[0-9 ]+$/ { next } NF == 1 && !end { end = $1; next }
        NF == 3 { n++; f = $2 / 1073741824; if (n == 1 || f < lo) lo = f; if (f > hi) hi = f; if (held - $3 > lost) lost = held - $3; last = $1 }
        END { printf "%s %.1f %.1f %d %.0f\n", (n && end && end - last <= 30) ? "held" : "ended", lo, hi, n, lost }' "$OUT/ram-$1-$2.txt")
      [ "$state" = held ] || fail "$1: the host holder ended during the cell: its host tier wasn't short"
      [ "$lost" = 0 ] || fail "$1: the kernel ended $(gib "$lost") GiB of the host holder during the cell: its host tier wasn't short"
      say "$1: host holder held all $(gib "$RAM_HELD") GiB through the cell, free $lo to $hi GiB over $n readings"
      return 0
    fi
    read -r _ held left pieces bound < <(tail -1 "$OUT/ram-$1-$2.txt"); [[ ${held:-x} =~ ^[0-9]+$ ]] && break
    ! grep -q "rent a host" "$OUT/ram-$1-$2.txt" || break
    say "$1: host holder $2, try $try: exit $code $(head -c 200 "$OUT/ram-$1-$2.txt")"; sleep 10
  done
  [ "$2" != off ] || fail "$1: the host holder did not end"
  [[ ${held:-x} =~ ^[0-9]+$ ]] || fail "$1: no host holder: $(head -c 300 "$OUT/ram-$1-$2.txt")"
  RAM_HELD=$held
  say "$1: host holder $(gib "$held") GiB held in $pieces pieces, $(gib "$left") GiB left free ($2 asked) under $bound"
}
# cellstate <name>: the record's LoRA bake blocks, derived bytes filled and persisted pages (the run's own measurements and
# its calls', every GPU), and whether the call memo answered it, tab-separated.
cellstate() { jq -r '. as $r | [.measurements, (.calls[]?.measurements)] | map(select(. != null) | .memory.stats.varena[]?.stats // empty) as $v
  | [($v | map(.moved.bake.blocks // 0) | add // 0), ($v | map(.moved.derived.bytes // 0) | add // 0),
     ($v | map(.baked_pages.persisted // 0) | max // 0), (($r.memo.memoized // false) or ([$r.calls[]?.memoized] | any) | tostring)] | @tsv' \
  "$OUT/$1.json" 2>/dev/null; }
printf 'cell\tarm\ttemperature\tbake_blocks\tderived_bytes\tpersisted\tmemoized\texec_s\tdownloaded\n' > "$OUT/states.tsv"
fails=0 memoized=0
declare -A MADE=() SEEN=() ASKED=()
for k in "${names[@]}"; do
  c=$(jq -c --arg k "$k" 'map(select(.name==$k)) | first' <<<"$CELLS"); mapfile -t args < <(jq -r '.args[]' <<<"$c")
  FLOOR_MBS=$(jq -r --arg d "$FLOOR0" '.floor_mb_s // $d' <<<"$c") FLOOR_AFTER=$(jq -r --arg d "$AFTER0" '.floor_after_s // $d' <<<"$c")
  for i in "${!args[@]}"; do
    a=${args[$i]//\{ref\}/$REF}; a=${a//\{clip\}/$CLIP}; a=${a//\{depth\}/$PICS/depth.png}; a=${a//\{digits\}/$DIGITS}; a=${a//\{long\}/$LONG}; a=${a//\{clip5\}/$OUT/clip5.mp4}; a=${a//\{clip15\}/$OUT/clip15.mp4}
    while [[ $a =~ \{pic([0-9]+)\} ]]; do a=${a//"${BASH_REMATCH[0]}"/$PICS/pic${BASH_REMATCH[1]}.jpg}; done
    while [[ $a =~ \{out:([A-Za-z0-9_-]+)\} ]]; do
      [ -n "${MADE[${BASH_REMATCH[1]}]:-}" ] || fail "cell $k: ${BASH_REMATCH[1]} made no image or video"
      a=${a//"${BASH_REMATCH[0]}"/"${MADE[${BASH_REMATCH[1]}]}"}
    done
    args[$i]=$a
  done
  if [ -n "${UPDATE[$k]:-}" ]; then
    [ -z "$FREE_GIB" ] || [ "$k" = "${names[0]}" ] || hold "$k" off
    # An arm begins: the derived state before it goes, and the update's restart ends every executor (see Arms).
    keep=$(jq -r --arg k "$k" '.[$k].keep' <<<"$ARMS")
    [ "$keep" = true ] || derived_state "$k" reset
    # shellcheck disable=SC2086 # the update's flags and wheel paths (no spaces)
    step "update-$k" machine update "$NAME" ${UPDATE[$k]} -y || fail "$k: the update"
    say "$k: pod Runtime $(cozy machine show "$NAME" --json | jq -r .status.runtime), TensorFS $(cozy machine show "$NAME" --json | jq -r .status.tensorfs)"
    derived_state "$k" settle
    [ "$keep" = true ] || derived_zero || fail "$k: its arm starts with derived state it did not make: $DERIVED"
    SEEN=()
    gpuwatch
  fi
  [ -z "$FREE_GIB" ] || { [ -z "${UPDATE[$k]:-}" ] && [ "$k" != "${names[0]}" ]; } || hold "$k" on
  mapfile -t gone < <(jq -r '.uninstall // [] | .[]' <<<"$c")
  if [ "${gone[0]:-}" = '*' ]; then
    mapfile -t gone < <(cozy model ls --on "$NAME" --json 2> /dev/null | jq -r '[.models[]? | "\(.role // "model") \(.model)"] | unique | .[]')
  else
    gone=("${gone[@]/#/model }")
  fi
  for i in "${!gone[@]}"; do
    read -r role ref <<<"${gone[$i]}"; [ "$role" = lora ] || role=model
    step "uninstall-$k-$i" "$role" uninstall "$ref" --on "$NAME" -y < /dev/null || say "$k: $ref stays on the pod: $(tail -c 200 "$OUT/uninstall-$k-$i.txt")"
  done
  mapfile -t downloads < <(jq -c '.download // [] | .[]' <<<"$c")
  for i in "${!downloads[@]}"; do
    mapfile -t dl < <(jq -r '.[]' <<<"${downloads[$i]}")
    step "download-$k-$i" model install "${dl[@]}" --on "$NAME" < /dev/null || fail "$k: the download of ${dl[0]}"
  done
  # wait_s: the executor left idle this long before the cell (an idle period's work runs); its record says whether it did
  w=$(jq -r '.wait_s // 0' <<<"$c"); [[ $w =~ ^[0-9]+$ ]] || fail "cell $k: wait_s $w"
  [ "$w" = 0 ] || { say "$k: the machine left idle ${w}s"; sleep "$w"; }
  hg=$(jq -r '.host_free_gib // empty' <<<"$c"); [[ -z $hg || $hg =~ ^[0-9]+([.][0-9]+)?$ ]] || fail "cell $k: host_free_gib $hg"
  [ -z "$hg" ] || ram "$k" "$hg"
  echo "$k" > "$OUT/current"
  say "$k: GPU before: $(gpu | paste -sd';')"
  tree=$(jq -r '.tree // empty' <<<"$c") target=$(jq -r .target <<<"$c")
  held "$k" before
  cd "$PACKAGES/$tree" || fail "no tree for $k"
  [ -z "$tree" ] || target=./$target
  # A request this session already asked of this Runtime and TensorFS, as asked (a moved seed counts), takes the next seed none
  # of them asked: the call memo would answer it.
  key=$(memokey "$k" "${args[@]}"); [ -n "$key" ] || fail "$k: no call-memo key for its request"
  if [ -n "${ASKED[$key]:-}" ] && ! printf '%s\n' "${args[@]}" | grep -q '^seed:=[0-9][0-9]*$'; then
    say "$k: asked before of this Runtime and TensorFS, with no seed to move: the call memo may answer it"
  elif [ -n "${ASKED[$key]:-}" ]; then
    while [ -n "${ASKED[$key]:-}" ]; do
      mapfile -t args < <(printf '%s\n' "${args[@]}" | awk '/^seed:=[0-9]+$/ {split($0, a, "="); print "seed:=" a[2] + 1; next} {print}')
      key=$(memokey "$k" "${args[@]}"); [ -n "$key" ] || fail "$k: no call-memo key for its request"
    done
    say "$k: asked before of this Runtime and TensorFS: $(printf '%s\n' "${args[@]}" | grep -m1 '^seed:=')"
  fi
  ASKED[$key]=1
  say "$k: start"  # what each process held as it started: gpu-pod.log's last reading before this (cr-297)
  # max_s: the cell's run is canceled this long in, and said UNFINISHED with where it stood: slow is not failed.
  ms=$(jq -r '.max_s // 0' <<<"$c"); [[ $ms =~ ^[0-9]+$ ]] || fail "cell $k: max_s $ms"; MS=''; rm -f "$OUT/$k.capped"
  if [ "$ms" != 0 ]; then
    ( sleep "$ms" || exit 0; clock=$(run_clock "$NAME") || exit 0; echo "$ms" > "$OUT/$k.capped"
      for r in ${clock#* }; do cozy run cancel "$r" > /dev/null 2>&1; done ) 9>&- & MS=$!
  fi
  step "$k" run "$target" "${args[@]}" --on "$NAME"; code=$?
  [ -z "$MS" ] || { pkill -P "$MS" 2> /dev/null; kill "$MS" 2> /dev/null; MS=''; }
  [ -z "$hg" ] || ram "$k" off
  N=$(runno "$k"); [ -n "$N" ] || N=$(current "$k")  # a failed run's output names it only in its first line
  # `step` runs every cell from the session's project (lib.sh RUN_PROJECT): its runs are known there alone.
  cd "$RUN_PROJECT" || fail "no $RUN_PROJECT"
  if [ -n "$N" ]; then cozy run watch "$N" --once --json --full > "$OUT/$k.json" 2>/dev/null; cozy run watch "$N" --once -o "$OUT/$k.out" > /dev/null 2>&1; fi
  cd "$OUT" || fail "no $OUT"
  held "$k" after
  cozy machine logs "$NAME" -n 0 > "$OUT/$k-tensord.log" 2>&1
  st=$(jq -r '.status // "none"' "$OUT/$k.json" 2>/dev/null); st=${st:-none}
  MADE[$k]=$(ls "$OUT/$k.out"/* 2> /dev/null | grep -E '[.](png|webp|jpg|mp4)$' | sort | head -1)
  say "$k run ${N:-none} ${st:-none} (exit $code): $(summary "$k")"
  say "$k has/needs/extras: $(jq -c '[.measurements.weights?, (.calls[]? | select(.measurements.weights? != null) | {(.function // "call"): .measurements.weights})] | map(select(. != null))' "$OUT/$k.json" 2>/dev/null)"
  # until_idle {region, step_s, max}: while this request's record says the idle queue hadn't reached `region` before it
  # (`graphs.<region>.idle.queued` > 0), wait step_s more and ask again with the next seed (`<cell>-w<i>`), up to max
  ui=$(jq -c '.until_idle // empty' <<<"$c")
  if [ -n "$ui" ]; then
    reg=$(jq -r .region <<<"$ui") stp=$(jq -r '.step_s // 30' <<<"$ui") most=$(jq -r '.max // 4' <<<"$ui") i=0 kk=$k waited=$(jq -r '.wait_s // 0' <<<"$c")
    while q=$(jq -r --arg r "$reg" '.measurements.residency.graphs[$r].idle.queued // "none"' "$OUT/$kk.json" 2>/dev/null); [ "$q" != 0 ] && [ "$q" != none ] && [ "$i" -lt "$most" ]; do
      i=$((i + 1)); kk=$k-w$i; waited=$((waited + stp))
      say "$k: $reg's idle queue held $q before it; the machine left idle ${stp}s more (${waited}s in all), then $kk"
      sleep "$stp"; mapfile -t again < <(printf '%s\n' "${args[@]}" | awk -v i="$i" '/^seed:=[0-9]+$/ {split($0, a, "="); print "seed:=" a[2] + i; next} {print}')
      cd "$PACKAGES/$tree" || fail "no tree for $kk"
      step "$kk" run "$target" "${again[@]}" --on "$NAME"; NN=$(runno "$kk"); [ -n "$NN" ] || NN=$(current "$kk")
      cd "$RUN_PROJECT" || fail "no $RUN_PROJECT"
      [ -z "$NN" ] || { cozy run watch "$NN" --once --json --full > "$OUT/$kk.json" 2>/dev/null; cozy run watch "$NN" --once -o "$OUT/$kk.out" > /dev/null 2>&1; }
      cd "$OUT" || fail "no $OUT"
      say "$kk run ${NN:-none} $(jq -r '.status // "none"' "$OUT/$kk.json" 2>/dev/null): $(summary "$kk")"
    done
    [ "$q" != none ] || say "$k: the record has no region $reg (graphs: $(jq -r '.measurements.residency.graphs // {} | keys | join(", ")' "$OUT/$kk.json" 2>/dev/null))"
    say "$k: $reg's idle queue before the last request: $q (after ${waited}s idle; the last request is $kk)"
  fi
  say "$k weights: $(weights "$k")"
  # Its temperature in its arm: the warm-up's, else cold for its function's first request since the arm began, else warm.
  if [ "$(jq -r --arg k "$k" '.[$k].warmup' <<<"$ARMS")" = true ]; then temp=warmup; elif [ -n "${SEEN[$target]:-}" ]; then temp=warm; else temp=cold; fi
  SEEN[$target]=1
  read -r bake derived persisted memo < <(cellstate "$k")
  # Weights fetched inside the request: its seconds are the link's (44006261838: 0.25.0's first H3 spent 454 s of 822 here).
  fetched=false; ! grep -qE 'downloading .* weights [0-9.]+ [KMG]i?B/' "$OUT/$k.txt" 2>/dev/null || fetched=true
  printf '%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\n' "$k" "$(jq -r --arg k "$k" '.[$k].arm' <<<"$ARMS")" "$temp" "${bake:-0}" "${derived:-0}" \
    "${persisted:-0}" "${memo:-false}" "$(jq -r '(.execution_ms // 0) / 1000' "$OUT/$k.json" 2>/dev/null)" "$fetched" >> "$OUT/states.tsv"
  say "$k state: $temp, bake ${bake:-0}, derived $(( ${derived:-0} / 1000000000 )) GB, persisted ${persisted:-0}, memoized ${memo:-false}, fetched weights $fetched"
  [ "${memo:-false}" != true ] || { memoized=$((memoized + 1)); say "$k: MEMOIZED: the call memo answered it, so its seconds are no measure"; }
  want=$(jq -r '.expect // "completed"' <<<"$c")
  if [ "$st" != completed ]; then
    why=$(jq -r '.reason.message // .reason // empty' "$OUT/$k.json" 2>/dev/null)
    say "$k reason: $(printf '%s' "${why:-$(tr '\r' '\n' < "$OUT/$k.txt" | grep -m1 -E '^error:' || tail -c 600 "$OUT/$k.txt")}" | head -c 600)"
    say "$k: the GPU now: $(gpu | paste -sd';')"
  fi
  if [ -s "$OUT/$k.capped" ] && [ "$st" != completed ]; then
    say "$k UNFINISHED at its $(cat "$OUT/$k.capped") s cap: at $(jq -r '"\(.progress.stage // "-") \(.progress.completed // 0)/\(.progress.total // 0)"' "$OUT/$k.json" 2> /dev/null), execution $(jq -r '(.execution_ms // 0) / 1000' "$OUT/$k.json" 2> /dev/null) s"
    continue
  fi
  # A refusal before any run (st none) is the failure an `expect: failed` cell proves.
  [ "$st" = "$want" ] || { [ "$want" = failed ] && [ "$st" = none ] && [ "$code" != 0 ]; } ||
    { fails=$((fails + 1)); [ "$fails" -lt "$MOST" ] || fail "$fails cells failed"; }
done
rm -f "$OUT/current"

digests() { jq -r '[.result | .. | .digest? // empty] | join(",")' "$OUT/$1.json" 2>/dev/null; }
file() { ls "$OUT/$1.out"/* 2> /dev/null | grep -E '\.(mp4|webm|png|webp|jpg|wav|flac)$' | head -1; }
frames() { local f; f=$(file "$1"); [ -s "$f" ] || { echo none; return; }; ffmpeg -nostdin -v error -i "$f" -map 0 -f framemd5 - | grep -v '^#' | md5sum | cut -c1-16; }
psnr() { ffmpeg -nostdin -i "$(file "$1")" -i "$(file "$2")" -lavfi '[0:v][1:v]psnr' -f null - 2>&1 | grep -oE 'average:[0-9.inf]+' | tail -1; }
# judge <a> <b>: the bench's same-seed rule (suites/bench/media.py) on the two cells' outputs: `like`, or why not.
judge() {
  local fa fb; fa=$(file "$1") fb=$(file "$2"); [ -s "$fa" ] && [ -s "$fb" ] || { echo "no output to judge"; return; }
  uv run -q --no-project --with numpy python3 - "$SUITE" "$fa" "$fb" "$1" 2>&1 <<'PY' | tail -1
import sys; sys.path.insert(0, sys.argv[1]); import media
why = media.differs(media.pair(sys.argv[2], sys.argv[3]), sys.argv[4] + "'s output (same request and seed)")
print('like' if not why else 'DIFFERS: ' + '; '.join(why))
PY
}
differ=0 judged=0
while read -r a b; do
  da=$(digests "$a") db=$(digests "$b")
  if [ -n "$da" ] && [ "$da" = "$db" ]; then say "$a vs $b: IDENTICAL bytes ($da)"; continue; fi
  fa=$(frames "$a") fb=$(frames "$b")
  if [ "$fa" != none ] && [ "$fa" = "$fb" ]; then say "$a vs $b: IDENTICAL decoded frames ($fa); files differ ($da / $db)"; continue; fi
  differ=$((differ + 1)); say "$a vs $b: DIFFERENT: digests $da / $db, frames $fa / $fb, video PSNR $(psnr "$a" "$b")"
  j=$(judge "$a" "$b"); say "$a vs $b: bench same-seed judge: $j"; [ "$j" = like ] || judged=$((judged + 1))
done < <(jq -r '.[] | "\(.[0]) \(.[1])"' <<<"$COMPARE")
for k in "${names[@]}"; do
  wide=$(awk -v k="$k" '$1 == k && $2 !~ /^\[?(127\.|::1\]?:|::ffff:127\.)/ {print $2, $3}' "$OUT/listeners.log" 2>/dev/null | sort -u | paste -sd';')
  say "$k: TCP listeners beyond loopback while it ran: ${wide:-none} ($(awk -v k="$k" '$1 == k' "$OUT/listeners.log" 2>/dev/null | wc -l) rows, listeners.log)"
done
u=0; for k in "${names[@]}"; do x=$(grep -ci unreachable "$OUT/$k.txt" 2>/dev/null); u=$((u + ${x:-0})); done
say "unreachable lines in the cells' follow output: $u"
say "bench same-seed judge: $judged of $(jq length <<<"$COMPARE") pairs differ"
awk -F'\t' 'NR>1{n++; if($2>m)m=$2; if($3!=0)e++; if($6!=""){r++; if($6>q)q=$6; if($7!=0)f++}; u+=$9}
  END{printf "probe: %d machine show (max %d ms, %d failed), %d run watch (max %d ms, %d non-zero), %d unreachable\n", n, m, e, r, q, f, u}' "$OUT/probe.tsv" | tee -a "$R"
# Each compared pair's seconds, cold and warm apart (pairs.tsv), and whether it may be compared at all (see Arms).
printf 'temperature\ta\tb\ta_s\tb_s\tb_vs_a\tfair\n' > "$OUT/pairs.tsv"
unfair=0
while read -r a b; do
  IFS=$'\t' read -r _ _ ta bake_a der_a _ memo_a sa dl_a < <(awk -F'\t' -v k="$a" '$1 == k' "$OUT/states.tsv")
  IFS=$'\t' read -r _ _ tb bake_b der_b _ memo_b sb dl_b < <(awk -F'\t' -v k="$b" '$1 == k' "$OUT/states.tsv")
  why=''
  [ -n "${ta:-}" ] && [ -n "${tb:-}" ] || why="a cell did not run"
  [ -n "$why" ] || [ "$ta" = "$tb" ] || why="$a is $ta, $b is $tb"
  [ -n "$why" ] || { [ "$memo_a" != true ] && [ "$memo_b" != true ]; } || why="the call memo answered one"
  [ -n "$why" ] || { [ "$dl_a" != true ] && [ "$dl_b" != true ]; } || why="one fetched its weights: run each function once before the first arm"
  [ -n "$why" ] || [ "$(jq -r --arg a "$a" --arg b "$b" '.[$a].keep == .[$b].keep' <<<"$ARMS")" = true ] || why="one arm kept another's derived state"
  for x in "$a:$ta:$bake_a:$der_a" "$b:$tb:$bake_b:$der_b"; do
    IFS=: read -r c t bk dr <<<"$x"
    [ -n "$why" ] || [ "$t" != cold ] || [ "${dr:-0}" = 0 ] || [ "${bk:-0}" != 0 ] || why="cold $c read derived pages it did not make"
  done
  d=$(awk -v x="${sa:-0}" -v y="${sb:-0}" 'BEGIN { if (x > 0) printf "%+.1f%%", (y / x - 1) * 100; else print "-" }')
  printf '%s\t%s\t%s\t%s\t%s\t%s\t%s\n' "${ta:-none}" "$a" "$b" "${sa:-}" "${sb:-}" "$d" "${why:-yes}" >> "$OUT/pairs.tsv"
  [ -z "$why" ] || { unfair=$((unfair + 1)); say "$a vs $b: NOT COMPARABLE: $why"; }
done < <(jq -r '.[] | "\(.[0]) \(.[1])"' <<<"$COMPARE")
for t in cold warm; do
  say "$t pairs (a s, b s, b vs a): $(awk -F'\t' -v t="$t" '$1 == t && $7 == "yes" {printf "%s %s %s %s %s; ", $2, $3, $4, $5, $6}' "$OUT/pairs.tsv")"
done
[ "$fails" = 0 ] && [ "$differ" = 0 ] && [ "$unfair" = 0 ] && [ "$memoized" = 0 ] ||
  fail "$fails cells did not complete, $differ pairs differ, $unfair pairs not comparable, $memoized cells answered by the call memo"
