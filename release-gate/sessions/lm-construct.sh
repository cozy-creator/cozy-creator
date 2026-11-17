# lm-construct (cr-307): Qwen3.6-35B-A3B's start-up on one rental: download, executor, construction, the load it answers from.
# cr-307, from xs-204's GPU proof: Qwen3.6-35B-A3B (int8) took 3.7-7 min to construct its executor on an RTX A6000 before it
# answered anything (PRO-4: prepare 223 s, `construct.fill` 209 s; a fresh pod's render 342 s; a second executor ~5.5 min).
# Paul 11-15: "fix known problems now". The coordinator 11-17: the same bookkeeping serves every model with many blocks, so
# SDXL runs beside it as the non-LLM twin.
# The first arm's Qwen3.6 run downloads the weights (`<arm>-first`). Then per arm, in order on the one pod (its Runtime and
# TensorFS by `cozy machine update`, which restarts its TensorD):
#   cold   Qwen3.6's `write` (the packages checkout's) on a photo, greedy, a new executor, its weights out of the page cache
#   warm   the same on a new executor (the package uninstalled first), its weights in the page cache
#   hot    the same executor again
#   sdxl   SDXL's generate (1 MP, 20 steps) on a new executor, then again on it with another seed
#   capped with `capped`, Qwen-Image 2's generate (1 MP, no upsampling) under that VRAM cap on a new executor: blocks
#          paged per step, the varena-heavy twin (cr-318)
#   paged  with `paged`, Qwen3.6 under that VRAM cap on a new executor: experts paged every token, its decode's
#          refills the engine's demand and ahead reads side by side (cr-318 step 2)
#   fault  with `fault` too, on that executor: Qwen3.6 again, and once it decodes every fourth chunk of the checkpoint is
#          emptied in place (a refill reading one fails while the decode's other reads are in flight), then restored, and
#          Qwen3.6 once more: it must answer as `paged` did (no read's staging crossed into another's)
# Every row's parts against the ceilings TensorFS measures on the pod (disk, pinned H2D) are in table.txt and the job summary.
# A judged arm must construct warm in `prepare_s` and run cold in `x` times its weights' read at the disk's ceiling plus
# `fixed_s` (legs.py): the default bar, 40 s and 2 x + 60 s, is 71 s on 44058999272's A6000 (6.78 GB/s), where the fixed
# Runtime ran 42 s and the RC 328 s.
# params:
#   tree     the packages_refs checkout qwen3.6 and sdxl run from (required)
#   arms     [{"name": "rc", "runtime": "<cozy-runtime run id or https wheel>", "tensorfs": "<artifact:<run>/<name> or https
#            wheel>", "judged": false}, {"name": "fix", ..., "judged": true}]
#   bar      {"prepare_s": 40, "x": 2, "fixed_s": 60}
#   sdxl     false: no SDXL rows
#   capped   a VRAM cap (`12GiB`): the capped Qwen-Image 2 row per arm
#   paged    a VRAM cap (`28GiB`): the paged Qwen3.6 row per arm
#   fault    true (with `paged`): the fault rows per arm
#   need_s   seconds the guard must buy (default 3600)
. "$(dirname "$0")/lib.sh"
HERE=$(cd "$(dirname "$0")" && pwd)
FIX=$HERE/fixtures/lm-construct PICTURE=$HERE/fixtures/ltx-images/teapot.jpg
TREE=$(param tree '') ARMS=$(param arms '[]') BAR=$(param bar '{"prepare_s": 40, "x": 2, "fixed_s": 60}')
[ -n "$TREE" ] && [ -f "$PACKAGES/$TREE/qwen3.6/pyproject.toml" ] && [ -f "$PACKAGES/$TREE/sdxl/pyproject.toml" ] &&
  [ -f "$PACKAGES/$TREE/qwen-image-2/pyproject.toml" ] ||
  fail "dispatch with -f packages_refs=<name>=<the packages RC> and params.tree=<name>"
