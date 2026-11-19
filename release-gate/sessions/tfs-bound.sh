# tfs-bound (cl-1318): TensorFS's load of a model against its machine's bound, cold from disk and from RAM, per TensorFS.
# Paul 11-19: "loading times for TensorFS should approach theoretical maximums"; "if the disk / PCIe would support us getting
# it in there in 5 seconds, we should get it in there as close to 5 seconds as possible". The bound (product decision "A model
# loads as fast as its machine allows"): max(bytes / disk GB/s, bytes / pinned H2D GB/s); from RAM, the H2D part alone.
# On one rental: Qwen-Image 2's generate (its weights all resident on a 24 GB card), SDXL's, and with `h3` H3's (its transformer
# paged on a 24 GB card) download their weights and make the executor environment. Then, before any arm:
#   ceiling  TensorFS's own ceilings on the pod (fixtures/tfs-bound/load.py)
#   disk     the disk without TensorFS: the models' chunk files read O_DIRECT over a grid of threads x request sizes, and
#            buffered from an empty page cache: what the disk itself reaches
# Per arm (its TensorFS by `cozy machine update`, and unpacked beside the executor environment for load.py), every executor
# ended first so the card is free:
#   fill     Qwen-Image 2's every component, one Region a block, by one `varena.fill`: evicted (from disk), then again (RAM)
#   h3-fill  the same for H3's transformer;  h3-paged: its blocks read ahead into pinned pages, then onto the card from them
#   sweep    each of the arm's `sweep` env sets, Qwen-Image 2's fill again, and each `paged_sweep` set H3's paged rows
#            again (a build that reads them: a proof's own)
#   runs     unless `"runs": false`: Qwen-Image 2 cold (evicted, new executor) and warm (new executor), SDXL on a new executor
#            and again, and with `h3` H3 Turbo 1 s cold and warm: every row's store read against the ceilings
# A measurement another session's run overlapped (cl-1316: a pinned run sent to this pod) is marked `overlapped` and not judged.
# params:
#   tree    the packages_refs checkout the packages run from (required)
#   arms    [{"name": "rc", "tensorfs": "<artifact:<run>/<name> or https wheel>", "sweep": [{"TFS_X_…": "…"}], "runs": true}, …]
#   runtime the pod's Runtime (a cozy-runtime run id or https wheel; default RUNTIME_WHEEL, from runtime_ref)
#   h3      true: H3's rows too (a 48 GB download)
#   need_s  seconds the cap must buy (default 4800)
. "$(dirname "$0")/lib.sh"
HERE=$(cd "$(dirname "$0")" && pwd)
FIX=$HERE/fixtures/tfs-bound
TREE=$(param tree '') ARMS=$(param arms '[]') H3=$(param h3 false)
[ -n "$TREE" ] && [ -f "$PACKAGES/$TREE/qwen-image-2/pyproject.toml" ] && [ -f "$PACKAGES/$TREE/sdxl/pyproject.toml" ] ||
  fail "dispatch with -f packages_refs=<name>=<the packages RC> and params.tree=<name>"
SOURCE=$(cd "$PACKAGES/$TREE" && pwd)
mapfile -t names < <(jq -r '.[].name' <<<"$ARMS")
[ "${#names[@]}" -gt 0 ] || fail "no arms"
for tool in jq python3; do command -v "$tool" > /dev/null || fail "no $tool"; done
RTW=$(param runtime "${RUNTIME_WHEEL:-}")
if [ -f "$RTW" ]; then RT=$RTW; else RT=$(wheel_from "$OUT/wheels/runtime" "$RTW"); fi
[ -n "$RT" ] && [ -f "$RT" ] || fail "no Runtime wheel ($RTW)"
declare -A TF=()
for a in "${names[@]}"; do
  [[ $a =~ ^[a-z0-9-]+$ ]] || fail "arm name $a"
  TF[$a]=$(wheel_from "$OUT/wheels/$a" "$(jq -r --arg a "$a" '.[] | select(.name == $a) | .tensorfs' <<<"$ARMS")")
  [ -n "${TF[$a]}" ] || fail "arm $a: no TensorFS wheel"
  say "arm $a: ${TF[$a]##*/}"
