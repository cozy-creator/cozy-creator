#!/usr/bin/env bash
# h3-group (cr-326): H3 over a group of GPUs, sequence parallel (Ulysses), one run per degree in `degrees`, on one rental of
# `gpus` GPUs. Paul, 11-17: "minimax h3 inference can use 1 GPU, 2, 4, 7, or 8 GPUs. We should always support all of these
# with ulysses-style parallelism." On rc/0.27.0 a memory change (cl-1119, cozy-runtime#2224) asked torch for the card while
# registering weights inside a construction, and every group follower failed its prepare, `Invalid device id` (Paul's Orbit,
# PRO6-36/38/45/46 on 2x H100 NVL), with nothing between releases running a group. Each degree here forms its group, registers
# H3's weights with native memory on every GPU and renders H3's shortest clip (Turbo, 1 s); it passes when the run completes
# at that degree on that many GPUs. A release-gate cell of the Runtime and varena suites, run by hand: the release lead at a cut
# (4, 2 and 1 on one 4x rtx-3090; 8 and 7 on an 8x by hand), and a change to groups, varena or minimax-h3 at 2 as its PR proof.
# A dev home on this runner's cozy (an RC build: pod-session `cozy`), packages from the packages_refs checkout `tree`: the
# rental moves to the RC's builds, as Paul's laptop moves his. params:
#   tree      the packages_refs checkout the packages run from (required)
#   gpus      the rental's GPUs (2); pod-session's deadline reads it too
#   degrees   the groups, one run each, in order (`gpus`): "2", "4", "8 7"
#   runtime   a Runtime in place of the RC's: a cozy-runtime dev-build or rc-dev run id, or a wheel's https URL
#   seconds   what the cap must buy (2400)
. "$(dirname "$0")/lib.sh"
command -v jq > /dev/null || fail "no jq"
TREE=$(param tree '') G=$(param gpus 2) FROM=$(param runtime '')
DEGREES=$(param degrees "$G")
[[ $G =~ ^[0-9]+$ ]] || fail "params.gpus is the rental's GPU count: $G"
for d in $DEGREES; do [[ $d =~ ^[0-9]+$ && $d -ge 1 && $d -le $G ]] || fail "degree $d is not 1..$G"; done
V=$(cozy --version | awk '{print $2}')
[[ $V == *-dev* ]] || fail "cozy $V is no RC build: dispatch with cozy=<a dev-build or rc-dev run of the build under test>"
[ -n "$TREE" ] && [ -f "$PACKAGES/$TREE/minimax-h3/pyproject.toml" ] ||
  fail "the packages run from a checkout: dispatch with -f packages_refs=<name>=<the packages RC> and params.tree=<name>"
SOURCE=$(cd "$PACKAGES/$TREE" && pwd)
kernels "$SOURCE/minimax-h3"
WHEEL=
if [ -n "$FROM" ]; then
  WHEEL=$(wheel_from "${RUNNER_TEMP:-$OUT}/runtime" "$FROM") && [ -f "$WHEEL" ] || fail "no Runtime wheel from $FROM"
  say "Runtime under test: ${WHEEL##*/}"
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
RENT_ARGS="--gpus $G"
quote "$(param seconds 2400)"
GUARD_ALSO=own guard
order
ready
# The dev home moves it to the RC's builds once it's ready; then this cozy as its TensorD (a newer RC TensorD than the runner's
# Calcifer refused its package upload, invalid_range: 44192752542), and a named Runtime if there is one.
for _ in $(seq 180); do
  cozy machine show "$NAME" --json > "$OUT/show-rc.json" 2> /dev/null
  [ "$(jq -r '.rc.state // empty' "$OUT/show-rc.json")" = current ] && [ "$(jq -r '.state // empty' "$OUT/show-rc.json")" = ready ] && break
  sleep 5
done
[ "$(jq -r '.rc.state // empty' "$OUT/show-rc.json")" = current ] || fail "$NAME isn't on the RC's builds in 15 min: $(jq -c .rc "$OUT/show-rc.json")"
step update machine update "$NAME" --tensord "$(command -v cozy)" ${WHEEL:+--runtime "$WHEEL"} -y || fail "update: $(tail -c 300 "$OUT/update.txt")"
for _ in $(seq 120); do [ "$(state "$NAME")" = ready ] && break; sleep 5; done
cozy machine show "$NAME" --json > "$OUT/show-held.json" 2> /dev/null
say "under test: cozy $(jq -r '.status.version' "$OUT/show-held.json") · Runtime $(jq -r '.status.runtime' "$OUT/show-held.json") · TensorFS $(jq -r '.status.tensorfs' "$OUT/show-held.json") · $G GPUs"

# group <degree>: from a restarted machine (no executor of the last group kept), H3 Turbo 1 s over <degree> GPUs, a new seed
# (the call memo answers a repeated request); its record in g<degree>.json. The DiT call's execution is the widest call's.
group() {
  local d=$1 c=g$1 n got
  cozy machine update "$NAME" --gpu-memory-limit off -y > "$OUT/restart-$c.txt" 2>&1 || fail "restart before $c"
  for _ in $(seq 120); do [ "$(state "$NAME")" = ready ] && break; sleep 5; done
  jq -n --argjson s "$RANDOM$RANDOM" '{summary: {references: [], text: "A lighthouse on a rocky point at dusk, waves breaking below."},
    beats: [{duration: 1, description: {references: [], text: "The beam sweeps once across the water."}}], seed: $s}' > "$OUT/$c.req.json"
  step "$c" run "$SOURCE/minimax-h3/generate" --input "$OUT/$c.req.json" --gpus "$d" --on "$NAME" --output "$OUT/$c"
  n=$(runno "$c"); [ -n "$n" ] || n=$(grep -oE '^Run [A-Z0-9]+-[0-9]+ failed' "$OUT/$c.txt" | awk '{print $2}' | tail -1)
  [ -z "$n" ] || (cd "$RUN_PROJECT" && cozy --json --full run watch "$n" --once) > "$OUT/$c.json" 2> "$OUT/$c.err"
  got=$(jq -r '[.calls[]?.measurements.execution // {} | select(.degree) | [.degree, (.gpus // [] | map(.uuid) | unique | length)]]
    | max_by(.[0]) // [1, 1] | "\(.[0]) \(.[1])"' "$OUT/$c.json" 2> /dev/null)
  if [ "$(jq -r '.status // empty' "$OUT/$c.json" 2> /dev/null)" = completed ] && [ "$got" = "$d $d" ]; then
    say "PASS $c: run $n over $d GPUs completed in $(jq -r '(.execution_ms // 0) / 1000 | floor' "$OUT/$c.json") s"
  else
    say "FAIL $c: run ${n:-none} $(jq -r '.status // "unrecorded"' "$OUT/$c.json" 2> /dev/null), degree and GPUs ${got:-none}, not $d $d: $(jq -r '[.reason.code, .reason.message] | map(select(. != null)) | join(": ")' "$OUT/$c.json" 2> /dev/null | cut -c1-400)"
    RC=1
  fi
}
for d in $DEGREES; do group "$d"; done
cozy machine logs "$NAME" --since 3h -n 0 > "$OUT/machine.log" 2>&1
{ echo "## h3-group on $NAME ($SKU x$G), degrees $DEGREES"; grep -E '^[0-9:]+ (PASS|FAIL)' "$R"; } >> "${GITHUB_STEP_SUMMARY:-/dev/null}"
exit "$RC"
