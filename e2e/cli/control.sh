# e2e/cli/control.sh: the control-plane audit's live bugs (cl-809..813), each a cell that fails without its fix, with the
# local machine up. Uses lifetime.sh's `launch` and `settled`.
#   machine_show_runs   cl-811: `cozy machine show` gives a held run's progress, not its status twice
#   writes_once         cl-812: a project write sent again under its key is done once: an undo resent steps once
#   page_resend         cl-812: the page's outbox sends a discard and an undo again after their answers are lost: each done once
#   hub_away            cl-809: locked local code that names the `tensorhub` index runs while the Hub doesn't answer
#   machine_age         cl-813: a dependency conflict uv explains through cozy-runtime is the package's, not the machine's age
#   broken_answers      cl-810: a machine that fails when asked about a run for over 120 s: the run ends as the machine says
here=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)

# A package of one module, `<module>.py` from stdin, in <dir> with <dependencies> (a TOML array's items) and, given, a
# [tool.uv.sources] table.
package_at() {
  local dir=$1 module=$2 deps=$3 sources=${4:-}
  mkdir -p "$dir" && cat > "$dir/$module.py"
  cat > "$dir/pyproject.toml" <<TOML
[project]
name = "${module//_/-}"
version = "0.0.0"
requires-python = ">=3.12"
dependencies = [$deps]

[project.entry-points."cozy.application"]
default = "$module:app"

[build-system]
requires = ["hatchling>=1.25"]
build-backend = "hatchling.build"

[tool.hatch.build.targets.wheel]
only-include = ["$module.py"]
$sources
TOML
}
counting() {
  cat <<'PY'
import msgspec

from cozy_runtime.author import App

app = App()


class Words(msgspec.Struct, forbid_unknown_fields=True):
    text: str


class Counted(msgspec.Struct):
    words: int


@app.entrypoint(accelerator=False)
def count(payload: Words) -> Counted:
    return Counted(words=len(payload.text.split()))
PY
}

machine_show_runs() {
step "machine show: a held run's progress (cl-811)"
launch held silent seconds:=60 || return 0
local line=''
for _ in $(seq 30); do
  cozy machine show local > "$OUT/held.machine.txt" 2>&1 || true
  line=$(grep -E "run $n\b" "$OUT/held.machine.txt" | head -1)
  grep -q 'waiting silently' <<< "$line" && break
  sleep 1
done
echo "machine show, run $n: $line"
grep -q 'waiting silently' <<< "$line" || check "machine show: run $n reads '${line:-nothing}', not its progress (waiting silently)"
grep -q 'left behind' <<< "$line" && check "machine show: run $n, in progress, is said left behind: '$line'"
cozy --json run cancel "$n" > /dev/null || check "machine show: run cancel $n failed"
settled held 60
}

# The same POST twice under one key, as the CLI's resend after a dropped answer and the page's outbox send it: the first
# answer twice, and the history stepped once. Then `cozy project undo`'s own write is kept under the CLI's key.
writes_once() {
step "a project write resent under its key is done once (cl-812)"
local P=$OUT/once lock=$HOME/.cozy/calcifer/calcifer.lock.holder addr token id key=resend-$RANDOM$RANDOM first second
cozy project new "$P" > "$OUT/once-new.txt" 2>&1 || { check "once: project new failed: $(tail -c 300 "$OUT/once-new.txt")"; return; }
for c in red green blue; do
  ffmpeg -v error -y -f lavfi -i "color=c=$c:size=64x64" -frames:v 1 "$OUT/once-$c.png" || { check "once: ffmpeg failed"; return; }
  (cd "$P" && cozy --json project import "$OUT/once-$c.png" > "$OUT/once-import-$c.json") || { check "once: import $c failed"; return; }
done
id=$(status "$P" | jq -r '.id // empty')
addr=$(jq -r .address < "$lock") token=$(jq -r .token < "$lock")
[ -n "$id" ] || { check "once: the project has no id"; return; }
for try in first second; do
  curl -sS -D "$OUT/once-$try.headers" -o "$OUT/once-$try.json" -H "Authorization: Bearer $token" -H 'Content-Type: application/json' \
    -H "Idempotency-Key: $key" -X POST -d '{}' "http://$addr/v1/projects/$id/history/undo" || check "once: the $try undo got no answer"
done
first=$(jq -c '[.entries[].id]' "$OUT/once-first.json") second=$(jq -c '[.entries[].id]' "$OUT/once-second.json")
(cd "$P" && cozy --json project history) > "$OUT/once-history.json"
undone=$(jq -c '[.entries[] | select(.undone) | .id]' "$OUT/once-history.json")
echo "once: undo sent twice under $key undid $first, then $second; the history's undone: $undone"
[ "$first" = "$second" ] || check "once: the undo sent again was answered $second, not the first answer $first"
grep -qi '^idempotent-replayed: true' "$OUT/once-second.headers" || check "once: the second undo isn't answered as replayed"
[ "$(jq 'length' <<< "$undone")" = 1 ] || check "once: one undo sent twice undid $undone (cl-812)"
# The CLI's own undo goes under a key of its own: kept, so a resend of it is answered, not done again.
(cd "$P" && cozy --json project undo > "$OUT/once-cli-undo.json") || check "once: cozy project undo failed"
kept=$(python3 -c 'import sqlite3, sys; print(sqlite3.connect(sys.argv[1]).execute("SELECT count(*) FROM answers WHERE key LIKE ?", ("cli-%",)).fetchone()[0])' \
  "$HOME/.cozy/calcifer/state.db" 2> "$OUT/once-answers.err") || kept=0
echo "once: answers kept under the CLI's keys: $kept"
[ "$kept" -ge 1 ] || check "once: cozy project undo's write carried no key Calcifer kept: $(head -c 200 "$OUT/once-answers.err")"
}