SOURCE=$(cd "$PACKAGES/$TREE" && pwd)
mapfile -t names < <(jq -r '.[].name' <<<"$ARMS")
[ "${#names[@]}" -gt 0 ] || fail "no arms"
for tool in jq python3; do command -v "$tool" > /dev/null || fail "no $tool"; done
V=$(cozy --version | awk '{print $2}')
[[ $V == *-dev* ]] || fail "cozy $V is no RC build: dispatch with cozy=<a dev-build of rc/<x>>"
# Each arm's wheels, fetched before renting.
declare -A RT=() TF=()
for a in "${names[@]}"; do
  [[ $a =~ ^[a-z0-9-]+$ ]] || fail "arm name $a"
  RT[$a]=$(wheel_from "$OUT/wheels/$a-runtime" "$(jq -r --arg a "$a" '.[] | select(.name == $a) | .runtime' <<<"$ARMS")")
  TF[$a]=$(wheel_from "$OUT/wheels/$a-tensorfs" "$(jq -r --arg a "$a" '.[] | select(.name == $a) | .tensorfs' <<<"$ARMS")")
  [ -n "${RT[$a]}" ] && [ -n "${TF[$a]}" ] || fail "arm $a: no Runtime or TensorFS wheel"
  say "arm $a: ${RT[$a]##*/}, ${TF[$a]##*/}"
done
jq -n --argjson arms "$ARMS" --argjson bar "$BAR" '[$arms[] | {key: .name, value: ($bar + {judged: (.judged // false)})}] | from_entries' \
  > "$OUT/bar.json" || fail "bar: $BAR"
CAPPED=$(param capped '') PAGED=$(param paged '') FAULT=$(param fault false)
[[ -z $CAPPED || $CAPPED =~ ^[0-9]+GiB$ ]] && [[ -z $PAGED || $PAGED =~ ^[0-9]+GiB$ ]] || fail "capped $CAPPED, paged $PAGED"
kernels "$SOURCE/qwen3.6"; kernels "$SOURCE/sdxl"; [ -z "$CAPPED" ] || kernels "$SOURCE/qwen-image-2"
[ -f ~/.ssh/id_ed25519.pub ] || ssh-keygen -q -t ed25519 -N '' -f ~/.ssh/id_ed25519 || fail "ssh-keygen"
RENT_ARGS=--ssh
quote "$(param need_s 3600)"
rent
pod() { timeout "${T:-120}" cozy machine ssh "$NAME" "$@"; }
for i in $(seq 1 20); do pod true < /dev/null > /dev/null 2>&1 && break; [ "$i" = 20 ] && fail "no SSH into $NAME"; sleep 15; done
T=60 pod "cat > /tmp/lm.py" < "$FIX/pod.py" || fail "pod.py"

