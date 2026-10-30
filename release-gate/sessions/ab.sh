# ab: one rental, a list of cells (each one run of a package function from a packages checkout, run in order), and
# pairs of cells compared for identical output: a refactor's proof is the same request giving the same bytes (xs-101).
# While cells run, `machine show` and the current run's `run show --once` are timed every 15 s (probe.tsv): a busy
# machine keeps answering and is never "unreachable" (cl-671).
# A pod's first call of each tree serves what has not compiled yet (its LoRA per step until the bake kernel is ready,
# a fused block eager until it is built), so give each tree a warm-up cell before the cells it compares; each cell's
# summary shows its `kernel.ready` and plan reports.
# params:
#   cells      [{"name", "tree" (a packages_refs name), "target" ("<package>/<function>"), "args" ([inputs and flags]),
#              "update" (optional: {"runtime", "tensorfs"}, each a wheel URL or, for runtime, a cozy-runtime dev-build
#              run id; the pod's `machine update` before this cell, so cells before and after compare two pairs)}];
#              "expect" (optional: the status the cell must end in, `completed` by default; `failed` for a refusal it proves);
#              in args, `{ref}` is release-gate/ref and `{out:<cell>}` an earlier cell's image or video;
#              two failed cells stop the session
#   compare    [["A", "B"], …]: the result digests, else the decoded frames of every stream, and the video's PSNR
#   traffic_gb GB the provider bills: the image, the models, the outputs (100)
#   need_s     seconds the cap must buy (3600)
. "$(dirname "$0")/lib.sh"
CELLS=$(param cells '[]') COMPARE=$(param compare '[]')
command -v ffmpeg >/dev/null || sudo apt-get install -y -qq ffmpeg >/dev/null 2>&1 || fail "no ffmpeg"
mapfile -t names < <(jq -r '.[].name' <<<"$CELLS")
[ "${#names[@]}" -gt 0 ] || fail "no cells"
for k in "${names[@]}"; do
  c=$(jq -c --arg k "$k" 'map(select(.name==$k)) | first' <<<"$CELLS")
  [[ $k =~ ^[A-Za-z0-9_-]+$ ]] && [ "$(jq --arg k "$k" 'map(select(.name==$k)) | length' <<<"$CELLS")" = 1 ] || fail "cell names must be unique words: $k"
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
  [ "${#w[@]}" = 0 ] || UPDATE[$k]="${w[*]}"
done
printf 'run 5 · sync local/x (source changed)\nrun 6 · local/x/f\nrun      6\n' > "$OUT/probe-cell.txt"
current() { awk '$1=="run" && $3=="·" && $4!="sync"{n=$2} END{print n}' "$OUT/$1.txt" 2>/dev/null; }
[ "$(runno probe-cell)" = 6 ] && [ "$(current probe-cell)" = 6 ] || fail "parsers"
quote "$(param traffic_gb 100)" "$(param need_s 3600)"
rent

echo -e "utc\tshow_ms\tshow_rc\tstate\trun\trunshow_ms\trunshow_rc\tstatus\tunreachable" > "$OUT/probe.tsv"
( while :; do
    t=$(date +%s%3N); s=$(timeout 120 cozy machine show "$RID" --json 2>&1); src=$?; sm=$(( $(date +%s%3N) - t ))
    k=$(cat "$OUT/current" 2>/dev/null); N=''; [ -z "$k" ] || N=$(current "$k")
    rs='' rms='' rrc='' rst=''
    if [ -n "$N" ]; then t=$(date +%s%3N); rs=$(timeout 120 cozy run show "$N" --once --json 2>&1); rrc=$?; rms=$(( $(date +%s%3N) - t ))
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
  + "plan \([$m.execution.plan_reports[]? | to_entries[] | "\(.key)=\(.value.applied)\(if (.value.code // "") != "" then "(\(.value.code))" else "" end)"] | join(" "))"] | join(""))' "$OUT/$1.json" 2>/dev/null; }
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
    # shellcheck disable=SC2086 # the update's flags and wheel paths (no spaces)
    step "update-$k" machine update "$NAME" ${UPDATE[$k]} -y || fail "$k: the update"
    say "$k: pod Runtime $(cozy machine show "$NAME" --json | jq -r .status.runtime), TensorFS $(cozy machine show "$NAME" --json | jq -r .status.tensorfs)"
  fi
  echo "$k" > "$OUT/current"
  cd "$PACKAGES/$(jq -r .tree <<<"$c")" || fail "no tree for $k"
  step "$k" run "./$(jq -r .target <<<"$c")" "${args[@]}" --on "$NAME" --out "$OUT/out/$k"; code=$?
  cd "$OUT" || fail "no $OUT"
  N=$(runno "$k")
  if [ -n "$N" ]; then cozy run show "$N" --once --json > "$OUT/$k.json" 2>/dev/null; fi
  st=$(jq -r '.status // "none"' "$OUT/$k.json" 2>/dev/null)
  MADE[$k]=$(find "$OUT/out/$k" -type f \( -name '*.png' -o -name '*.webp' -o -name '*.jpg' -o -name '*.mp4' \) 2>/dev/null | sort | head -1)
  say "$k run ${N:-none} ${st:-none} (exit $code): $(summary "$k")"
  want=$(jq -r '.expect // "completed"' <<<"$c")
  [ "$st" = completed ] || say "$k reason: $(jq -r '.reason.message // .reason // "none"' "$OUT/$k.json" 2>/dev/null | head -c 600)"
  [ "$st" = "$want" ] || { fails=$((fails + 1)); [ "$fails" -lt 2 ] || fail "two cells failed"; }
done
rm -f "$OUT/current"

digests() { jq -r '[.result | .. | .digest? // empty] | join(",")' "$OUT/$1.json" 2>/dev/null; }
file() { jq -r '.output[]?.path' "$OUT/$1.json" 2>/dev/null | grep -E '\.(mp4|webm|png|webp|jpg|wav|flac)$' | head -1; }
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
  END{printf "probe: %d machine show (max %d ms, %d failed), %d run show (max %d ms, %d non-zero), %d unreachable\n", n, m, e, r, q, f, u}' "$OUT/probe.tsv" | tee -a "$R"
[ "$fails" = 0 ] && [ "$differ" = 0 ] || fail "$fails cells did not complete, $differ pairs differ"
