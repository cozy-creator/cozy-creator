# post-promotion (at-promotion.md "POST-PROMOTION SMOKE", cl-1254's leg 4): a promoted release as its users meet it, on prod.
# params:
#   part     fresh    this runner's cozy as the documented install gives it (pod-session `cozy`: released): it must be `version`;
#                     `cozy new`, then a published package on local (birefnet/remove_background on release-gate/ref/seed.jpg)
#            upgrade  this runner's cozy an older release (pod-session `cozy`: v0.26.1), local used on it first; a plain `cozy update`
#                     (no version) must land on `version`, then a render on local by the CLI and by the page (e2e/cli/web/
#                     cut-1254.spec.js, its project pinned to local). Then leg 4: local put back on the old pair by hand (a render is
#                     refused runtime_held: the hand's hold wins), let go (it takes its Hub's pair, the pin), and a render
#            rental   the cheapest rental ($SKU) and one published GPU tool on it (`tool`, default sdxl/generate); rents only with
#                     RENT=true (lib.sh `quote`, `rent`; its trap ends the pod)
#            existing this runner's cozy an older release (pod-session `cozy`: v0.26.1): local given its own pair by a run of a published
#                     package capped below the new Runtime (`old`, default birefnet@0.9.0, CPU), so not held; then, still on the old
#                     cozy: (a) that package again and the same package unpinned, (b) local code (draw.py), (c) what the old cozy says at
#                     start; then one plain `cozy update` and whatever failed again
#   version  the promoted cozy (default 0.27.1)
set -uo pipefail
HERE=$(cd "$(dirname "$0")" && pwd)
PART=$(jq -r '.part // ""' <<< "$PARAMS") VERSION=$(jq -r '.version // "0.27.1"' <<< "$PARAMS")
if [ "$PART" = rental ]; then
  . "$HERE/lib.sh"
  TOOL=$(param tool sdxl/generate)
  quote 900
  rent
  say "rental: $NAME, $(jq -c '.status.gpus | map(.name)' "$OUT/show.json" 2> /dev/null), cozy $(jq -r .status.version "$OUT/show.json" 2> /dev/null), Runtime $(jq -r .status.runtime "$OUT/show.json" 2> /dev/null)"
  (cd "$RUN_PROJECT" && timeout 1200 cozy run "$TOOL" "prompt=A lighthouse on a sea cliff at dusk $RANDOM" --on "$NAME") > "$OUT/rental-run.txt" 2>&1; code=$?
  say "rental: cozy run $TOOL --on $NAME: exit $code: $(tail -n 3 "$OUT/rental-run.txt" | tr '\n' ' ' | cut -c1-300)"
  [ "$code" = 0 ] || fail "$TOOL on $NAME"
  exit 0
fi

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
P=$OUT/project DRAW=$HERE/fixtures/cut-1254/draw.py
say "post-promotion $PART on a GitHub runner: cozy $(cozy --version | awk '{print $2}'); the Hub's pin: $(curl -fsS https://tensorhub.com/v1/software | jq -c '{cozy, runtime, tensorfs}'); GitHub Latest: $(curl -fsS https://api.github.com/repos/cozy-creator/cozy-creator/releases/latest | jq -r .tag_name)"