run_page import "$PICTURE" --name photo > "$OUT/import-photo.json" 2>&1 || fail "import the photo: $(tail -c 300 "$OUT/import-photo.json")"
photo=$(jq -r '.sha256 // empty' "$OUT/import-photo.json"); [ -n "$photo" ] || fail "the import holds no file"
ON_PHOTO=$(jq -cn --arg f "sha256:${photo#sha256:}" '{text: "What is in <photo>? Answer in two sentences.", references: [{key: "photo", type: "image", file: $f}]}')
# Each image row its own seed: a call answered from the memo (the same code and inputs) would time nothing.
qimg() { jq -n --argjson s "$1" '{prompt: {text: "a lighthouse keeper'"'"'s desk at dawn, watercolor"}, megapixels: 1, prompt_upsampling: false, seed: $s}' > "$OUT/qimg-$1.json"; echo "$OUT/qimg-$1.json"; }
sdxl() { jq -n --argjson s "$1" '{prompt: "a lighthouse keeper'"'"'s desk at dawn, watercolor", megapixels: 1.0, aspect_ratio: "1:1", steps: 20, seed: $s}' > "$OUT/sdxl-$1.json"; echo "$OUT/sdxl-$1.json"; }
RECORDS=()
# record <label>: the run its step named, its whole record as <label>.json.
record() {
  local n; n=$(runno "$1")
  [ -n "$n" ] && (cd "$RUN_PROJECT" && cozy --json --full run watch "$n" --once) > "$OUT/$1.json" 2> "$OUT/$1.err"
  RECORDS+=("$1=$OUT/$1.json")
  say "$1: run ${n:-none} $(jq -r '.status // "unrecorded"' "$OUT/$1.json" 2> /dev/null), executing $(jq -r '.execution_ms // "?"' "$OUT/$1.json" 2> /dev/null) ms, prepare $(jq -r '.measurements.execution.construction.prepare_ms // "?"' "$OUT/$1.json" 2> /dev/null) ms: $(jq -r '.result.text // empty' "$OUT/$1.json" 2> /dev/null | tr '\n' ' ' | cut -c1-200)"
}
# write <label>: Qwen3.6 answers the photo, greedily.
write() {
  step "$1" run "$SOURCE/qwen3.6/write" "instructions:=$ON_PHOTO" max_tokens:=200 temperature:=0 --on "$NAME"
  record "$1"
}
# fresh <package>: its executor ends: the package uninstalled (the next run installs it from the checkout again), else every
# executor on the pod ended by its pid.
fresh() {
  local p
  p=$(cozy package ls --on "$NAME" --json 2> /dev/null | jq -r --arg p "$1" '[.packages[] | .package | select(. == $p or endswith("/" + $p))] | first // empty')
  [ -n "$p" ] && step "rm-$1" package uninstall "$p" --on "$NAME" -y && return 0
  say "fresh $1: not uninstalled (${p:-not listed}); ending the pod's executors: $(T=60 pod "python3 /tmp/lm.py end" < /dev/null 2>&1)"
}
# fault <arm>: Qwen3.6 on the paged executor, its checkpoint's middle chunk emptied once it decodes, then restored; then
# Qwen3.6 again, which must answer as <arm>-paged did.
fault() {
  local a=$1 ck n i stage p
  ck=$(jq -r '.measurements.models[0].manifest // empty' "$OUT/$a-paged.json" 2> /dev/null)
  [ -n "$ck" ] || { say "FAIL $a-fault: no checkpoint in $a-paged's record"; RC=1; return; }
  (step "$a-fault" run "$SOURCE/qwen3.6/write" "instructions:=$ON_PHOTO" max_tokens:=200 temperature:=0 --on "$NAME") & p=$!
  local since=''
  for i in $(seq 900); do
    n=$(runno "$a-fault")
    [ -z "$n" ] || since=${since:-$SECONDS}
    [ -n "$n" ] && stage=$( (cd "$RUN_PROJECT" && cozy --json run watch "$n" --once) 2> /dev/null | jq -r '.progress.stage // .status // empty')
    # Decoding: its stage says so, or 30 s since it began (construction ~15 s, then the paged decode for ~35 s).
    [[ ${stage:-} == answering* ]] && break
    [ -n "$since" ] && [ $((SECONDS - since)) -ge 30 ] && break
    kill -0 "$p" 2> /dev/null || break
    sleep 1
  done
  local broke
  broke=$(T=300 pod "python3 /tmp/lm.py break $ck" < /dev/null 2>&1)
  say "$a-fault: run ${n:-none} at '${stage:-?}': $broke"
  wait "$p"
  [ "$(jq -r '.emptied // 0' <<<"$broke" 2> /dev/null)" -gt 0 ] || { say "FAIL $a-fault: no chunk was emptied"; RC=1; }
  record "$a-fault"
  say "$a-fault: restored $(T=300 pod "python3 /tmp/lm.py restore $ck" < /dev/null 2>&1)"
  write "$a-after"
  local want got
  want=$(jq -r '.result.text // empty' "$OUT/$a-paged.json" 2> /dev/null) got=$(jq -r '.result.text // empty' "$OUT/$a-after.json" 2> /dev/null)
  if [ -n "$want" ] && [ "$want" = "$got" ]; then
    say "PASS $a-fault: $(jq -r '.status' "$OUT/$a-fault.json" 2> /dev/null) ($(jq -r '.error.code // .error // "no error"' "$OUT/$a-fault.json" 2> /dev/null | head -c 120)), then $a-after answered as $a-paged did"
  else
    say "FAIL $a-fault: $a-after answered '$(head -c 120 <<<"$got")', $a-paged '$(head -c 120 <<<"$want")'"; RC=1
  fi
}
ceiling() {
  T=600 pod "python3 /tmp/lm.py ceiling" < /dev/null > "$OUT/ceiling.json" 2> "$OUT/ceiling.err"
  say "ceiling: $(cat "$OUT/ceiling.json")"
}

