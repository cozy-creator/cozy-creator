# cold-load (cl-1318; Paul 11-19: "if the disk / PCIe would support us getting it in there in 5 seconds, we should get it in there
# as close to 5 seconds as possible"): on one rental, each model's time until inference can start (its loop's first step: a
# denoise step, a first token) against the machine's bound, cold from disk and from RAM.
# The bound counts the bytes in place by then: all of them from disk (÷ disk GB/s), and the part held on the GPU over PCIe too
# (÷ pinned H2D GB/s), the larger; from RAM, the PCIe part alone. Both speeds are the pod's own, varena's `ceiling` in each run's
# record. A model paged on its card (H3 on a 24 GB card) streams its other blocks during steps: step time, not load.
# Per arm (its `cozy machine update`), per model: a warm-up run (downloads, installs, kernels; never judged), then `repeats`
# rounds of `conditions`, each on a fresh executor (TensorD restarted: `cozy machine update --gpu-memory-limit off`):
#   cold  its derived pages' records (LoRA bakes), stage memos and learned peaks removed and the page cache dropped: a
#         first load
#   ram   an immediate repeat: the page cache and the bakes the run before persisted are kept
#   disk  the page cache dropped, the persisted bakes kept: a later load from disk
# With `hot`, a last run on the last cell's executor: the warm reference for a loop of one step (`warm`: hot).
# Each cell's request differs (seed, `{take}`): no call or stage memo answers it. Kernels, JIT caches and the package
# environments' page cache stay (the warm-up built and read them); the drop is the store's, derived pages' objects included.
# Every number is from the run's own record (stages, legs, varena's fill trace and bakes): fixtures/cold-load/table.py (its
# header defines each) makes table.txt, and makes it again from a downloaded artifact. A run of another client's on the pod
# (cl-1316: TensorD's log names it) voids the cells it overlapped: strays.json, and their rows say so.
# params:
#   tree        the packages_refs checkout the models run from (required)
#   models      names or objects, run in order (default ["h3", "qwen-image-2"]):
#               h3            H3 Shot (minimax-h3/generate): one 15 s beat, Turbo, its defaults
#               qwen-image-2  Qwen Image 2's Image tool (qwen-image-2/image): its defaults (Turbo, 2 MP, the prompt enhancer)
#               {"name", "target": "<package>/<function>", "input": {…}, "loop": "<its steps' stage>", "warm": "step2"|"hot"}:
#               `{take}` in the input is the cell's number, and its seed is the cell's own; no loop: its longest steps that
#               decode nothing; warm: the step a cell's first is measured against (table.py), its own step 2 by default
#   conditions  cold, ram and disk, in the order each round runs them (default ["cold", "ram"])
#   repeats     rounds (default 2)
#   hot         true: each model's `hot` run (default false)
#   update      `cozy machine update` flags every arm starts from, each with its value: `self` (this session's cozy, for
#               --tensord), an https URL (a wheel, or a cozy binary), `artifact:<run id>/<name>` (a cozy-creator run's wheel),
#               a cozy-runtime dev-build run id (--runtime), else as given ([]: the image's)
#   arms        [{"name", "update": [flags over `update`]}], run in order (default [{"name": "rc"}]): A B B A is four
#   need_s      seconds the cap must buy (default 9000)
#   ident       true: after its rounds, each arm runs each model once more, cold, with one fixed request (seed 4242) in its
#               own project (no call memo of another arm's answers it): the arms' outputs compared (default false)
#   trace       true: after each cell, the pod's /tmp/cozy-trace (a proof build's stack samples) as <label>.trace.tgz
. "$(dirname "$0")/lib.sh"
HERE=$(cd "$(dirname "$0")" && pwd)
FIX=$HERE/fixtures/cold-load
TREE=$(param tree '') MODELS=$(param models '["h3", "qwen-image-2"]') CONDITIONS=$(param conditions '["cold", "ram"]')
REPEATS=$(param repeats 2) BASE=$(param update '[]') ARMS=$(param arms '[{"name": "rc"}]') NEED=$(param need_s 9000)
HOT=$(param hot false) IDENT=$(param ident false) TRACE=$(param trace false)
[[ $IDENT =~ ^(true|false)$ && $TRACE =~ ^(true|false)$ ]] || fail "ident $IDENT, trace $TRACE: true or false"
[[ $HOT =~ ^(true|false)$ ]] || fail "hot $HOT: true or false"
for tool in jq python3 curl; do command -v "$tool" > /dev/null || fail "no $tool"; done
[ -n "$TREE" ] && [ -d "$PACKAGES/$TREE" ] || fail "dispatch with -f packages_refs=<name>=<ref> and params.tree=<name>"
SOURCE=$(cd "$PACKAGES/$TREE" && pwd)
[[ $REPEATS =~ ^[1-9]$ && $NEED =~ ^[0-9]+$ ]] || fail "repeats $REPEATS, need_s $NEED"
jq -e 'type == "array" and length > 0 and all(.[]; . == "cold" or . == "ram" or . == "disk")' <<<"$CONDITIONS" > /dev/null ||
  fail "conditions $CONDITIONS: of cold, ram, disk"
