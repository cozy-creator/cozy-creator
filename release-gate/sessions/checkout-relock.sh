# checkout-relock (cl-1199; Predictive Downloads' 44175857630: each RC Runtime merge re-locked a dev home's checkout package, which
# reinstalled it on the rental and dropped its box's finished draft). A dev home runs the packages checkout's sdxl, whose floor only the
# open RC meets (`cozy-runtime>=0.48.0.dev0`), on one rental held by hand on the RC's builds of the moment, so nothing on the pod moves.
# Its box's draft runs ahead; the session waits for the RC's next Runtime build, then renders the box:
#   1. relock: from the draft's end to the pyproject change, a call of the package sends it no new source (a lock moved for unchanged
#              code is a new manifest, which an `install local/<pkg>` run installs) and the pod builds its environment no more
#              (TensorD's install line);
#   2. draft:  the render adopts the run the draft finished before that build;
#   3. inputs: the package's pyproject.toml takes a dependency: two calls, exactly one new source (one lock) and one install.
# params:
#   tree     a packages_refs checkout (`-f packages_refs=pk=<ref>`, tree pk): the dev home's packages
#   tool     the function (default sdxl/generate); `fields`, its box's (default a prompt)
#   wait_s   how long it waits for the RC's next Runtime build (default 3000); none comes: it ends, inconclusive
. "$(dirname "$0")/lib.sh"
TREE=$(param tree '') TOOL=$(param tool sdxl/generate) WAIT=$(param wait_s 3000) BEGAN=$(date +%s)
PKG=${TOOL%%/*} PROMPT="a brass compass on a walnut desk"
FIELDS=$(param fields "$(jq -nc --arg p "$PROMPT" '{prompt: $p}')")
[ -f "$PACKAGES/$TREE/$PKG/pyproject.toml" ] || fail "no $PKG in the packages checkout $TREE"
SOURCE=$(cd "$PACKAGES/$TREE" && pwd)
V=$(cozy --version | awk '{print $2}')
[[ $V == *-dev* ]] || fail "cozy $V is no RC build: dispatch with cozy=<a dev-build run of rc/<x> or a branch of it>"
SUMS=https://github.com/cozy-creator/cozy-runtime/releases/download/rc-${V%%-*}/SHA256SUMS
# newest: the RC's newest Runtime build, as its SHA256SUMS names it first (rc-dev.yaml).
newest() { curl -fsSL --retry 3 "$SUMS" | awk '$2 ~ /^cozy_runtime-.*-py3-none-any\.whl$/ { split($2, f, "-"); print f[2]; exit }'; }
cfg=$HOME/.cozy/config.toml
mkdir -p "${cfg%/*}" && touch "$cfg" || fail "no $cfg"
{ echo 'dev = true'; grep -v '^dev *=' "$cfg" || true; } > "$cfg.new" && mv "$cfg.new" "$cfg" || fail "dev = true not written"
awk -v source="$SOURCE" '
  function ours() { print "packages = \"" source "\""; put = 1 }
  /^packages *=/ { next } { print } /^\[calcifer\]/ { ours() } END { if (!put) { printf "\n[calcifer]\n"; ours() } }' \
  "$cfg" > "$cfg.next" && mv "$cfg.next" "$cfg" || fail "[calcifer] not written"
cozy restart > "$OUT/restart.txt" 2>&1 || fail "cozy restart: $(tail -c 300 "$OUT/restart.txt")"
kernels "$SOURCE/$PKG"
say "cozy $V, a dev home on $SOURCE; $PKG needs $(grep -o '"cozy-runtime[^"]*"' "$SOURCE/$PKG/pyproject.toml"), its lock $(grep -A1 '^name = "cozy-runtime"' "$SOURCE/$PKG/uv.lock" | tail -1)"
# Off by default since Paul 11-18: the pod runs a draft ahead only with its config's dev knob (`pod_speculates`, over ssh).
[ -f ~/.ssh/id_ed25519.pub ] || ssh-keygen -q -t ed25519 -N '' -f ~/.ssh/id_ed25519 || fail "ssh-keygen"
RENT_ARGS=--ssh
quote "$(param need_s 4200)"
rent
# Moved to the RC's builds as the dev home moves a rental at boot, then held there by hand with this session's TensorD, dedicated and
# speculating (a draft runs ahead only there): a new RC build moves nothing on the pod.
for _ in $(seq 72); do [ "$(cozy machine show "$NAME" --json 2> /dev/null | jq -r '.rc.state // empty')" = current ] && break; sleep 5; done
[ "$(cozy machine show "$NAME" --json | jq -r '.rc.state // empty')" = current ] || step rc machine update "$NAME" -y || fail "$NAME didn't move to the RC"
pod_speculates
step held machine update "$NAME" --tensord "$(command -v cozy)" --mode dedicated -y || fail "the pod refused the session's TensorD"
cozy machine show "$NAME" --json > "$OUT/held.json"
[ "$(jq -r '.rc.state // empty' "$OUT/held.json")" = held ] || fail "$NAME isn't held: $(jq -c .rc "$OUT/held.json")"
read -r WANT WANT_RT WANT_FS < <(jq -r '"\(.status.version) \(.status.runtime) \(.status.tensorfs)"' "$OUT/held.json")
say "pod held on TensorD $WANT, Runtime $WANT_RT, TensorFS $WANT_FS"
ours() {
  local v; v=$(cozy machine show "$NAME" --json 2> /dev/null | jq -r '"\(.status.version) \(.status.runtime) \(.status.tensorfs)"')
  [ "$v" = "$WANT $WANT_RT $WANT_FS" ] || fail "$1: the pod runs $v, not $WANT $WANT_RT $WANT_FS"
}
( while :; do
    printf '{"at_ms":%s,"status":%s}\n' "$(date +%s%3N)" \
      "$(cozy machine show "$NAME" --json 2> /dev/null | jq -c '{ahead: (.status.ahead // [])}' 2> /dev/null || echo null)" >> "$OUT/status.ndjson"
    sleep 1
  done ) & BG=$!
mark() { echo "$(date +%s%3N) $1" >> "$OUT/marks.txt"; }
# call <name> <seed>: the package called in the session's project.
call() { mark "$1"; step "$1" run "$TOOL" --on "$NAME" prompt="$PROMPT" seed:="$2" || fail "$1: $(tail -c 300 "$OUT/$1.txt")"; }
item() { tail -1 "$OUT/status.ndjson" | jq -r '[.status.ahead[]? | select(.id | startswith("cozy:prepare:")) | "\(.id) \(.readiness.stage // "")"] | first // ""'; }
until_stage() {
  local t=0 now=''
  until now=$(item); [ -n "$now" ] && [[ ${now#* } =~ ^($1)$ ]]; do
    sleep 1; t=$((t + 1)); [ "$t" -lt "$2" ] || fail "the box's draft wasn't $1 within $2 s: $(tail -1 "$OUT/status.ndjson" | cut -c1-400)"
  done
  ITEM=${now%% *}; say "the box's draft is ${now#* } ($t s; ${ITEM#cozy:prepare:})"
}

ours fetch
call fetch 1
python3 "$RUN_PAGE" show "$RUN_PROJECT" > "$OUT/shown.txt" 2>&1 & SHOW=$!; BG="$BG $SHOW"
for _ in $(seq 30); do grep -q '"held"' "$OUT/shown.txt" && break; sleep 1; done
grep -q '"held": \["' "$OUT/shown.txt" || fail "the project wasn't shown: $(tail -c 300 "$OUT/shown.txt")"
mark draft
run_page add q --tool "$TOOL" --fields "$FIELDS" > "$OUT/box.json" 2>&1 || fail "the box: $(tail -c 300 "$OUT/box.json")"
until_stage 'loaded|running|done' 600
until_stage done 300
DRAFT=$ITEM; ours drafted; mark done

# The RC's next Runtime build. A person keeps the pod with `keepalive` meanwhile; the page stays shown.
B0=$(newest); [ -n "$B0" ] || fail "no Runtime build in $SUMS"
say "the RC's newest Runtime build: $B0; waiting up to $WAIT s for its next"
t=0 B=$B0
while [ "$B" = "$B0" ]; do
  [ "$t" -lt "$WAIT" ] || fail "INCONCLUSIVE: no new RC Runtime build in $WAIT s"
  [ $((t % 600)) != 0 ] || cozy machine keepalive "$NAME" > /dev/null 2>&1 || say "keepalive refused"
  sleep 60; t=$((t + 60)); B=$(newest); B=${B:-$B0}
done
mark build; say "the RC's Runtime build $B appeared ($t s); Calcifer reads it within 20 s"
sleep 180
now=$(item); say "the box's draft 3 min after: ${now:-none}"
ours after-build
call p2 3
mark render
python3 "$RUN_PAGE" render "$RUN_PROJECT" --on "$NAME" q > "$OUT/render.txt" 2>&1 || fail "the render: $(tail -c 400 "$OUT/render.txt")"
mark rendered
kill "$SHOW" 2> /dev/null; sleep 5

# The package's own inputs change: a dependency it has anyway (`packaging`, the Runtime's), declared.
python3 -I - "$SOURCE/$PKG/pyproject.toml" <<'PY' || fail "the pyproject wasn't changed"
import re, sys
path = sys.argv[1]; text = open(path).read()
changed = re.sub(r'(?m)^dependencies = \[\n', 'dependencies = [\n    "packaging",\n', text, count=1)
assert changed != text, "no dependencies list"
open(path, "w").write(changed)
PY
mark inputs
call p3 4
call p4 5
ours end
kill $BG 2> /dev/null; BG=''

cozy machine logs "$NAME" --lines 0 --since "$(( ($(date +%s) - BEGAN) / 60 + 2 ))m" > "$OUT/tensord.log" 2>&1 || say "the pod's TensorD log wasn't read"
cozy machine logs local calcifer --lines 20000 > "$OUT/calcifer.log" 2>&1 || say "Calcifer's log wasn't read"
(cd "$RUN_PROJECT" && cozy --json run ls --all --limit 0) > "$OUT/runs.json" 2>&1 || say "the project's runs weren't listed"
python3 -I - "$OUT" "$PKG" "$DRAFT" <<'PY' | tee -a "$R"; [ "${PIPESTATUS[0]}" = 0 ] || RC=1
import re, sys
from datetime import datetime
out, pkg, draft = sys.argv[1:]
marks = {}
for line in open(f"{out}/marks.txt"):
    at, what = line.split(maxsplit=1)
    marks.setdefault(what.strip(), int(at) / 1000)
def at(line):
    m = re.search(r"\d{4}-\d\d-\d\dT[\d:.]+Z", line)
    return datetime.fromisoformat(m.group(0)[:26].rstrip("Z") + "+00:00").timestamp() if m else None
def lines(file, pattern):
    return [(at(l), l.strip()) for l in open(f"{out}/{file}", errors="replace") if re.search(pattern, l) and at(l)]
import json
listed = json.load(open(f"{out}/runs.json")).get("runs", [])
locks = [(r["created_at_ms"] / 1000, f'run {r["number"]} {r["target"]}') for r in listed if r.get("target", "").startswith(f"install local/{pkg}")]
installs = lines("tensord.log", rf'install line: its turn .*what="local/{re.escape(pkg)} ')
guesses = lines("tensord.log", r"a guess runs as if it were sent")
adopts = lines("tensord.log", r"adopts the speculative run")
def within(rows, a, b):
    return [l for t, l in rows if marks[a] <= t < marks[b]]
ok = True
def check(passed, said):
    global ok
    ok &= passed
    print(("PASS " if passed else "FAIL ") + said)
relocked, reinstalled = within(locks, "done", "inputs"), within(installs, "done", "inputs")
check(not relocked and not reinstalled, f"relock: from the draft's end to the pyproject change, {len(relocked)} new sources and {len(reinstalled)} installs of {pkg}"
      + "".join(f"\n    {l[:240]}" for l in relocked + reinstalled))
ran = [l for t, l in guesses if t < marks["done"]]
run = re.search(r"\(machine run (\d+)\)", ran[-1]).group(1) if ran else None
took = [l for t, l in adopts if t >= marks["render"]]
check(bool(run) and any(f"(machine run {run}, " in l and "ompleted" in l for l in took),
      f"draft: the render adopts the draft's finished machine run {run} ({draft})" + "".join(f"\n    {l[:240]}" for l in took or ["no adoption"]))
after, built = [l for t, l in locks if t >= marks["inputs"]], [l for t, l in installs if t >= marks["inputs"]]
check(len(after) == 1 and len(built) == 1, f"inputs: after the pyproject change, {len(after)} new sources and {len(built)} installs for two calls"
      + "".join(f"\n    {l[:240]}" for l in after + built))
sys.exit(0 if ok else 1)
PY
[ "$RC" = 0 ] && say "PASS: a new RC Runtime build re-locked nothing, the draft was adopted; the pyproject change locked once"