# web/resend.js in Chrome: the outbox's tries of one action share its key, and a discard and an undo whose answers are lost land once.
page_resend() {
step "page: a discard and an undo whose answers are lost, each done once (cl-812)"
local chrome pw=$OUT/playwright
chrome=$(web_chrome)
[ -n "$chrome" ] && [ -x "$chrome" ] || { check "page resend: no Google Chrome on this runner (${chrome:-none})"; return; }
[ -d "$pw/node_modules/playwright-core" ] || npm install -s --prefix "$(web_path "$pw")" playwright-core@1.55.0 > "$OUT/web-npm.log" 2>&1 \
  || { check "page resend: npm install playwright-core failed: $(tail -c 300 "$OUT/web-npm.log")"; return; }
web_proof resend "$chrome" "$pw" ''
jq -c '{discard: .seen.discard, undo: .seen.undo}' "$OUT/web-resend.json" 2> /dev/null || true
}

# A locked project whose sources name the `tensorhub` index (a Windows-only dependency, so nothing comes from it here): run once,
# then with hubcheck stopped, so nothing answers at the Hub's address, run again. The lock names its index: no Hub is asked.
hub_away() {
step "locked local code runs while the Hub doesn't answer (cl-809)"
local dir=$OUT/hub-away addr exe=''
counting | package_at "$dir" hub_away '"cozy-runtime>=0.30", "msgspec>=0.19", "minimax-h3-tools ; sys_platform == '"'"'win32'"'"'"' \
  "$(printf '\n[tool.uv.sources]\nminimax-h3-tools = { index = "tensorhub" }')"
cozy --json package lock -p "$dir" --root > "$OUT/hub-away-lock.json" 2>&1 || { check "hub away: package lock failed: $(tail -c 300 "$OUT/hub-away-lock.json")"; return; }
grep -q '/v1/index/_/simple/' "$dir/uv.lock" || { check "hub away: the lock names no tensorhub index"; return; }
cell hub-away-1 "$dir" "text=one two three" || return 0
[ -n "$HUB_CHECK" ] || { check "hub away: no hubcheck to stop"; return; }
addr=$(head -1 "$OUT/hubcheck.addr")
kill "$HUB_CHECK" && wait "$HUB_CHECK" 2> /dev/null || true
curl -sS -m 5 -o /dev/null "http://$addr/" 2> /dev/null && check "hub away: something still answers at $addr"
if cell hub-away-2 "$dir" "text=four five"; then
  grep -q '"words": *2\b' "$OUT/hub-away-2.ndjson" || check "hub away: the reply is not 2 words"
fi
# The Hub back, at the same address, for what follows.
[ "${RUNNER_OS:-}" != Windows ] || exe=.exe
mkdir -p "$OUT/hub-again"
"${RUNNER_TEMP:-$OUT}/hubcheck$exe" --out "$OUT/hub-again" --listen "$addr" > /dev/null 2> "$OUT/hub-again/hubcheck.log" &
HUB_CHECK=$!
for _ in $(seq 30); do curl -sS -m 5 -o /dev/null "http://$addr/openapi.yaml" 2> /dev/null && break; sleep 1; done
}