case $PART in
  fresh)
    [ "$(cozy --version)" = "cozy $VERSION" ] || red "fresh: the documented install gave $(cozy --version), not $VERSION"
    mkdir -p "$P" && (cd "$P" && cozy --json new --unencrypted .) > "$OUT/new.json" 2>&1 || fail "cozy new: $(tail -c 300 "$OUT/new.json")"
    say "fresh: cozy new: $(jq -r '.path // .project.path // empty' "$OUT/new.json" 2> /dev/null | head -1)"
    (cd "$P" && timeout 1200 cozy run birefnet/remove_background --file "image=$REF/seed.jpg" --on local) > "$OUT/birefnet.out" 2> "$OUT/birefnet.err"; code=$?
    say "fresh: cozy run birefnet/remove_background --on local: exit $code: $(tail -n 3 "$OUT/birefnet.out" | tr '\n' ' ' | cut -c1-300)"
    [ "$code" = 0 ] || red "fresh: birefnet on local: $(tail -c 400 "$OUT/birefnet.err")"
    grep -hE '^(run|  installing|  downloading)' "$OUT/birefnet.err" | head -6 | sed 's/^/    /' | tee -a "$R"
    ready; say "fresh: local: $(pair), held: $(held)"
    ;;
  upgrade)
    OLD=$(cozy --version | awk '{print $2}')
    ready || fail "local not ready on cozy $OLD"
    say "upgrade: cozy $OLD's local at first: $(pair), held: $(held)"
    mkdir -p "$P" && cp "$DRAW" "$P/draw.py" && (cd "$P" && cozy --json new --unencrypted .) > "$OUT/new.json" 2>&1 || fail "cozy new: $(tail -c 300 "$OUT/new.json")"
    # local used on the old cozy: a run there, which gives it its pair as that cozy does now.
    (cd "$P" && timeout 900 cozy run ./draw.py/draw "instructions=Used on cozy $OLD $RANDOM" --on local) > "$OUT/used.out" 2> "$OUT/used.err"; code=$?
    ready; say "upgrade: a run on cozy $OLD's local: exit $code; local: $(pair), held: $(held)"
    printf 'upgraded:\n  %s:\n    instructions: A still after cozy update %s\n' "$P/draw.py/draw" "$RANDOM" > "$P/n.yaml"
    (cd "$P" && cozy apply -f n.yaml) > "$OUT/apply.txt" 2>&1 || fail "cozy apply: $(tail -c 300 "$OUT/apply.txt")"
    cozy update -y > "$OUT/update.txt" 2>&1 || fail "cozy update: $(tail -c 400 "$OUT/update.txt")"
    for _ in $(seq 120); do [ "$(pair | jq -r .version)" = "$VERSION" ] && break; sleep 5; done
    ready
    say "upgrade: plain cozy update: $(grep -E '^cozy .* → ' "$OUT/update.txt" | head -1 | cut -c1-120); now $(cozy --version); local: $(pair), held: $(held)"
    [ "$(cozy --version)" = "cozy $VERSION" ] || red "upgrade: cozy update landed on $(cozy --version), not $VERSION"
    (cd "$P" && timeout 900 cozy render upgraded --on local -y) > "$OUT/render.out" 2> "$OUT/render.err"; code=$?
    say "upgrade: CLI render on local: exit $code: $(cat "$OUT/render.out" "$OUT/render.err" | head -3 | tr '\n' ' ' | cut -c1-260)"
    [ "$code" = 0 ] || red "upgrade: cozy render on local failed"
    ready; say "upgrade: after the CLI render, local: $(pair)"
    PW=$OUT/playwright-test
    npm install -s --prefix "$PW" @playwright/test@1.55.0 > "$OUT/npm.log" 2>&1 || fail "npm install @playwright/test: $(tail -c 300 "$OUT/npm.log")"
    mkdir -p "$OUT/page"
    FLOW_SPEC=cut-1254.spec.js CUT_OUT=$OUT/page DRAW=$DRAW/draw HOLDER=$HOME/.cozy/calcifer/calcifer.lock.holder COZY=$(command -v cozy) \
      CHROME=$(command -v google-chrome) OUT=$OUT ARGV_LOG=$OUT/argv.log NODE_PATH=$PW/node_modules \
      timeout 1500 node "$PW/node_modules/@playwright/test/cli.js" test -c "$HERE/../../e2e/cli/web/flow.config.js" > "$OUT/page.log" 2>&1
    say "upgrade: page render on local: $(jq -c '{failure, take: (.take | if . then {machine: .main.machine, width: .main.width} else null end)}' "$OUT/page/page.json" 2> /dev/null || echo 'no page.json')"
    jq -e '.failure == null and (.take.main.width // 0) > 0' "$OUT/page/page.json" > /dev/null 2>&1 || red "upgrade: the page render on local didn't land"
    ready; say "upgrade: after the page render, local: $(pair)"
    # Leg 4: the old pair by hand holds; let go, local takes its Hub's pair, the pin.
    OLD_RT=0.47.1 OLD_TFS=0.26.1
    cozy --json machine update local --runtime "$OLD_RT" --tensorfs "$OLD_TFS" -y > "$OUT/leg4-back.json" 2>&1 && ready ||
      red "leg 4: local not put back on $OLD_RT / $OLD_TFS: $(tail -c 300 "$OUT/leg4-back.json")"
    say "leg 4: put back by hand: local $(pair), held: $(held)"
    printf 'leg4:\n  %s:\n    instructions: A still on the old pair %s\n' "$P/draw.py/draw" "$RANDOM" > "$P/l.yaml"
    (cd "$P" && cozy apply -f l.yaml && timeout 600 cozy render leg4 --on local -y) > "$OUT/leg4-held.out" 2> "$OUT/leg4-held.err"; code=$?
    say "leg 4: render while held by hand: exit $code: $(head -1 "$OUT/leg4-held.err" | cut -c1-240)"
    grep -qs 'error\[runtime_held\]' "$OUT/leg4-held.err" || red "leg 4: a render on local held by hand was not refused runtime_held"
    cozy --json machine update local -y > "$OUT/leg4-go.json" 2>&1 && ready || red "leg 4: let go: $(tail -c 300 "$OUT/leg4-go.json")"
    say "leg 4: let go: its update $(grep -oE '"target":"[^"]*"' "$OUT/leg4-go.json" | tail -1); local $(pair), held: $(held)"
    [ "$(pair | jq -r .runtime)" = "$(curl -fsS https://tensorhub.com/v1/software | jq -r .runtime)" ] || red "leg 4: let go, local isn't on its Hub's Runtime"
    (cd "$P" && timeout 600 cozy render leg4 --on local -y) > "$OUT/leg4.out" 2> "$OUT/leg4.err"; code=$?
    say "leg 4: render after it: exit $code: $(cat "$OUT/leg4.out" | head -1 | cut -c1-200); local $(pair)"
    [ "$code" = 0 ] || red "leg 4: render on local after let-go failed: $(head -2 "$OUT/leg4.err" | tr '\n' ' ' | cut -c1-300)"
    ;;
  existing)
    OLD=$(cozy --version | awk '{print $2}') PKG=$(jq -r '.old // "birefnet@0.9.0"' <<< "$PARAMS")
    ready || fail "local not ready on cozy $OLD"
    mkdir -p "$P" && cp "$DRAW" "$P/draw.py" && (cd "$P" && cozy --json new --unencrypted .) > "$OUT/new.json" 2>&1 || fail "cozy new: $(tail -c 300 "$OUT/new.json")"
    try() {  # try <name> <cozy run args...>: one run on local, its exit and its last words
      local name=$1 code; shift
      (cd "$P" && timeout 900 cozy run "$@" --on local) > "$OUT/$name.out" 2> "$OUT/$name.err"; code=$?
      ready
      say "existing: $name: cozy run $*: exit $code: $(grep -hE '^(error|cause|next)' "$OUT/$name.err" | head -3 | tr '\n' ' ' | cut -c1-400)$([ "$code" = 0 ] && tail -n 1 "$OUT/$name.out" | cut -c1-120); local $(pair), held: $(held)"
      grep -hE 'Updating|updating|moves? local|Runtime' "$OUT/$name.err" | head -3 | sed 's/^/    /' | tee -a "$R"
      return "$code"
    }
    say "existing: cozy $OLD's local at first: $(pair), held: $(held)"
    # The state an existing user's local is in: its own pair 0.47.1 / 0.26.1, not held. A package's run gives its environment a Runtime,
    # not the machine a pair; local code capped below the new Runtime (draw047.py) moves the machine to one (Calcifer's own move: no hold).
    cp "$HERE/fixtures/cut-1254/draw047.py" "$P/draw047.py"
    try setup ./draw047.py/draw "instructions=Its first pair on cozy $OLD $RANDOM" || true
    p=$(pair)
    if [ "$(jq -r .runtime <<< "$p")" = 0.47.1 ] && [ "$(held)" = none ]; then say "existing: built: local on its own pair 0.47.1, not held"
    else
      say "existing: NOT the existing user's state after a capped run: local $p, held $(held); nearest: the pair put there by hand (a hold)"
      cozy --json machine update local --runtime 0.47.1 --tensorfs 0.26.1 -y > "$OUT/hand.json" 2>&1 && ready ||
        fail "the pair by hand: $(tail -c 300 "$OUT/hand.json")"
      say "existing: by hand: local $(pair), held: $(held)"
    fi
    # (c) What the old cozy says at start, and of its own version.
    for c in status "machine ls"; do (cd "$P" && cozy $c) > "$OUT/start-${c// /-}.txt" 2>&1; done
    say "existing: (c) cozy status / machine ls say of an update: $(grep -hiE 'update|0\.27' "$OUT/start-status.txt" "$OUT/start-machine-ls.txt" | head -4 | tr '\n' ' ' | cut -c1-400)"
    # (a) the capped package again, and the same package unpinned; (b) local code.
    a1=0 a2=0 b=0
    try a-capped "${PKG%%@*}/remove_background@${PKG#*@}" --file "image=$REF/seed.jpg" || a1=1
    try a-unpinned "${PKG%%@*}/remove_background" --file "image=$REF/seed.jpg" || a2=1
    try b-local-code ./draw.py/draw "instructions=Local code on cozy $OLD $RANDOM" || b=1
    say "existing: on cozy $OLD: (a) capped $([ $a1 = 0 ] && echo works || echo FAILS), unpinned $([ $a2 = 0 ] && echo works || echo FAILS); (b) local code $([ $b = 0 ] && echo works || echo FAILS)"
    [ $a1$a2$b = 000 ] || red "existing: an existing user's local fails before they update (above)"
    cozy update -y > "$OUT/update.txt" 2>&1 || red "existing: cozy update: $(tail -c 300 "$OUT/update.txt")"
    for _ in $(seq 120); do [ "$(pair | jq -r .version)" = "$VERSION" ] && break; sleep 5; done
    ready; say "existing: plain cozy update: $(grep -E '^cozy .* → ' "$OUT/update.txt" | head -1 | cut -c1-80); local $(pair), held: $(held)"
    [ $a1 = 0 ] || try a-capped-after "${PKG%%@*}/remove_background@${PKG#*@}" --file "image=$REF/seed.jpg" || red "existing: (a) capped still fails after cozy update"
    [ $a2 = 0 ] || try a-unpinned-after "${PKG%%@*}/remove_background" --file "image=$REF/seed.jpg" || red "existing: (a) unpinned still fails after cozy update"
    [ $b = 0 ] || try b-local-code-after ./draw.py/draw "instructions=Local code after cozy update $RANDOM" || red "existing: (b) still fails after cozy update"
    # A hand's hold outlives `cozy update` by design; what its refusal names, `cozy machine update local`, lets it go.
    if [ "$(held)" != none ]; then
      cozy --json machine update local -y > "$OUT/letgo.json" 2>&1 && ready || red "existing: let go: $(tail -c 300 "$OUT/letgo.json")"
      say "existing: then \`cozy machine update local\` (the refusal's next): $(grep -oE '"target":"[^"]*"' "$OUT/letgo.json" | tail -1); local $(pair), held: $(held)"
      try a-unpinned-letgo "${PKG%%@*}/remove_background" --file "image=$REF/seed.jpg" || red "existing: (a) unpinned fails after the let-go"
      try b-local-code-letgo ./draw.py/draw "instructions=Local code after the let-go $RANDOM" || red "existing: (b) fails after the let-go"
    fi
    ;;
  *) fail "part is fresh, upgrade, existing or rental, not '$PART'" ;;
esac
say "$([ "$RED" = 0 ] && echo GREEN || echo RED): post-promotion $PART"
exit "$RED"