done
say "runtime: ${RT##*/}"
kernels "$SOURCE/qwen-image-2"; kernels "$SOURCE/sdxl"; [ "$H3" != true ] || kernels "$SOURCE/minimax-h3"
[ -f ~/.ssh/id_ed25519.pub ] || ssh-keygen -q -t ed25519 -N '' -f ~/.ssh/id_ed25519 || fail "ssh-keygen"
RENT_ARGS=--ssh
quote "$(param need_s 4800)"
rent
pod() { timeout "${T:-120}" cozy machine ssh "$NAME" "$@"; }
for i in $(seq 1 20); do pod true < /dev/null > /dev/null 2>&1 && break; [ "$i" = 20 ] && fail "no SSH into $NAME"; sleep 15; done
T=60 pod "cat > /tmp/load.py" < "$FIX/load.py" || fail "load.py"
T=60 pod "cat > /tmp/lm.py" < "$HERE/fixtures/lm-construct/pod.py" || fail "pod.py"
step update machine update "$NAME" --tensord "$(command -v cozy)" --runtime "$RT" --tensorfs "${TF[${names[0]}]}" -y || fail "the update"
say "pod: $(cozy machine show "$NAME" --json | jq -r '"cozy \(.status.version) · Runtime \(.status.runtime) · TensorFS \(.status.tensorfs) · GPU 0 \(.status.gpus[0].name)"')"
T=60 pod "nproc; free -g | sed -n 2p; df -h /var/lib/cozy | tail -1; lsblk -d -o NAME,MODEL,ROTA,SIZE 2>/dev/null | head -5; uname -r" < /dev/null > "$OUT/host.txt" 2>&1
say "host: $(tr '\n' ';' < "$OUT/host.txt" | cut -c1-400)"

# busy: runs and queued work on the pod now, every client's (cl-1316: another session's pinned run can land here).
busy() { cozy machine ls --json 2> /dev/null | jq -r --arg m "$NAME" '[.machines[] | select(.name == $m) | (.running + .queued)] | first // -1'; }
RECORDS=()
# record <label>: the run its step named, its whole record as <label>.json.
record() {
  local n; n=$(runno "$1")
  [ -n "$n" ] && (cd "$RUN_PROJECT" && cozy --json --full run watch "$n" --once) > "$OUT/$1.json" 2> "$OUT/$1.err"
  RECORDS+=("$1=$OUT/$1.json")
  say "$1: run ${n:-none} $(jq -r '.status // "unrecorded"' "$OUT/$1.json" 2> /dev/null), executing $(jq -r '.execution_ms // "?"' "$OUT/$1.json" 2> /dev/null) ms, store $(jq -c '[.report.measured[0].memory[0].store // empty] | first' "$OUT/$1.json" 2> /dev/null)"
}
# ckids <label>: the checkpoints its run read, comma-joined.
ckids() { jq -r '[.measurements.models[]?.manifest // empty] | unique | join(",")' "$OUT/$1.json" 2> /dev/null; }
seed=$((RANDOM * 7))
qimg() { seed=$((seed + 1)); jq -n --argjson s "$seed" '{prompt: {text: "a lighthouse keeper'"'"'s desk at dawn, watercolor"}, megapixels: 1, prompt_upsampling: false, seed: $s}' > "$OUT/qimg-$seed.json"; echo "$OUT/qimg-$seed.json"; }
sdxl() { seed=$((seed + 1)); jq -n --argjson s "$seed" '{prompt: "a lighthouse keeper'"'"'s desk at dawn, watercolor", megapixels: 1.0, aspect_ratio: "1:1", steps: 20, seed: $s}' > "$OUT/sdxl-$seed.json"; echo "$OUT/sdxl-$seed.json"; }
h3() { seed=$((seed + 1)); jq -n --argjson s "$seed" '{summary: {references: [], text: "A lighthouse on a rocky point at dusk, waves breaking below."},
  beats: [{duration: 1, description: {references: [], text: "The beam sweeps once across the water."}}], seed: $s}' > "$OUT/h3-$seed.json"; echo "$OUT/h3-$seed.json"; }
run_qimg() { step "$1" run "$SOURCE/qwen-image-2/generate" --input "$(qimg)" --on "$NAME"; record "$1"; }
run_sdxl() { step "$1" run "$SOURCE/sdxl/generate" --input "$(sdxl)" --on "$NAME"; record "$1"; }
run_h3() { step "$1" run "$SOURCE/minimax-h3/generate" --input "$(h3)" --on "$NAME"; record "$1"; }
# fresh: every executor on the pod ends, so the next run starts a new one and the card is free.
fresh() { say "fresh: $(T=60 pod "python3 /tmp/lm.py end" < /dev/null 2>&1 | tail -1)"; }
# load <label> <arm> <env> <args…>: load.py on the pod with the arm's TensorFS, its JSON as <label>.json.
load() {
  local label=$1 a=$2 env=$3; shift 3
  local before after
  before=$(busy)
  T=1800 pod "cd /tmp && $env PYTHONPATH=/tmp/tf-$a TENSORFS_HOME='$HOME_TFS' $PY /tmp/load.py $*" < /dev/null > "$OUT/$label.out" 2> "$OUT/$label.err"
  after=$(busy)
  tail -n 1 "$OUT/$label.out" | jq --arg b "$before" --arg a "$after" '. + {busy_before: ($b | tonumber? // -1), busy_after: ($a | tonumber? // -1)}' > "$OUT/$label.json" 2> /dev/null ||
    echo "{\"error\": \"no report\", \"stderr\": $(tail -c 600 "$OUT/$label.err" | jq -Rs .)}" > "$OUT/$label.json"
  [ "$before" = 0 ] && [ "$after" = 0 ] || say "$label: overlapped (the pod ran $before before, $after after)"
  say "$label: $(jq -c 'del(.trace) | del(.from)' "$OUT/$label.json" | cut -c1-900)"
}

