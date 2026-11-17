#!/usr/bin/env bash
# cr326-group (proof, never merged): a 2-GPU group renders H3 Turbo 15 s with the Runtime under test, then the same request
# on 1 GPU. cr-326: on rc/0.27.0 every group follower failed its prepare (`Invalid device id`, Paul's PRO6-36/38/45/46).
# Setup as llm-two-gpus: a dev home on this runner's RC cozy, packages from the packages_refs checkout `tree`, the rental on
# the RC's builds, then this cozy as its TensorD and `runtime`'s wheel. params: tree, runtime (required), seconds (3600).
. "$(dirname "$0")/lib.sh"
for tool in jq python3; do command -v "$tool" > /dev/null || fail "no $tool"; done
TREE=$(param tree '') FROM=$(param runtime '')
V=$(cozy --version | awk '{print $2}')
[[ $V == *-dev* ]] || fail "cozy $V is no RC build: dispatch with cozy=<a dev-build run of the build under test>"
[ -n "$TREE" ] && [ -f "$PACKAGES/$TREE/minimax-h3/pyproject.toml" ] ||
  fail "the packages run from a checkout: dispatch with -f packages_refs=<name>=<the packages RC> and params.tree=<name>"
[ -n "$FROM" ] || fail "params.runtime names the Runtime under test: a cozy-runtime dev-build or rc-dev run id"
SOURCE=$(cd "$PACKAGES/$TREE" && pwd)
kernels "$SOURCE/minimax-h3"
WHEEL=$(wheel_from "${RUNNER_TEMP:-$OUT}/runtime" "$FROM") && [ -f "$WHEEL" ] || fail "no Runtime wheel from $FROM"
say "Runtime under test: ${WHEEL##*/}"
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
# h3 <cell> <gpus>: H3 Turbo 15 s from the checkout, a new seed; its whole record in <cell>.json.
h3() {
  local cell=$1 n
  jq -n --argjson s "$RANDOM$RANDOM" '{summary: {references: [], text: "A steady tracking shot of a courier cycling a cargo bike through a narrow cobbled street after rain, past cafe tables and a flower stall, into a sunlit square."},
    beats: [{duration: 15, description: {references: [], text: "The courier rings the bell at the flower stall and rolls down the slope into the square."}}],
    soundscape: {references: [], text: "Tires hissing on wet cobbles, a bicycle bell, cafe chatter."}, seed: $s}' > "$OUT/$cell.req.json"
  step "$cell" run "$SOURCE/minimax-h3/generate" --input "$OUT/$cell.req.json" --gpus "$2" --on "$NAME" --output "$OUT/$cell"
  n=$(runno "$cell"); [ -n "$n" ] || n=$(grep -oE '^Run [A-Z0-9]+-[0-9]+ failed' "$OUT/$cell.txt" | awk '{print $2}' | tail -1)
  [ -z "$n" ] || (cd "$RUN_PROJECT" && cozy --json --full run watch "$n" --once) > "$OUT/$cell.json" 2> "$OUT/$cell.err"
  say "$cell: run ${n:-none} on $2 GPU(s): $(jq -r '.status // "unrecorded"' "$OUT/$cell.json" 2> /dev/null) in $(jq -r '(.execution_ms // 0) / 1000 | floor' "$OUT/$cell.json" 2> /dev/null) s; GPUs $(jq -c '[.measurements.execution.gpus[]?.uuid]' "$OUT/$cell.json" 2> /dev/null); $(jq -r '[.reason.code, .reason.message] | map(select(. != null)) | join(": ")' "$OUT/$cell.json" 2> /dev/null | cut -c1-300)"
  [ "$(jq -r '.status // empty' "$OUT/$cell.json" 2> /dev/null)" = completed ] || RC=1
}
h3 two 2
h3 one 1
cozy machine logs "$NAME" --since 3h -n 0 > "$OUT/machine.log" 2>&1
grep -c 'Invalid device id' "$OUT/machine.log" | xargs -I{} say "machine log: {} 'Invalid device id' lines"
{ echo "## cr326-group on $NAME ($SKU x2): cozy $on, Runtime $runtime"; grep -E '(two|one): run|FAIL' "$R"; } >> "${GITHUB_STEP_SUMMARY:-/dev/null}"
exit "$RC"