PRESETS='{
  "h3": {"target": "minimax-h3/generate", "loop": "denoise", "input": {
    "summary": {"text": "A lighthouse on a rocky point at dusk, take {take}.", "references": []},
    "beats": [{"duration": 15, "description": {"text": "Waves break on the rocks below as the lamp turns and a gull crosses its beam.", "references": []}}]}},
  "qwen-image-2": {"target": "qwen-image-2/image", "loop": "generating image", "input": {
    "instructions": {"text": "A red lighthouse on a cliff at dusk, waves breaking below, photo, take {take}", "references": []}}}
}'
MODELS=$(jq -c --argjson p "$PRESETS" 'map(if type == "string" then ($p[.] // error("no preset \(.)")) + {name: .} else . end)' <<<"$MODELS") ||
  fail "models: names of $(jq -r 'keys | join(", ")' <<<"$PRESETS"), or {name, target, input, loop, warm}"
jq -e 'all(.[]; (.name | test("^[a-z0-9-]+$")) and (.target | test("^[a-z0-9.-]+/[a-z0-9_]+$")) and (.input | type == "object")
  and ((.warm // "step2") | . == "step2" or . == "hot"))' \
  <<<"$MODELS" > /dev/null && [ "$(jq '[.[].name] | unique | length' <<<"$MODELS")" = "$(jq length <<<"$MODELS")" ] ||
  fail "models $MODELS: each a unique name, a <package>/<function> target, an input object and warm step2 or hot"
mapfile -t names < <(jq -r '.[].name' <<<"$MODELS")
for m in "${names[@]}"; do
  p=$(jq -r --arg m "$m" '.[] | select(.name == $m) | .target | split("/")[0]' <<<"$MODELS")
  [ -f "$SOURCE/$p/pyproject.toml" ] || fail "no $p in the packages checkout $TREE"
  kernels "$SOURCE/$p"
  say "model $m: $(jq -r --arg m "$m" '.[] | select(.name == $m) | "\(.target), loop \(.loop // "the longest")"' <<<"$MODELS"), from $TREE $(git -C "$SOURCE" rev-parse --short HEAD 2> /dev/null)"
done

# Each arm's `machine update` flags (the base's, then its own over them), their sources fetched before renting.
resolve() {
  local d=$1 flag=$2 v=$3 f
  case $v in
    self) command -v cozy ;;
    https://*.whl | artifact:*) wheel_from "$d" "$v" ;;
    https://*) mkdir -p "$d" && f=$d/${v##*/} && curl -fsSL --retry 5 -o "$f" "$v" && chmod +x "$f" && echo "$f" ;;
    *) if [ "$flag" = --runtime ] && [[ $v =~ ^[0-9]+$ ]]; then wheel_from "$d" "$v"; else echo "$v"; fi ;;
  esac
}
mapfile -t arms < <(jq -r '.[].name' <<<"$ARMS")
[ "${#arms[@]}" -gt 0 ] && [ "$(printf '%s\n' "${arms[@]}" | sort -u | wc -l)" = "${#arms[@]}" ] || fail "arms $ARMS: unique names"
declare -A FLAGS=()
for a in "${arms[@]}"; do
  [[ $a =~ ^[a-z0-9-]+$ ]] || fail "arm name $a"
  pairs=$(jq -c --arg a "$a" --argjson base "$BASE" '(.[] | select(.name == $a) | .update // []) as $own
    | [$base, $own][] | if (length % 2 == 0) and all(to_entries[]; (.key % 2 == 1) or (.value | test("^--[a-z-]+$"))) then . else error("flags") end
    | [range(0; length; 2) as $i | {(.[$i]): .[$i + 1]}] | add // {}' <<<"$ARMS" | jq -sc 'add // {}') ||
    fail "arm $a: update flags are [\"--flag\", \"value\", …]"
  f=()
  while IFS=$'\t' read -r flag v; do
    [ -n "$flag" ] || continue
    r=$(resolve "$OUT/arm-$a/${flag#--}" "$flag" "$v") && [ -n "$r" ] || fail "arm $a: $flag $v not fetched"
    f+=("$flag" "$r")
  done < <(jq -r 'to_entries[] | [.key, .value] | @tsv' <<<"$pairs")
  FLAGS[$a]="${f[*]}" none="nothing: the image's software"
  say "arm $a: machine update ${f[*]:-$none}"
done

[ -f ~/.ssh/id_ed25519.pub ] || ssh-keygen -q -t ed25519 -N '' -f ~/.ssh/id_ed25519 || fail "ssh-keygen"
RENT_ARGS=--ssh
quote "$NEED"
rent
pod() { timeout "${T:-120}" cozy machine ssh "$NAME" "$@"; }
for i in $(seq 1 20); do pod true < /dev/null > /dev/null 2>&1 && break; [ "$i" = 20 ] && fail "no SSH into $NAME"; sleep 15; done
T=60 pod "cat > /tmp/cold-load.py" < "$FIX/pod.py" || fail "pod.py"
T=600 pod "python3 /tmp/cold-load.py census" < /dev/null > "$OUT/census-start.json" 2> "$OUT/census-start.err"
say "pod: $(cat "$OUT/census-start.json")"

# The pod's live runs and line, every 15 s: what ran beside each cell.
( while :; do
    printf '{"at_ms":%s,"show":%s}\n' "$(date +%s%3N)" \
      "$(timeout 60 cozy machine show "$NAME" --json 2> /dev/null | jq -c '{running, queued, runs: [.status.runs[]? | {id, number, state, target}]}' 2> /dev/null || echo null)" >> "$OUT/monitor.ndjson"
    sleep 15
  done ) & BG=$!

WANT=''
# software: the pod's cozy, Runtime and TensorFS.
software() { cozy machine show "$NAME" --json 2> /dev/null | jq -r '"\(.status.version // "") \(.status.runtime // "") \(.status.tensorfs // "")"'; }
# arm <name>: its update (a restart), and what it put on the pod.
arm() {
  if [ -n "${FLAGS[$1]}" ]; then
    # shellcheck disable=SC2086 # flags and paths without spaces
    step "update-$1" machine update "$NAME" ${FLAGS[$1]} -y || fail "arm $1: the update"
  fi
  WANT=$(software)
  say "arm $1: $(cozy machine show "$NAME" --json | jq -r '"cozy \(.status.version) · Runtime \(.status.runtime) · TensorFS \(.status.tensorfs) · \(.status.gpus[0].name)"')"
}
# fresh <label>: TensorD restarted, no executor kept; a pod moved off the arm's software is updated again, twice at most.
fresh() {
  local t v
  step "restart-$1" machine update "$NAME" --gpu-memory-limit off -y || fail "$1: the restart"
  for t in $(seq 120); do [ "$(state "$NAME")" = ready ] && break; sleep 5; done
  for t in 1 2; do
    v=$(software)
    [ "$v" != "$WANT" ] || return 0
    say "$1: the pod runs $v, not the arm's $WANT: updated again"
    # shellcheck disable=SC2086
    step "update-$1-$t" machine update "$NAME" ${FLAGS[$A]} -y
  done
  [ "$(software)" = "$WANT" ] || fail "$1: the pod kept moving off the arm's software"
}
# podpy <label> <mode>: the pod-side fixture's line in <label>.census.json.
podpy() {
  local try
  for try in 1 2 3; do
    T=900 pod "python3 /tmp/cold-load.py $2" < /dev/null > "$OUT/$1.census.json" 2> "$OUT/$1.census.err" &&
      jq -e .mode "$OUT/$1.census.json" > /dev/null 2>&1 && return 0
    say "$1: pod $2, try $try: $(tail -c 200 "$OUT/$1.census.err")"; sleep 10
  done
  fail "$1: the pod's $2 gave nothing"
}
# prepare <label> <condition>: the cell's executor and caches.
prepare() {
  [ "$2" = hot ] || fresh "$1"
  case $2 in
    cold) podpy "$1" forget; mv "$OUT/$1.census.json" "$OUT/$1.forget.json"; podpy "$1" drop ;;
    disk) podpy "$1" drop ;;
    *) podpy "$1" census ;;
  esac
  say "$1 ($2): $(jq -c '{drop_caches, store_gb, store_resident_gb, derived_records, cached_gb}' "$OUT/$1.census.json")"
}
printf 'cell\tarm\ttemperature\tbake_blocks\tderived_bytes\tpersisted\tmemoized\texec_s\tdownloaded\n' > "$OUT/states.tsv"
: > "$OUT/cold-load.tsv"
K=0
# cell <arm> <model> <condition> [label] [ident]: one run of the model, its record in <label>.json. ident: the fixed request.
cell() {
  local a=$1 m=$2 c=$3 label=${4:-$1-$2-$3} n target loop take seed
  K=$((K + 1)) take=$K seed=$((7000 + K))
  [ -z "${5:-}" ] || take=ident seed=4242
  target=$(jq -r --arg m "$m" '.[] | select(.name == $m) | .target' <<<"$MODELS")
  loop=$(jq -r --arg m "$m" '.[] | select(.name == $m) | .loop // ""' <<<"$MODELS")
  jq --arg m "$m" --arg take "$take" --argjson seed "$seed" '.[] | select(.name == $m) | .input
    | walk(if type == "string" then gsub("\\{take\\}"; $take) else . end) | .seed = $seed' <<<"$MODELS" > "$OUT/$label.req.json"
  prepare "$label" "$c"
  step "$label" run "$SOURCE/$target" --input "$OUT/$label.req.json" --on "$NAME"
  n=$(runno "$label")
  [ -n "$n" ] && (cd "$RUN_PROJECT" && cozy --json --full run watch "$n" --once) > "$OUT/$label.json" 2> "$OUT/$label.err"
  say "$label: run ${n:-none} $(jq -r '.status // "unrecorded"' "$OUT/$label.json" 2> /dev/null), executing $(jq -r '(.execution_ms // 0) / 1000 | floor' "$OUT/$label.json" 2> /dev/null) s"
  printf '%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\n' "$label" "$a" "$m" "$c" "$loop" "$label.json" "$label.census.json" \
    "$(jq -r --arg m "$m" '.[] | select(.name == $m) | .warm // "step2"' <<<"$MODELS")" >> "$OUT/cold-load.tsv"
  # The ledger's temperature: cold only from a cleared machine.
  local temperature=warm downloaded=false
  [ "$c" != cold ] || temperature=cold
  [ "$c" != warmup ] || temperature=warmup downloaded=true
  jq -r --arg l "$label" --arg a "$a" --arg t "$temperature" --arg dl "$downloaded" '. as $r | [.measurements, (.calls[]?.measurements)]
    | map(select(. != null) | .memory.stats.varena[]?.stats // empty) as $v
    | [$l, $a, $t, ($v | map(.moved.bake.blocks // 0) | add // 0), ($v | map(.moved.derived.bytes // 0) | add // 0),
       ($v | map(.baked_pages.persisted // 0) | max // 0), (($r.memo.memoized // false) or ([$r.calls[]?.memoized] | any) | tostring),
       (($r.execution_ms // 0) / 1000), $dl] | @tsv' "$OUT/$label.json" >> "$OUT/states.tsv" 2> /dev/null
  # Its bakes persist after it: settled before anything ends its executor.
  podpy "$label-after" settle
  if [ "$TRACE" = true ]; then
    T=300 pod "cd /tmp && tar -czf - cozy-trace 2> /dev/null; rm -rf /tmp/cozy-trace" < /dev/null > "$OUT/$label.trace.tgz" 2> "$OUT/$label.trace.err"
    say "$label: trace $(stat -c %s "$OUT/$label.trace.tgz") bytes"
  fi
}

for A in "${arms[@]}"; do
  arm "$A"
  for m in "${names[@]}"; do
    cell "$A" "$m" warmup "$A-$m-warmup"
    [ "$(jq -r .status "$OUT/$A-$m-warmup.json" 2> /dev/null)" = completed ] || fail "$A-$m-warmup didn't complete: no cell of $m would"
    for r in $(seq 1 "$REPEATS"); do
      for c in $(jq -r '.[]' <<<"$CONDITIONS"); do cell "$A" "$m" "$c" "$A-$m-$c$r"; done
    done
    if [ "$HOT" = true ] || [ "$(jq -r --arg m "$m" '.[] | select(.name == $m) | .warm // "step2"' <<<"$MODELS")" = hot ]; then
      cell "$A" "$m" hot
    fi
  done
  if [ "$IDENT" = true ]; then
    shared=$RUN_PROJECT RUN_PROJECT=$OUT/ident-$A
    run_project "$RUN_PROJECT" > "$OUT/ident-$A.project.txt" 2>&1 || fail "arm $A: its ident project"
    for m in "${names[@]}"; do cell "$A" "$m" cold "$A-$m-ident" ident; done
    RUN_PROJECT=$shared
  fi
done
kill $BG 2> /dev/null; BG=''

LOGGED=true
cozy machine logs "$NAME" --lines 0 --since "$(( ($(date +%s) - $(jq -r .at_ms "$OUT/census-start.json") / 1000) / 60 + 10 ))m" > "$OUT/tensord.log" 2>&1 ||
  { LOGGED=false; say "the pod's TensorD log wasn't read"; }
(cd "$RUN_PROJECT" && cozy --json run ls --all --on "$NAME" --limit 0) > "$OUT/runs.json" 2>&1
cozy model ls --on "$NAME" --json --full > "$OUT/models.json" 2>&1
# Strays (cl-1316): every run TensorD's log names in a cell's span that isn't this session's.
python3 -I - "$OUT" "$LOGGED" <<'PY' | tee -a "$R"
import json, os, re, sys
from datetime import datetime
d = sys.argv[1]
if sys.argv[2] != "true":
    json.dump({}, open(os.path.join(d, "strays.json"), "w"))
    print("strays: unknown, the pod's TensorD log wasn't read")
    sys.exit(0)
mine = set()
for line in open(os.path.join(d, "runs.json"), errors="replace"):
    for m in re.findall(r'"id"\s*:\s*"(run-[0-9a-f]+)', line):
        mine.add(m)
cells = [l.rstrip("\n").split("\t") for l in open(os.path.join(d, "cold-load.tsv")) if l.strip()]
spans = {}
for c in cells:
    try:
        r = json.load(open(os.path.join(d, c[5])))
    except (OSError, ValueError):
        continue
    mine.add(str(r.get("id")))
    spans[c[0]] = (r.get("created_at_ms") or 0, r.get("settled_at_ms") or 0)
seen = []
for line in open(os.path.join(d, "tensord.log"), errors="replace"):
    if "update run-" in line:
        continue
    ids = set(re.findall(r"run-[0-9a-f]{24}", line)) - mine
    if not ids:
        continue
    try:
        at = datetime.fromisoformat(line.split()[0].rstrip("Z")).timestamp() * 1000
    except ValueError:
        continue
    seen += [(at, i) for i in ids]
strays = {label: sorted({i for at, i in seen if a <= at <= b}) for label, (a, b) in spans.items()}
strays = {k: v for k, v in strays.items() if v}
json.dump(strays, open(os.path.join(d, "strays.json"), "w"))
print(f"strays: {json.dumps(strays) if strays else 'none: the pod ran only this session’s runs'}; other runs in the log: {sorted({i for _, i in seen})}")
PY
python3 -I "$FIX/table.py" "$OUT" > "$OUT/table.txt" 2> "$OUT/table.err" || { say "table: $(tail -c 300 "$OUT/table.err")"; RC=1; }
grep -v '^{' "$OUT/table.txt" | tee -a "$R"
{ echo "## cold-load on $NAME ($SKU)"; echo; grep -v '^{' "$OUT/table.txt"; } >> "${GITHUB_STEP_SUMMARY:-/dev/null}"
bad=$(jq -r 'select(.status != "completed" or (.flaws | length) > 0) | "\(.label): \(.status) \(.flaws | join("; "))"' \
  < <(grep '^{' "$OUT/table.txt") 2> /dev/null)
[ -z "$bad" ] || { say "cells with flaws: $bad"; RC=1; }
exit "$RC"