# The weights and the executor environment, on the first arm.
run_qimg qimg-first; QCK=$(ckids qimg-first)
run_sdxl sdxl-first
HCK=''
if [ "$H3" = true ]; then run_h3 h3-first; HCK=$(ckids h3-first); fi
say "checkpoints: qwen-image-2 ${QCK:-none}, h3 ${HCK:-none}"
[ -n "$QCK" ] || fail "qimg-first named no checkpoint"
PY=$(T=60 pod "ls -t /var/lib/cozy/tensord/generations/*/env/bin/python | head -1" < /dev/null 2> /dev/null | tail -1)
# The store an executor reads (TensorD imposes TENSORFS_HOME on it): the first runs' executors are still up.
HOME_TFS=$(T=60 pod "for e in /proc/[0-9]*/environ; do tr '\\0' '\\n' < \$e 2>/dev/null | sed -n 's/^TENSORFS_HOME=//p'; done | sort -u | head -1" < /dev/null 2> /dev/null | tail -1)
[[ $PY == /var/lib/cozy/* ]] && [ -n "$HOME_TFS" ] || fail "no executor environment ($PY) or store ($HOME_TFS)"
say "executor python $PY, store $HOME_TFS"
for a in "${names[@]}"; do
  T=300 pod "rm -rf /tmp/tf-$a && mkdir -p /tmp/tf-$a && cat > /tmp/tf-$a.whl && $PY -m zipfile -e /tmp/tf-$a.whl /tmp/tf-$a" < "${TF[$a]}" ||
    fail "$a: the wheel to the pod"
done
fresh
load ceiling "${names[0]}" '' ceiling
ALLCK=$QCK${HCK:+,$HCK}
load disk "${names[0]}" '' disk "${ALLCK//,/ }"

k=0
for a in "${names[@]}"; do
  if [ "$k" -gt 0 ]; then
    step "update-$a" machine update "$NAME" --tensorfs "${TF[$a]}" -y || fail "$a: the update"
    say "$a: $(cozy machine show "$NAME" --json | jq -r '"TensorFS \(.status.tensorfs)"')"
  fi
  fresh
  load "$a-fill" "$a" '' fill "$QCK" transformer,text_encoder,vae
  if [ -n "$HCK" ]; then
    load "$a-h3-fill" "$a" '' fill "$HCK" transformer 3
    load "$a-h3-paged" "$a" '' paged "$HCK" transformer 3
  fi
  i=0
  while read -r env; do
    [ -n "$env" ] || continue
    i=$((i + 1)); load "$a-sweep-$i" "$a" "$env" fill "$QCK" transformer,text_encoder,vae
  done < <(jq -r --arg a "$a" '.[] | select(.name == $a) | (.sweep // [])[] | to_entries | map("\(.key)=\(.value)") | join(" ")' <<<"$ARMS")
  i=0
  while read -r env; do
    [ -n "$env" ] && [ -n "$HCK" ] || continue
    i=$((i + 1)); load "$a-paged-sweep-$i" "$a" "$env" paged "$HCK" transformer 3
  done < <(jq -r --arg a "$a" '.[] | select(.name == $a) | (.paged_sweep // [])[] | to_entries | map("\(.key)=\(.value)") | join(" ")' <<<"$ARMS")
  if [ "$(jq -r --arg a "$a" '.[] | select(.name == $a) | .runs // true' <<<"$ARMS")" = true ]; then
    fresh; load "$a-evict" "$a" '' evict "${ALLCK//,/ }"
    run_qimg "$a-qimg-cold"
    fresh; run_qimg "$a-qimg-warm"
    fresh; run_sdxl "$a-sdxl-new"; run_sdxl "$a-sdxl-warm"
    if [ -n "$HCK" ]; then
      fresh; load "$a-evict-h3" "$a" '' evict "${ALLCK//,/ }"
      run_h3 "$a-h3-cold"
      fresh; run_h3 "$a-h3-warm"
    fi
  fi
  k=$((k + 1))
done

python3 -I "$FIX/table.py" "$OUT" "${names[@]}" > "$OUT/table.txt" 2>&1 || RC=1
tee -a "$R" < "$OUT/table.txt"
{ echo "## tfs-bound on $NAME ($SKU)"; echo; cat "$OUT/table.txt"; } >> "${GITHUB_STEP_SUMMARY:-/dev/null}"
exit "$RC"
