# run-overhead (cl-1172): a warm run's time outside its compute on a rental of $SKU, this runner's Calcifer driving it, as
# e2e/cli/overhead.sh measures it on this computer: N warm repeats each of minimax-h3-tools' echo (no model) and Qwen3.5's write on
# the real 0.8B (a model held on the GPU), each `cozy run` timed from its start to its exit, less its record's execution time.
# The rental runs this session's cozy as its TensorD (pod-session `cozy`: a dev-build run), and `runtime` (a cozy-runtime dev-build
# run id) when given, else the RC's.
# Without `rent`, $0: the quote, and the checks before it.
# params:
#   runtime  a cozy-runtime dev-build run id the rental runs (default: none, the RC's)
#   n        warm repeats of each (default 10)
#   seconds  what the cap must buy (default 1500)
. "$(dirname "$0")/lib.sh"
HERE=$(cd "$(dirname "$0")" && pwd)
. "$HERE/../../e2e/cli/overhead.sh"
check() { say "FAIL $*"; RC=1; }
for tool in jq python3; do command -v "$tool" > /dev/null || fail "no $tool"; done
V=$(cozy --version | awk '{print $2}')
[[ $V == *-dev* ]] || fail "cozy $V is no dev build: dispatch with cozy=<a dev-build run of the change>"
SRC=$(param runtime '') OVERHEAD_N=$(param n 10)
W=''
if [ -n "$SRC" ]; then
  [[ $SRC =~ ^[0-9]+$ ]] || fail "runtime: a cozy-runtime dev-build run id"
  W=$(wheel_from "$OUT/wheel" "$SRC") && [ -n "$W" ] || fail "no manylinux wheel in cozy-runtime run $SRC"
  say "runtime: ${W##*/}"
fi
cfg=$HOME/.cozy/config.toml
mkdir -p "${cfg%/*}" && touch "$cfg" || fail "no $cfg"
{ echo 'dev = true'; grep -v '^dev *=' "$cfg" || true; } > "$cfg.new" && mv "$cfg.new" "$cfg" || fail "dev = true not written"
cozy restart > "$OUT/restart.txt" 2>&1 || fail "cozy restart: $(tail -c 300 "$OUT/restart.txt")"
[ -z "$(own)" ] || fail "this runner's Calcifer already holds a rental: $(own | paste -sd' ')"
trap 'code=$?; claim; (exit "$code"); finish' EXIT
quote "$(param seconds 1500)"
GUARD_ALSO=own guard
rent
for _ in $(seq 180); do
  cozy machine show "$NAME" --json > "$OUT/show-rc.json" 2> /dev/null
  [ "$(jq -r '.rc.state // empty' "$OUT/show-rc.json")" = current ] && [ "$(jq -r '.state // empty' "$OUT/show-rc.json")" = ready ] && break
  sleep 5
done
update=(--tensord "$(command -v cozy)")
[ -z "$W" ] || update+=(--runtime "$W")
step update machine update "$NAME" "${update[@]}" -y || fail "the pod's TensorD and Runtime"
for _ in $(seq 180); do
  cozy machine show "$NAME" --json > "$OUT/show-held.json" 2> /dev/null
  [ "$(jq -r '.state // empty' "$OUT/show-held.json")" = ready ] && break
  sleep 5
done
say "on: $(jq -r '"cozy \(.status.version) · Runtime \(.status.runtime) · " + ([.status.gpus[] | "GPU \(.index) \(.name)"] | join(", "))' "$OUT/show-held.json")"
say "ping: $(ping -c 5 -q "$(jq -r '.address // .endpoint // empty' "$OUT/show-held.json" | sed -E 's#^[a-z]+://##; s#[:/].*$##')" 2> /dev/null | tail -1)"
OVERHEAD_ON=$NAME
overhead_of echo minimax-h3-tools/echo text
overhead_of write qwen3.5/write instructions --model model=qwen/qwen3.5-0.8b@0.1.0 max_tokens:=4 seed:=7 temperature:=0
cat "$OUT"/overhead-echo.txt "$OUT"/overhead-write.txt >> "$R" 2> /dev/null
[ ! -f /tmp/cozy-hops.log ] || cp /tmp/cozy-hops.log "$OUT/hops.log"
cozy machine logs "$NAME" --since 2h -n 0 > "$OUT/machine.log" 2>&1
{ echo "## run-overhead on $NAME ($SKU)"; tail -qn 1 "$OUT"/overhead-echo.txt "$OUT"/overhead-write.txt 2> /dev/null | sed 's/^/- /'; } >> "${GITHUB_STEP_SUMMARY:-/dev/null}"
exit "$RC"
