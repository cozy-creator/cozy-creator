# cut-1254 (cl-1254, cozy-creator #3950): the released upgrade path at the cut, after PyPI serves the release's Runtime and TensorFS and
# before Latest moves. This runner's cozy is the Latest a person has (pod-session `cozy`: released), its `local` on the Latest's pair and a
# project with a box; then `cozy update <to>` (the tagged prerelease) and three legs, each from local on the Latest's pair:
#   render  `cozy render upgraded --on local` (the page's route): "updating local first (… runtime …)", and the take lands
#   page    Calcifer's page in Chrome (e2e/cli/web/cut-1254.spec.js): Render pressed on a box in Focus, "Updating local first", its picture
#   old     `cozy run ./draw.py/draw --on local`: local code floored at cozy-runtime>=0.43.1, which the Latest's Runtime meets (W-S2)
# Each leg's green also needs `machine show local` on cozy <to> and Runtime <runtime> after it. The box's tool is draw.py, a weightless
# still (CPU). Rents nothing; `rent` is not read.
# params:
#   to       the cozy to update to (default 0.27.0)
#   runtime  the Runtime each leg must move local to (default 0.48.0)
set -uo pipefail
HERE=$(cd "$(dirname "$0")" && pwd)
ROOT=$(cd "$HERE/../.." && pwd)
WEB=$ROOT/e2e/cli/web
TO=$(jq -r '.to // "0.27.0"' <<< "$PARAMS") WANT=$(jq -r '.runtime // "0.48.0"' <<< "$PARAMS")
mkdir -p "$OUT"
R=$OUT/report.txt RED=0
say() { echo "$(date -u +%T) $*" | tee -a "$R"; }
red() { say "RED: $*"; RED=1; }
fail() { say "FAIL: $*"; exit 1; }
pair() { cozy --json machine show local 2> /dev/null | jq -c '.status | {version, runtime, tensorfs}'; }
held() { cozy --json machine show local 2> /dev/null | jq -r '.held // "none"'; }
ready() {
  local _
  for _ in $(seq 120); do [ "$(cozy --json machine show local 2> /dev/null | jq -r '.state // empty')" = ready ] && return 0; sleep 5; done
  return 1
}
# back <leg>: local on the Latest's pair, then let go: in the window the Hub's pin is that pair, so nothing moves it.
back() {
  cozy --json machine update local --runtime "$OLD_RT" --tensorfs "$OLD_TFS" -y > "$OUT/back-$1.json" 2>&1 && ready &&
    cozy --json machine update local -y >> "$OUT/back-$1.json" 2>&1 && ready ||
    { red "$1: local not put back on $OLD_RT / $OLD_TFS: $(tail -c 300 "$OUT/back-$1.json")"; return 1; }
  say "$1: before it, local: $(pair), held: $(held)"
  [ "$(pair | jq -r .runtime)" = "$OLD_RT" ] || { red "$1: local is not on Runtime $OLD_RT before it"; return 1; }
}
# moved <leg>: local on cozy $TO and Runtime $WANT after it.
moved() {
  local p
  ready || red "$1: local not ready after it"
  p=$(pair)
  say "$1: after it, local: $p"
  [ "$(jq -r .version <<< "$p")" = "$TO" ] && [ "$(jq -r .runtime <<< "$p")" = "$WANT" ] || red "$1: local is not on cozy $TO, Runtime $WANT: $p"
}
# said <leg> <files...>: what the leg printed about local, and no runtime_outdated.
said() {
  local leg=$1 line; shift
  line=$(grep -ihoE "updating local first[^\"]*|Updating local's Runtime from [^\"]*" "$@" 2> /dev/null | head -1)
  say "$leg: said: ${line:-nothing about updating local}"
  [ -n "$line" ] || red "$leg: it never said it updates local first"
  ! grep -qs runtime_outdated "$@" || red "$leg: refused runtime_outdated: $(grep -hs runtime_outdated "$@" | head -1 | cut -c1-300)"
}

LATEST=$(cozy --version | awk '{print $2}')
say "cut-1254 on a GitHub runner: cozy $LATEST (the Latest), to $TO; each leg must move local to Runtime $WANT"
say "the Hub's pin: $(curl -fsS https://tensorhub.com/v1/software 2> /dev/null | jq -c '{runtime, tensorfs, cozy}' 2> /dev/null || echo unread)"
say "PyPI: cozy-runtime $(curl -fsS https://pypi.org/pypi/cozy-runtime/json | jq -r .info.version), tensorfs $(curl -fsS https://pypi.org/pypi/tensorfs/json | jq -r .info.version)"

