# cut-1254 (cl-1254, cozy-creator #3950): the released upgrade path at the cut, after PyPI serves the release's Runtime and TensorFS and
# before Latest moves. This runner's cozy is the Latest a person has (pod-session `cozy`: released), its `local` on the Latest's pair and a
# project with a box; then `cozy update <to>` (the tagged prerelease) and three legs, each from local on the Latest's pair:
#   render  `cozy render upgraded --on local` (the page's route): "updating local first (… runtime …)", and the take lands
#   page    Calcifer's page in Chrome (e2e/cli/web/cut-1254.spec.js): Render pressed on a box in Focus, "Updating local first", its picture
#   old     `cozy run ./draw.py/draw --on local`: local code floored at cozy-runtime>=0.43.1, which the Latest's Runtime meets (W-S2)
#   held    last, local held by hand on the Latest's pair (no let-go): a render must still be refused `runtime_held`
# Each move leg's green also needs `machine show local` on the new cozy and a Runtime of <runtime> after it. The box's tool is draw.py, a
# weightless still (CPU). Rents nothing; `rent` is not read.
# params:
#   to       the cozy `cozy update` installs (default 0.27.0)
#   runtime  what local's Runtime must start with after each move leg (default 0.48.0)
#   place    instead of `cozy update <to>`, a build put in place as `cozy update` puts it (binary kept as .cozy-previous, Calcifer
#            restarted by its service), then the update `cozy update`'s activate sends, by the Latest's CLI: `machine update local
#            --tensord <its version>` (activate also asks PyTorch, which no hold reads). `release:<version>` (that release's archive,
#            held to its sha256.sum) or `dev:<dev-build run id>` (its cozy-* artifact): a build with no signed archive to `cozy update` to
set -uo pipefail
HERE=$(cd "$(dirname "$0")" && pwd)
ROOT=$(cd "$HERE/../.." && pwd)
WEB=$ROOT/e2e/cli/web
TO=$(jq -r '.to // "0.27.0"' <<< "$PARAMS") WANT=$(jq -r '.runtime // "0.48.0"' <<< "$PARAMS") PLACE=$(jq -r '.place // ""' <<< "$PARAMS")
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
  [ "$(jq -r .version <<< "$p")" = "$TO" ] && [[ $(jq -r .runtime <<< "$p") == "$WANT"* ]] || red "$1: local is not on cozy $TO, Runtime $WANT: $p"
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

if [ -z "$PLACE" ]; then
  cozy update "$TO" -y > "$OUT/update.txt" 2>&1 || fail "cozy update $TO: $(tail -c 400 "$OUT/update.txt")"
else
  NEW=$OUT/placed && rm -rf "$NEW" && mkdir -p "$NEW" || fail "no $NEW"
  case $PLACE in
    release:*)
      v=${PLACE#release:} a=cozy-x86_64-unknown-linux-gnu.tar.gz
      curl -fsSL --retry 5 -o "$NEW/$a" "https://github.com/cozy-creator/cozy-creator/releases/download/v$v/$a" &&
        curl -fsSL --retry 5 -o "$NEW/sha256.sum" "https://github.com/cozy-creator/cozy-creator/releases/download/v$v/sha256.sum" &&
        (cd "$NEW" && awk -v f="$a" '{ n = $2; sub(/^\*/, "", n) } n == f' sha256.sum | sha256sum -c --strict -) && tar -xzf "$NEW/$a" -C "$NEW" ||
        fail "release $v's $a"
      bin=$(find "$NEW" -type f -name cozy -print -quit) ;;
    dev:*)
      gh run download "${PLACE#dev:}" -R cozy-creator/cozy-creator -p 'cozy-*' -D "$NEW" > "$OUT/placed.log" 2>&1 || fail "dev-build ${PLACE#dev:}: $(tail -c 300 "$OUT/placed.log")"
      bin=$(find "$NEW" -type f -name cozy-linux-x86_64 -print -quit) ;;
    *) fail "place is release:<version> or dev:<run id>, not $PLACE" ;;
  esac
  [ -n "${bin:-}" ] && chmod 755 "$bin" && TO=$("$bin" --version | awk '{print $2}') && [ -n "$TO" ] || fail "no cozy in $PLACE"
  # As `cozy update` puts it: the old binary kept beside it, the new one renamed over it, Calcifer restarted by its service.
  COZY=$(command -v cozy) BINDIR=$(dirname "$(command -v cozy)")
  cp "$bin" "$BINDIR/.cozy-update-cut" && { ln -f "$COZY" "$BINDIR/.cozy-previous" 2> /dev/null || cp "$COZY" "$BINDIR/.cozy-previous"; } &&
    mv -f "$BINDIR/.cozy-update-cut" "$COZY" && systemctl --user restart cozy-calcifer > "$OUT/update.txt" 2>&1 &&
    "$BINDIR/.cozy-previous" --json machine ls >> "$OUT/update.txt" 2>&1 || fail "$TO not put in place: $(tail -c 300 "$OUT/update.txt")"
  say "setup: cozy $TO put in place ($PLACE); before activate's update, local: $(pair), held: $(held)"
  # `cozy update`'s activate, by the Latest's CLI: local to the cozy installed here.
  "$BINDIR/.cozy-previous" --json machine update local --tensord "$TO" -y >> "$OUT/update.txt" 2>&1 ||
    fail "the Latest's CLI: machine update local --tensord $TO: $(tail -c 300 "$OUT/update.txt")"
  say "setup: activate's update by cozy $LATEST's CLI: $(grep -oE '"target":"[^"]*"' "$OUT/update.txt" | tail -1)"
fi
[ "$(cozy --version)" = "cozy $TO" ] || fail "cozy is $(cozy --version) after cozy update $TO"
# Calcifer restarts on the new binary at once; local moves to it once its runs end.
for _ in $(seq 120); do [ "$(pair | jq -r .version)" = "$TO" ] && break; sleep 5; done
ready || fail "local not ready after cozy update $TO"
p=$(pair)
say "setup: cozy $TO: $(grep -E '^cozy .* → ' "$OUT/update.txt" | head -1 | cut -c1-120); local: $p, held: $(held)"
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

# 4. A hold by hand still holds: local on the Latest's pair by hand, not let go, and a new box rendered there is refused.
cozy --json machine update local --runtime "$OLD_RT" --tensorfs "$OLD_TFS" -y > "$OUT/hold.json" 2>&1 && ready ||
  red "held leg: local not held on $OLD_RT / $OLD_TFS: $(tail -c 300 "$OUT/hold.json")"
say "held leg: before it, local: $(pair), held: $(held)"
printf 'held:\n  %s:\n    instructions: A still while local is held %s\n' "$P/draw.py/draw" "$RANDOM" > "$P/h.yaml"
(cd "$P" && cozy apply -f h.yaml && timeout 600 cozy render held --on local -y) > "$OUT/held.out" 2> "$OUT/held.err"; code=$?
say "held leg: exit $code: $(cat "$OUT/held.err" | head -2 | tr '\n' ' ' | cut -c1-300)"
grep -qs 'error\[runtime_held\]' "$OUT/held.err" || red "held leg: a render on local held by hand was not refused runtime_held (exit $code)"
p=$(pair)
say "held leg: after it, local: $p"
[ "$(jq -r .runtime <<< "$p")" = "$OLD_RT" ] || red "held leg: local held on $OLD_RT moved to $(jq -r .runtime <<< "$p")"

say "$([ "$RED" = 0 ] && echo GREEN || echo RED): cut-1254 on cozy $TO from $LATEST, local from Runtime $OLD_RT"
exit "$RED"