k=0
for a in "${names[@]}"; do
  step "update-$a" machine update "$NAME" --tensord "$(command -v cozy)" --runtime "${RT[$a]}" --tensorfs "${TF[$a]}" -y || fail "$a: the update"
  say "$a: $(cozy machine show "$NAME" --json | jq -r '"cozy \(.status.version) · Runtime \(.status.runtime) · TensorFS \(.status.tensorfs) · GPU 0 \(.status.gpus[0].name)"')"
  if [ "$a" = "${names[0]}" ]; then
    write "$a-first"  # the download, and the executor environment the ceiling is measured from
    ceiling
    fresh qwen3.6
    if [ -n "$CAPPED" ]; then  # its download and first construction's caches, which no arm's row should carry
      step "$a-qimg-first" run "$SOURCE/qwen-image-2/generate" --input "$(qimg 99)" --on "$NAME"; record "$a-qimg-first"
      fresh qwen-image-2
    fi
  fi
  T=300 pod "python3 /tmp/lm.py evict" < /dev/null > "$OUT/$a-evict.json" 2>&1; say "$a: evicted $(cat "$OUT/$a-evict.json")"
  write "$a-cold"
  fresh qwen3.6
  write "$a-warm"
  write "$a-hot"
  if [ "$(param sdxl true)" = true ]; then
    fresh qwen3.6
    step "$a-sdxl-new" run "$SOURCE/sdxl/generate" --input "$(sdxl $((100 + k)))" --on "$NAME"; record "$a-sdxl-new"
    step "$a-sdxl-warm" run "$SOURCE/sdxl/generate" --input "$(sdxl $((200 + k)))" --on "$NAME"; record "$a-sdxl-warm"
  fi
  if [ -n "$CAPPED" ]; then
    step "cap-$a" machine update "$NAME" --gpu-memory-limit "$CAPPED" -y || fail "$a: the cap"
    step "$a-qimg-capped" run "$SOURCE/qwen-image-2/generate" --input "$(qimg $((300 + k)))" --on "$NAME"; record "$a-qimg-capped"
    step "uncap-$a" machine update "$NAME" --gpu-memory-limit off -y || fail "$a: lifting the cap"
  fi
  if [ -n "$PAGED" ]; then
    step "page-$a" machine update "$NAME" --gpu-memory-limit "$PAGED" -y || fail "$a: the cap"
    write "$a-paged"
    [ "$FAULT" != true ] || fault "$a"
    step "unpage-$a" machine update "$NAME" --gpu-memory-limit off -y || fail "$a: lifting the cap"
  fi
  k=$((k + 1))
done

cozy machine logs "$NAME" --since 3h -n 0 > "$OUT/machine.log" 2>&1
grep -aE 'floor moved .*to=none' "$OUT/machine.log" | awk '{for (i = 1; i <= NF; i++) if ($i ~ /^(bytes|seconds|mb_s)=/) printf "%s ", $i; print ""}' |
  awk '{split($1, b, "="); if (b[2] > 1e9) print "download: " $0}' | tee -a "$R"
jq -e .disk_gbps "$OUT/ceiling.json" > /dev/null 2>&1 || echo '{}' > "$OUT/ceiling.json"
python3 -I "$FIX/legs.py" "$OUT/ceiling.json" "$OUT/bar.json" "${RECORDS[@]}" > "$OUT/table.txt" 2>&1 || RC=1
grep -v '^{' "$OUT/table.txt" | tee -a "$R"
{ echo "## lm-construct on $NAME ($SKU)"; echo; echo "ceiling: $(cat "$OUT/ceiling.json")"; echo; grep -v '^{' "$OUT/table.txt"; } >> "${GITHUB_STEP_SUMMARY:-/dev/null}"
exit "$RC"