# The Latest's user: local on the Latest's pair, a project with a box.
ready || fail "local not ready on cozy $LATEST"
cozy --json machine update local -y > "$OUT/setup-local.json" 2>&1 && ready || fail "cozy $LATEST's local onto its pair: $(tail -c 300 "$OUT/setup-local.json")"
p=$(pair) OLD_RT=$(pair | jq -r .runtime) OLD_TFS=$(pair | jq -r .tensorfs)
say "setup: cozy $LATEST's local: $p, held: $(held)"
P=$OUT/cut-1254 DRAW=$HERE/fixtures/cut-1254/draw.py
mkdir -p "$P" && cp "$DRAW" "$P/draw.py" || fail "no project folder"
(cd "$P" && cozy --json new --unencrypted .) > "$OUT/new.json" 2>&1 || fail "cozy new: $(tail -c 300 "$OUT/new.json")"
printf 'upgraded:\n  %s:\n    instructions: A still for the cut %s\n' "$P/draw.py/draw" "$RANDOM" > "$P/n.yaml"
(cd "$P" && cozy apply -f n.yaml) > "$OUT/apply.txt" 2>&1 || fail "cozy apply: $(tail -c 300 "$OUT/apply.txt")"
say "setup: project $P, box upgraded (draw.py), on cozy $LATEST"

cozy update "$TO" -y > "$OUT/update.txt" 2>&1 || fail "cozy update $TO: $(tail -c 400 "$OUT/update.txt")"
[ "$(cozy --version)" = "cozy $TO" ] || fail "cozy is $(cozy --version) after cozy update $TO"
# Calcifer restarts on the new binary at once; local moves to it once its runs end.
for _ in $(seq 120); do [ "$(pair | jq -r .version)" = "$TO" ] && break; sleep 5; done
ready || fail "local not ready after cozy update $TO"
p=$(pair)
say "setup: cozy update $TO: $(grep -E '^cozy .* → ' "$OUT/update.txt" | head -1 | cut -c1-120); local: $p, held: $(held)"
[ "$(jq -r .version <<< "$p")" = "$TO" ] && [ "$(jq -r .runtime <<< "$p")" = "$OLD_RT" ] || red "setup: local is not on cozy $TO with Runtime $OLD_RT: $p"

# 1. The render leg (the page's route).
say "render leg: cozy render upgraded --on local"
(cd "$P" && timeout 900 cozy render upgraded --on local -y) > "$OUT/render.out" 2> "$OUT/render.err"; code=$?
say "render leg: exit $code: $(tail -n 2 "$OUT/render.out" "$OUT/render.err" | grep -v '^==' | tr '\n' ' ' | cut -c1-300)"
[ "$code" = 0 ] || red "render leg: cozy render exit $code"
said "render leg" "$OUT/render.out" "$OUT/render.err"
(cd "$P" && cozy --json run ls --limit 3) 2> /dev/null | jq -c '[.runs[]? | {number, kind, status, machine, target: .target[0:60]}]' | tee -a "$R"
moved "render leg"

# 2. The page leg.
if back "page leg"; then
  PW=$OUT/playwright-test
  npm install -s --prefix "$PW" @playwright/test@1.55.0 > "$OUT/npm.log" 2>&1 || fail "npm install @playwright/test: $(tail -c 300 "$OUT/npm.log")"
  mkdir -p "$OUT/page"
  say "page leg: Calcifer's page in Chrome, Render pressed on a draw.py box in Focus"
  FLOW_SPEC=cut-1254.spec.js CUT_OUT=$OUT/page DRAW=$DRAW/draw HOLDER=$HOME/.cozy/calcifer/calcifer.lock.holder COZY=$(command -v cozy) \
    CHROME=$(command -v google-chrome) OUT=$OUT ARGV_LOG=$OUT/argv.log NODE_PATH=$PW/node_modules \
    timeout 1500 node "$PW/node_modules/@playwright/test/cli.js" test -c "$WEB/flow.config.js" > "$OUT/page.log" 2>&1; code=$?
  say "page leg: playwright exit $code; saw: $(jq -c '{said, at_ms, failure, take: (.take | if . then {width: .main.width, height: .main.height} else null end)}' "$OUT/page/page.json" 2> /dev/null || echo 'no page.json')"
  [ "$code" = 0 ] || red "page leg: the spec failed: $(grep -E '✘|Error:|expect' "$OUT/page.log" | head -4 | tr '\n' ' ' | cut -c1-400)"
  moved "page leg"
fi

# 3. The old-package leg: local code whose floor (cozy-runtime>=0.43.1) the Latest's Runtime meets.
if back "old-package leg"; then
  say "old-package leg: cozy run ./draw.py/draw --on local"
  (cd "$P" && timeout 900 cozy run ./draw.py/draw "instructions=An old package's still $RANDOM" --on local) > "$OUT/old.out" 2> "$OUT/old.err"; code=$?
  say "old-package leg: exit $code: $(tail -n 2 "$OUT/old.out" "$OUT/old.err" | grep -v '^==' | tr '\n' ' ' | cut -c1-300)"
  [ "$code" = 0 ] || red "old-package leg: cozy run exit $code"
  said "old-package leg" "$OUT/old.out" "$OUT/old.err"
  moved "old-package leg"
fi

say "$([ "$RED" = 0 ] && echo GREEN || echo RED): cut-1254 on cozy $TO from $LATEST, local from Runtime $OLD_RT"
exit "$RED"
