# llm-two-gpus (cl-1119): two LLMs on one 2-GPU rental each get a GPU of their own, and write long answers at once to the end.
# Paul 11-17, on pod-session 43985932697 (2× RTX A6000), where Qwen3.6-35B-A3B went beside Qwen3.5-9B on GPU 0, to 48.2 of 48 GB,
# while GPU 1 held 1 MiB, and then built a second executor on GPU 1 that ran out of memory alone: "this sounds like a scheduling
# problem in TensorD, please investigate".
# A dev home on this runner's cozy, its packages from the packages_refs checkout `tree`: the rental moves to the RC's builds, then
# to this session's cozy as its TensorD and the Runtime `runtime` names, held there for the session.
#   placed  Qwen3.5's and Qwen3.6's `write`, a short question each (cold: install, download): each on a GPU of its own (its run's
#           GPU uuid against the pod's nvidia-smi).
#   writes  both write a long story at once (max_tokens 6000): both complete, each on its executor from `placed` (no second
#           executor), with each GPU's peak memory from nvidia-smi once a second.
#   paged   with `cap`: every GPU limited to it, so Qwen3.6 pages, its write (300 tokens) cold then three times warm, on the
#           Runtime under test and on `baseline`'s, each arm a restart: its regions, and its times against the baseline's.
# params:
#   tree      the packages_refs checkout the packages run from (required)
#   runtime   the rental's Runtime: a cozy-runtime dev-build or rc-dev run id, or a wheel's https URL (required)
#   cap       a per-GPU limit for `paged`, e.g. 24GiB (default: none, no `paged`)
#   baseline  the Runtime `paged` compares against, as `runtime` names one (required with `cap`)
#   seconds   what the cap must buy (default 3600)
. "$(dirname "$0")/lib.sh"
for tool in jq python3; do command -v "$tool" > /dev/null || fail "no $tool"; done
TREE=$(param tree '') FROM=$(param runtime '') LIMIT=$(param cap '') BASE_FROM=$(param baseline '')
[ -z "$LIMIT" ] || [ -n "$BASE_FROM" ] || fail "params.cap pages Qwen3.6 against params.baseline's Runtime: name it"
V=$(cozy --version | awk '{print $2}')
[[ $V == *-dev* ]] || fail "cozy $V is no RC build: dispatch with cozy=<a dev-build run of the build under test>"
[ -n "$TREE" ] && [ -f "$PACKAGES/$TREE/qwen3.5/pyproject.toml" ] && [ -f "$PACKAGES/$TREE/qwen3.6/pyproject.toml" ] ||
  fail "the packages run from a checkout: dispatch with -f packages_refs=<name>=<the packages RC> and params.tree=<name>"
[ -n "$FROM" ] || fail "params.runtime names the Runtime under test: a cozy-runtime dev-build or rc-dev run id"
SOURCE=$(cd "$PACKAGES/$TREE" && pwd)
kernels "$SOURCE/qwen3.5"
kernels "$SOURCE/qwen3.6"
WHEEL=$(wheel_from "${RUNNER_TEMP:-$OUT}/runtime" "$FROM") && [ -f "$WHEEL" ] || fail "no Runtime wheel from $FROM"
say "Runtime under test: ${WHEEL##*/}"
if [ -n "$LIMIT" ]; then
  BASE=$(wheel_from "${RUNNER_TEMP:-$OUT}/baseline" "$BASE_FROM") && [ -f "$BASE" ] || fail "no baseline Runtime wheel from $BASE_FROM"
  say "baseline Runtime: ${BASE##*/}"
fi
cfg=$HOME/.cozy/config.toml
mkdir -p "${cfg%/*}" && touch "$cfg" || fail "no $cfg"
{ echo 'dev = true'; grep -v '^dev *=' "$cfg" || true; } > "$cfg.new" && mv "$cfg.new" "$cfg" || fail "dev = true not written"
awk -v source="$SOURCE" '
  function ours() { print "packages = \"" source "\""; put = 1 }
  /^packages *=/ { next } { print } /^\[calcifer\]/ { ours() } END { if (!put) { printf "\n[calcifer]\n"; ours() } }' \
  "$cfg" > "$cfg.next" && mv "$cfg.next" "$cfg" || fail "calcifer.packages not written"
