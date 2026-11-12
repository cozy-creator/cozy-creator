# ab: one rental, a list of cells (each one run of a package function from a packages checkout, run in order), and
# pairs of cells compared for identical output: a refactor's proof is the same request giving the same bytes (xs-101).
# While cells run, `machine show` and the current run's `run watch --once` are timed every 15 s (probe.tsv): a busy
# machine keeps answering and is never "unreachable" (cl-671).
# A pod's first call of each tree serves what has not compiled yet (its LoRA per step until the bake kernel is ready,
# a fused block eager until it is built), so give each tree a warm-up cell before the cells it compares; each cell's
# summary shows its `kernel.ready` and plan reports.
# params:
#   cells      [{"name", "tree" (a packages_refs name; absent, `target` is a published `<package>/<function>[@release]`,
#              run from the packages directory: an unchanged release on a candidate Runtime), "target"
#              ("<package>/<function>"), "args" ([inputs and flags]),
#              "update" (optional: {"runtime", "tensorfs"}, each a wheel URL, `artifact:<run id>/<name>` of a
#              cozy-creator run (a dev-build's tensorfs-wheel-<sha>) or, for runtime, a cozy-runtime dev-build
#              run id, and "gpu_memory_limit" (`10GiB`, `off`: the development VRAM cap); the pod's `machine update`
#              before this cell, so cells before and after compare two pairs)}];
#              "expect" (optional: the status the cell must end in, `completed` by default; `failed` for a refusal it proves);
#              "download" (optional: [[model ref, `cozy model install` flags…], …] made present on the pod first, so
#              the cell's run starts with its weights on disk, as a machine whose package was installed earlier does);
#              in args, `{ref}` is release-gate/ref and `{out:<cell>}` an earlier cell's image or video;
#              two failed cells stop the session
#   compare    [["A", "B"], …]: the result digests, else the decoded frames of every stream, and the video's PSNR
#   need_s     seconds the cap must buy (3600)
#   free_gib   optional: before the first cell another program (suites/bench/vram.py, raw driver allocations by a
#              root shell over ssh) takes all of GPU 0 but this many GiB and holds it to the end, a card as full as
#              the 0.25.0 gate's RTX 5090 kazune was (about 3 GiB outside cozy; cr-293)
. "$(dirname "$0")/lib.sh"
CELLS=$(param cells '[]') COMPARE=$(param compare '[]') FREE_GIB=$(param free_gib '')
[[ -z $FREE_GIB || $FREE_GIB =~ ^[0-9]+([.][0-9]+)?$ ]] || fail "free_gib $FREE_GIB"
VRAM=$(dirname "$0")/../suites/bench/vram.py
if [ -n "$FREE_GIB" ]; then
  [ -f "$VRAM" ] || fail "no suites/bench/vram.py"
  [ -f ~/.ssh/id_ed25519.pub ] || ssh-keygen -q -t ed25519 -N '' -f ~/.ssh/id_ed25519 || fail "ssh-keygen"
  RENT_ARGS=--ssh
fi
command -v ffmpeg > /dev/null || fail "no ffmpeg (pod-session.yaml installs it from the tool cache)"
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
# Each update's wheels, fetched before renting so a missing one costs nothing.
wheel() {
  local d=$OUT/wheels/$1-$2; mkdir -p "$d"
  case $3 in
    https://*) curl -fsSL --retry 5 -o "$d/${3##*/}" "$3" || return 1 ;;
    artifact:*/*) local a=${3#artifact:}; gh run download "${a%%/*}" -R cozy-creator/cozy-creator -n "${a#*/}" -D "$d" >/dev/null 2>&1 || return 1 ;;
    *) gh run download "$3" -R cozy-creator/cozy-runtime -p 'cozy-runtime-*' -D "$d" >/dev/null 2>&1 || return 1 ;;
  esac
  find "$d" -name '*manylinux*x86_64*.whl' | head -1
}
declare -A UPDATE=()
for k in "${names[@]}"; do
  w=()
  for kind in runtime tensorfs; do
    src=$(jq -r --arg k "$k" --arg t "$kind" 'map(select(.name==$k)) | first | .update[$t] // empty' <<<"$CELLS"); [ -n "$src" ] || continue
    f=$(wheel "$k" "$kind" "$src") && [ -n "$f" ] || fail "cell $k: no $kind wheel from $src"
    say "cell $k: updates $kind to ${f##*/}"; w+=("--$kind" "$f")
  done
  cap=$(jq -r --arg k "$k" 'map(select(.name==$k)) | first | .update.gpu_memory_limit // empty' <<<"$CELLS")
  [[ -z $cap || $cap =~ ^([0-9]+[MG]iB|off)$ ]] || fail "cell $k: gpu_memory_limit $cap"
  [ -z "$cap" ] || { say "cell $k: caps VRAM at $cap"; w+=(--gpu-memory-limit "$cap"); }
  [ "${#w[@]}" = 0 ] || UPDATE[$k]="${w[*]}"