# Local code whose own dependencies conflict with cozy-runtime's (`msgspec<0.1`), on a machine older than Calcifer: uv says "No
# solution found" and names cozy-runtime, and that is the package's to fix. On `local` one binary is the CLI, Calcifer and TensorD:
# `machine update local --tensord <release>` puts the release before this cozy in all three, then the candidate goes back as the
# installed cozy and `cozy doctor` restarts Calcifer on it, while local's TensorD runs on, older. Nothing may update the machine
# for the conflict (an update run it didn't ask for), and the refusal is the machine's own. Then the machine goes back to the candidate.
machine_age() {
step "a dependency conflict is the package's, not the machine's age (cl-813)"
local older dir=$OUT/conflicting updates running calcifer
[ -f "$CAND_COZY" ] || { echo "machine age: no candidate file to go back to; skipped"; return 0; }
older=$(curl -sS https://api.github.com/repos/cozy-creator/cozy-creator/releases | jq -r '[.[] | select(.tag_name | test("^v[0-9]+[.][0-9]+[.][0-9]+$")) | .tag_name][0] | ltrimstr("v")')
[ -n "$older" ] || { check "machine age: no released cozy to put on the machine"; return; }
cozy --json machine update local --tensord "$older" -y > "$OUT/age-older.json" 2>&1 || { check "machine age: machine update local --tensord $older failed: $(tail -c 300 "$OUT/age-older.json")"; return; }
machine_ready
install_candidate_binary
cozy --json doctor > "$OUT/age-doctor.json" 2>&1 || true
running=$(cozy --json machine show local | jq -r '.status.version // empty')
calcifer=$(jq -r .version "$HOME/.cozy/calcifer/calcifer.lock.holder")
echo "machine age: Calcifer runs cozy $calcifer, local's TensorD $running"
if [ "$running" != "$older" ] || [ "$calcifer" = "$older" ]; then
  check "machine age: no skew to test: Calcifer $calcifer, local $running"
else
  counting | package_at "$dir" conflicting '"cozy-runtime>=0.30", "msgspec<0.1"'
  updates=$(cozy --json run ls --all 2> /dev/null | jq '[.runs[]? | select(.kind == "update")] | length')
  local ALLOW=package_installation_uv_failed
  cell conflicting "$dir" "text=one" || true
  cozy --json run ls --all > "$OUT/age-runs.json" 2> /dev/null || true
  local now
  now=$(jq '[.runs[]? | select(.kind == "update")] | length' "$OUT/age-runs.json")
  [ "$now" = "$updates" ] || check "machine age: Calcifer updated local for a dependency conflict ($updates update runs before, $now after): $(jq -c '[.runs[] | select(.kind == "update")][0] | {number, target, status}' "$OUT/age-runs.json")"
  grep -q machine_too_old "$OUT/conflicting.ndjson" "$OUT/conflicting.err" && check "machine age: a dependency conflict was taken for local's age: $(grep -o 'too old[^"]*' "$OUT/conflicting.ndjson" "$OUT/conflicting.err" | head -1)"
  [ -f "$OUT/conflicting.refused" ] || check "machine age: the install wasn't refused package_installation_uv_failed"
  echo "machine age: on TensorD $running under Calcifer $calcifer: update runs $updates -> $now; said: $(grep -oE '"code": *"[^"]*"|too old[^"]*|updating it to[^"]*' "$OUT/conflicting.ndjson" "$OUT/conflicting.err" | sort -u | head -4 | paste -sd' ')"
fi
cozy --json machine update local --tensord "$CAND_COZY" -y > "$OUT/age-back.json" 2>&1 || fail "machine age: local didn't go back to the candidate: $(tail -c 300 "$OUT/age-back.json")"
machine_ready
}

# The machine's own failure when asked about a run: its journal's run_measurements table away while the run ends, so every
# read of the run's outcome fails there, for past the 120 s Calcifer once settled it FAILED at. Then the table is back.
broken_answers() {
step "a machine that fails when asked about a run for 150 s (cl-810)"
local journal said='' since=0 status
journal=$(find ~/.cozy -name executions.sqlite3 -path '*tensord*' 2> /dev/null | head -1)
[ -n "$journal" ] || { check "broken: no TensorD journal under ~/.cozy"; return; }
journal_table() { python3 -c 'import sqlite3, sys; c = sqlite3.connect(sys.argv[1], timeout=60); c.execute(f"ALTER TABLE {sys.argv[2]} RENAME TO {sys.argv[3]}"); c.commit()' "$journal" "$1" "$2"; }
launch broken silent seconds:=10 || return 0
journal_table run_measurements run_measurements_away || { check "broken: the journal's table couldn't be moved"; return; }
for _ in $(seq 400); do
  cozy --json run show "$n" --once > "$OUT/broken.show.json" 2> /dev/null || true
  status=$(jq -r .status "$OUT/broken.show.json")
  [ "$status" = in_progress ] || break
  if [ "$since" = 0 ] && jq -e '.waiting // "" | test("fails when asked")' "$OUT/broken.show.json" > /dev/null; then
    since=$SECONDS said=$(jq -r .waiting "$OUT/broken.show.json")
  fi
  [ "$since" = 0 ] || [ $((SECONDS - since)) -lt 150 ] || break
  sleep 1
done
journal_table run_measurements_away run_measurements || check "broken: the journal's table couldn't be put back"
echo "broken: run $n $status after $((SECONDS - since)) s of '$said'; now: $(jq -r '.waiting // empty' "$OUT/broken.show.json")"
[ "$since" != 0 ] || check "broken: run $n never said local fails when asked about it ($status)"
[ "$status" = in_progress ] || check "broken: run $n settled $status while its machine failed to answer: $(jq -c .reason "$OUT/broken.show.json")"
settled broken 120
J '.status == "completed"' "$OUT/broken.show.json" || check "broken: run $n is $(jq -c '[.status, .reason]' "$OUT/broken.show.json"), not completed as its machine says"
}