cozy restart > "$OUT/restart.txt" 2>&1 || fail "cozy restart: $(tail -c 300 "$OUT/restart.txt")"
[ -z "$(own)" ] || fail "this runner's Calcifer already holds a rental: $(own | paste -sd' ')"
trap 'code=$?; claim; (exit "$code"); finish' EXIT
[ -f ~/.ssh/id_ed25519.pub ] || ssh-keygen -q -t ed25519 -N '' -f ~/.ssh/id_ed25519 || fail "ssh-keygen"
RENT_ARGS="--ssh --gpus 2"
quote "$(param seconds 3600)"
GUARD_ALSO=own guard
order
ready
# The dev home moves it to the RC's builds (its TensorFS) once it's ready; then the builds under test, named, which holds it there.
for _ in $(seq 180); do
  cozy machine show "$NAME" --json > "$OUT/show-rc.json" 2> /dev/null
  [ "$(jq -r '.rc.state // empty' "$OUT/show-rc.json")" = current ] && [ "$(jq -r '.state // empty' "$OUT/show-rc.json")" = ready ] && break
  sleep 5
done
[ "$(jq -r '.rc.state // empty' "$OUT/show-rc.json")" = current ] || fail "$NAME isn't on the RC's builds in 15 min: $(jq -c .rc "$OUT/show-rc.json")"
step update machine update "$NAME" --tensord "$(command -v cozy)" --runtime "$WHEEL" -y || fail "update: $(tail -c 300 "$OUT/update.txt")"
for _ in $(seq 120); do
  cozy machine show "$NAME" --json > "$OUT/show-held.json" 2> /dev/null
  [ "$(jq -r '.state // empty' "$OUT/show-held.json")" = ready ] && break
  sleep 5