done
printf 'run 5 · sync local/x (source changed)\nrun 6 · local/x/f\nrun      6\n' > "$OUT/probe-cell.txt"
current() { awk '$1=="run" && $3=="·" && $4!="sync"{n=$2} END{print n}' "$OUT/$1.txt" 2>/dev/null; }
[ "$(runno probe-cell)" = 6 ] && [ "$(current probe-cell)" = 6 ] || fail "parsers"
quote "$(param need_s 3600)"
rent


echo -e "utc\tshow_ms\tshow_rc\tstate\trun\trunshow_ms\trunshow_rc\tstatus\tunreachable" > "$OUT/probe.tsv"
( while :; do
    t=$(date +%s%3N); s=$(timeout 120 cozy machine show "$RID" --json 2>&1); src=$?; sm=$(( $(date +%s%3N) - t ))
    k=$(cat "$OUT/current" 2>/dev/null); N=''; [ -z "$k" ] || N=$(current "$k")
    rs='' rms='' rrc='' rst=''
    if [ -n "$N" ]; then t=$(date +%s%3N); rs=$(timeout 120 cozy run watch "$N" --once --json --full 2>&1); rrc=$?; rms=$(( $(date +%s%3N) - t ))
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
    script='[ -s /var/tmp/ab-vram.ready ] && kill "$(cut -d" " -f1 /var/tmp/ab-vram.ready)"; rm -f /var/tmp/ab-vram.ready; sleep 2'
  else
    script=$(cat <<EOS
echo $(base64 -w0 "$VRAM") | base64 -d > /var/tmp/ab-vram.py && rm -f /var/tmp/ab-vram.ready
setsid nohup python3 /var/tmp/ab-vram.py $(awk -v g="$FREE_GIB" 'BEGIN { printf "%d", g * 1073741824 }') /var/tmp/ab-vram.ready > /var/tmp/ab-vram.out 2>&1 < /dev/null &
i=0; while [ ! -s /var/tmp/ab-vram.ready ] && kill -0 \$! 2> /dev/null && [ \$i -lt 240 ]; do sleep 0.5; i=\$((i + 1)); done
cat /var/tmp/ab-vram.ready 2> /dev/null || tail -c 300 /var/tmp/ab-vram.out
EOS
)
  fi
  cozy machine ssh "$NAME" "echo $(base64 -w0 <<< "$script") | base64 -d | sh" > "$OUT/hold-$1-$2.txt" 2>&1
  [ "$2" = on ] || return 0
  read -r _ held free _ < <(tail -1 "$OUT/hold-$1-$2.txt")
  [[ ${held:-x} =~ ^[0-9]+$ ]] || fail "$1: no holder: $(head -c 300 "$OUT/hold-$1-$2.txt")"
  say "$1: holder $((held >> 20)) MiB held, $((free >> 20)) MiB left free"
}
fails=0
declare -A MADE=()
for k in "${names[@]}"; do
  c=$(jq -c --arg k "$k" 'map(select(.name==$k)) | first' <<<"$CELLS"); mapfile -t args < <(jq -r '.args[]' <<<"$c")
  for i in "${!args[@]}"; do
    a=${args[$i]//\{ref\}/$REF}
    while [[ $a =~ \{out:([A-Za-z0-9_-]+)\} ]]; do
      [ -n "${MADE[${BASH_REMATCH[1]}]:-}" ] || fail "cell $k: ${BASH_REMATCH[1]} made no image or video"
      a=${a//"${BASH_REMATCH[0]}"/"${MADE[${BASH_REMATCH[1]}]}"}
    done
    args[$i]=$a
  done
  if [ -n "${UPDATE[$k]:-}" ]; then
    [ -z "$FREE_GIB" ] || [ "$k" = "${names[0]}" ] || hold "$k" off
    # shellcheck disable=SC2086 # the update's flags and wheel paths (no spaces)
    step "update-$k" machine update "$NAME" ${UPDATE[$k]} -y || fail "$k: the update"
    say "$k: pod Runtime $(cozy machine show "$NAME" --json | jq -r .status.runtime), TensorFS $(cozy machine show "$NAME" --json | jq -r .status.tensorfs)"
  fi
  [ -z "$FREE_GIB" ] || { [ -z "${UPDATE[$k]:-}" ] && [ "$k" != "${names[0]}" ]; } || hold "$k" on
  mapfile -t downloads < <(jq -c '.download // [] | .[]' <<<"$c")
  for i in "${!downloads[@]}"; do
    mapfile -t dl < <(jq -r '.[]' <<<"${downloads[$i]}")
    step "download-$k-$i" model install "${dl[@]}" --on "$NAME" < /dev/null || fail "$k: the download of ${dl[0]}"
  done
  echo "$k" > "$OUT/current"
  tree=$(jq -r '.tree // empty' <<<"$c") target=$(jq -r .target <<<"$c")
  cd "$PACKAGES/$tree" || fail "no tree for $k"
  [ -z "$tree" ] || target=./$target
  step "$k" run "$target" "${args[@]}" --on "$NAME"; code=$?
  cd "$OUT" || fail "no $OUT"
  N=$(runno "$k")
  if [ -n "$N" ]; then cozy run watch "$N" --once --json --full > "$OUT/$k.json" 2>/dev/null; cozy run watch "$N" --once -o "$OUT/$k.out" > /dev/null 2>&1; fi
  st=$(jq -r '.status // "none"' "$OUT/$k.json" 2>/dev/null); st=${st:-none}
  MADE[$k]=$(ls "$OUT/$k.out"/* 2> /dev/null | grep -E '[.](png|webp|jpg|mp4)$' | sort | head -1)
  say "$k run ${N:-none} ${st:-none} (exit $code): $(summary "$k")"
  say "$k weights: $(weights "$k")"
  want=$(jq -r '.expect // "completed"' <<<"$c")
  if [ "$st" != completed ]; then
    why=$(jq -r '.reason.message // .reason // empty' "$OUT/$k.json" 2>/dev/null)
    say "$k reason: $(printf '%s' "${why:-$(tr '\r' '\n' < "$OUT/$k.txt" | grep -m1 -E '^error:' || tail -c 600 "$OUT/$k.txt")}" | head -c 600)"
  fi
  # A refusal before any run (st none) is the failure an `expect: failed` cell proves.
  [ "$st" = "$want" ] || { [ "$want" = failed ] && [ "$st" = none ] && [ "$code" != 0 ]; } ||
    { fails=$((fails + 1)); [ "$fails" -lt 2 ] || fail "two cells failed"; }
done
rm -f "$OUT/current"

digests() { jq -r '[.result | .. | .digest? // empty] | join(",")' "$OUT/$1.json" 2>/dev/null; }
file() { ls "$OUT/$1.out"/* 2> /dev/null | grep -E '\.(mp4|webm|png|webp|jpg|wav|flac)$' | head -1; }
frames() { local f; f=$(file "$1"); [ -s "$f" ] || { echo none; return; }; ffmpeg -nostdin -v error -i "$f" -map 0 -f framemd5 - | grep -v '^#' | md5sum | cut -c1-16; }
psnr() { ffmpeg -nostdin -i "$(file "$1")" -i "$(file "$2")" -lavfi '[0:v][1:v]psnr' -f null - 2>&1 | grep -oE 'average:[0-9.inf]+' | tail -1; }
differ=0
while read -r a b; do
  da=$(digests "$a") db=$(digests "$b")
  if [ -n "$da" ] && [ "$da" = "$db" ]; then say "$a vs $b: IDENTICAL bytes ($da)"; continue; fi
  fa=$(frames "$a") fb=$(frames "$b")
  if [ "$fa" != none ] && [ "$fa" = "$fb" ]; then say "$a vs $b: IDENTICAL decoded frames ($fa); files differ ($da / $db)"; continue; fi
  differ=$((differ + 1)); say "$a vs $b: DIFFERENT: digests $da / $db, frames $fa / $fb, video PSNR $(psnr "$a" "$b")"
done < <(jq -r '.[] | "\(.[0]) \(.[1])"' <<<"$COMPARE")
u=0; for k in "${names[@]}"; do x=$(grep -ci unreachable "$OUT/$k.txt" 2>/dev/null); u=$((u + ${x:-0})); done
say "unreachable lines in the cells' follow output: $u"
awk -F'\t' 'NR>1{n++; if($2>m)m=$2; if($3!=0)e++; if($6!=""){r++; if($6>q)q=$6; if($7!=0)f++}; u+=$9}
  END{printf "probe: %d machine show (max %d ms, %d failed), %d run watch (max %d ms, %d non-zero), %d unreachable\n", n, m, e, r, q, f, u}' "$OUT/probe.tsv" | tee -a "$R"
[ "$fails" = 0 ] && [ "$differ" = 0 ] || fail "$fails cells did not complete, $differ pairs differ"