done
on=$(jq -r '.status.version // empty' "$OUT/show-held.json") runtime=$(jq -r '.status.runtime // empty' "$OUT/show-held.json")
[ "$on" = "$V" ] || fail "$NAME runs TensorD $on, not this session's $V"
[[ ${WHEEL##*/} == *"-${runtime}-"* ]] || fail "$NAME runs Runtime $runtime, not ${WHEEL##*/}"
say "under test: cozy $on · Runtime $runtime · TensorFS $(jq -r '.status.tensorfs' "$OUT/show-held.json") · held $(jq -r '.held // "no"' "$OUT/show-held.json")"

# nvidia-smi once a second for the whole session: each GPU's memory and busy share.
SMI=$OUT/smi.csv
cozy machine ssh "$NAME" "nvidia-smi --query-gpu=index,uuid --format=csv,noheader" < /dev/null > "$OUT/uuids.csv" 2> "$OUT/uuids.err"
grep -q '^1, GPU-' "$OUT/uuids.csv" || fail "no GPU uuids from $NAME: $(head -c 300 "$OUT/uuids.csv" "$OUT/uuids.err")"
cozy machine ssh "$NAME" "timeout 3000 nvidia-smi --query-gpu=index,utilization.gpu,memory.used --format=csv,noheader,nounits -l 1 | while IFS= read -r l; do echo \"\$(date +%s%3N),\$l\"; done" \
  < /dev/null > "$SMI" 2> "$OUT/smi.err" & smi=$!
BG="$BG $smi"
# write <cell> <package> <inputs…>: its `write` from the checkout on the rental, then its whole record (<cell>.json).
write() {
  local cell=$1 package=$2 n code; shift 2
  step "$cell" run "$SOURCE/$package/write" "$@" --on "$NAME"; code=$?
  n=$(runno "$cell")
  [ -z "$n" ] || (cd "$RUN_PROJECT" && cozy --json --full run watch "$n" --once) > "$OUT/$cell.json" 2> "$OUT/$cell.err"
  say "$cell: run ${n:-none} $(jq -r '.status // "unrecorded"' "$OUT/$cell.json" 2> /dev/null), GPU $(gpu "$cell") pid $(pid "$cell"): $(jq -r '.result.text // .error.message // empty' "$OUT/$cell.json" 2> /dev/null | tr '\n' ' ' | cut -c1-300)"
  return "$code"
}
# gpu <cell>: the pod's index of the GPU its run ran on; pid <cell>: its executor.
gpu() { local u; u=$(jq -r 'first(.measurements.execution.gpus[]?.uuid) // empty' "$OUT/$1.json" 2> /dev/null); [ -n "$u" ] && awk -F', ' -v u="$u" '$2 == u { print $1 }' "$OUT/uuids.csv"; }
pid() { jq -r 'first(.measurements.execution.gpus[]?.pid) // empty' "$OUT/$1.json" 2> /dev/null; }
status() { jq -r '.status // empty' "$OUT/$1.json" 2> /dev/null; }

write q35-short qwen3.5 instructions='Name three things a lighthouse keeper does each night.' max_tokens:=200
write q36-short qwen3.6 instructions='Name three things a lighthouse keeper does each night.' max_tokens:=200
STORY='Write a very long story, at least 3000 words in twenty numbered chapters, about a lighthouse keeper through one winter of storms.'
write q35-story qwen3.5 "instructions=$STORY" max_tokens:=6000 & s35=$!
write q36-story qwen3.6 "instructions=$STORY" max_tokens:=6000 & s36=$!
BG="$BG $s35 $s36"
wait "$s35"; wait "$s36"
sleep 3; kill "$smi" 2> /dev/null
cozy machine logs "$NAME" --since 3h -n 0 > "$OUT/machine.log" 2>&1
cozy machine logs "$NAME" memory -n 100000 > "$OUT/memory.log" 2>&1
peaks=$(awk -F', *' 'NF >= 4 { if ($4 + 0 > p[$2]) p[$2] = $4 + 0 } END { for (g in p) printf "GPU %s %d MiB; ", g, p[g] }' "$SMI")
say "peak memory: ${peaks:-no nvidia-smi samples}"

for cell in q35-short q36-short q35-story q36-story; do
  [ "$(status "$cell")" = completed ] || { say "FAIL $cell: $(status "$cell") $(jq -r '.error.message // empty' "$OUT/$cell.json" 2> /dev/null | cut -c1-300)"; RC=1; }
done
g35=$(gpu q35-short) g36=$(gpu q36-short)
if [ -n "$g35" ] && [ -n "$g36" ] && [ "$g35" != "$g36" ]; then say "PASS placed: Qwen3.5 on GPU $g35, Qwen3.6 on GPU $g36"
else say "FAIL placed: Qwen3.5 on GPU ${g35:-?}, Qwen3.6 on GPU ${g36:-?}: one GPU for both while the other was free"; RC=1; fi
for m in q35 q36; do
  [ "$(pid "$m-story")" = "$(pid "$m-short")" ] && [ "$(gpu "$m-story")" = "$(gpu "$m-short")" ] ||
    { say "FAIL $m-story ran on GPU $(gpu "$m-story") pid $(pid "$m-story"), not its executor from $m-short (GPU $(gpu "$m-short") pid $(pid "$m-short"))"; RC=1; }
done
[ "$RC" != 0 ] || say "PASS writes: both stories completed at once, each on its own GPU's executor"

# paged: Qwen3.6 under the cap on each Runtime. Distinct seeds: the call memo answers a repeated request.
regions() { jq -r '[.measurements.memory.stats.varena[]?.stats.weight_residency.regions // empty] | max // empty' "$OUT/$1.json" 2> /dev/null; }
ms() { jq -r '.execution_ms // empty' "$OUT/$1.json" 2> /dev/null; }
arm() {
  local a=$1 wheel=$2 k
  step "limit-$a" machine update "$NAME" --runtime "$wheel" --gpu-memory-limit "$LIMIT" -y || fail "limit-$a: $(tail -c 300 "$OUT/limit-$a.txt")"
  for _ in $(seq 120); do [ "$(state "$NAME")" = ready ] && break; sleep 5; done
  for k in cold warm1 warm2 warm3; do
    write "$a-$k" qwen3.6 "instructions=List the twelve months, each with a paragraph on a lighthouse keeper's work then." max_tokens:=300 seed:=$RANDOM
    [ "$(status "$a-$k")" = completed ] || { say "FAIL paged $a-$k: $(status "$a-$k")"; RC=1; }
  done
  say "paged $a ($(cozy machine show "$NAME" --json | jq -r '.status.runtime') under $LIMIT): $(regions "$a-cold") regions; cold $(ms "$a-cold") ms, warm $(ms "$a-warm1") $(ms "$a-warm2") $(ms "$a-warm3") ms"
}
if [ -n "$LIMIT" ]; then
  arm test "$WHEEL"
  arm base "$BASE"
  python3 -I - "$OUT" <<'PY' | tee -a "$R"
import json, statistics, sys
def ms(cell):
    return json.load(open(f"{sys.argv[1]}/{cell}.json")).get("execution_ms")
test = statistics.median(ms(f"test-warm{i}") for i in (1, 2, 3))
base = statistics.median(ms(f"base-warm{i}") for i in (1, 2, 3))
cold = ms("test-cold") / ms("base-cold")
print(f"paged: warm median {test:.0f} ms against the baseline's {base:.0f} ms ({test / base - 1:+.1%}); cold {cold - 1:+.1%}")
PY
fi
{ echo "## llm-two-gpus on $NAME ($SKU x2): cozy $on, Runtime $runtime"; grep -E '(PASS|FAIL|peak memory|story: run|short: run)' "$R"; } >> "${GITHUB_STEP_SUMMARY:-/dev/null}"
exit "$RC"
