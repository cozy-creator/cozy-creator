# e2e/cli/agent.sh: what an agent reads (tracker/design/cli-for-agents.md, step 1). Off a terminal each view is one block on
# stdout, under its byte budget; an empty one says so; `--json` is one line holding the view's facts, `--full` the whole
# record; every `next:` a view prints parses; and `project wait` exits as `--help` says (a stall's 4 is the gate's: it takes
# Calcifer's 10 minutes). Measured on a small project of weightless pictures.
#   agent            judged, as the cli suite runs it
#   agent measure    each view's bytes into $OUT/agent/bytes.tsv, nothing judged: what a cozy before this printed
here=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
J() { jq -e "$@" > /dev/null; }

# Bytes each view may print on this fixture (about 3 to a token).
budget() {
  case $1 in
    home) echo 650 ;; home-out) echo 550 ;; project-status) echo 650 ;; project-empty) echo 100 ;; machine-ls) echo 300 ;; run-ls) echo 600 ;; run-ls-empty) echo 100 ;;
    run-show) echo 300 ;; run-show-failed) echo 1500 ;; json-home) echo 2000 ;; json-project-status) echo 1300 ;; json-machine-ls) echo 500 ;; json-run-ls) echo 3500 ;;
    json-run-show) echo 700 ;; wait-idle) echo 650 ;; set) echo 100 ;; delete) echo 450 ;; restore) echo 100 ;; status-one) echo 900 ;; json-status-one) echo 2000 ;;
  esac
}

agent() {
step "what an agent reads"
local A=$OUT/agent F=$OUT/agent/film E=$OUT/agent/empty MEASURE=${1:-} n failed line words
rm -rf "$A"; mkdir -p "$A"
: > "$A/bytes.tsv"
# view <label> <dir> <cozy args…>: one view of the project at <dir> (COZY_PROJECT; none for /), its stdout kept in <label>.out and
# measured.
view() {
  local label=$1 dir=$2 bytes named=''; shift 2
  [ "$dir" = / ] || named=$dir
  (cd "$dir" && COZY_PROJECT=$named cozy "$@") > "$A/$label.out" 2> "$A/$label.err" || true
  bytes=$(wc -c < "$A/$label.out")
  local most; most=$(budget "$label")
  printf '%s\t%s\t%s\tcozy %s\n' "$label" "$bytes" "$most" "$*" >> "$A/bytes.tsv"
  [ -z "$MEASURE" ] || return 0
  [ -z "$most" ] || [ "$bytes" -le "$most" ] || check "agent: cozy $* printed $bytes bytes, over its $most"
  [ ! -s "$A/$label.err" ] || check "agent: cozy $* wrote to stderr, not in its block: $(head -c 300 "$A/$label.err")"
}

# A project's boxes are project.db's: what an agent writes goes in by `cozy project import` (yaml_in, cl-803).
cozy project new "$E" > /dev/null 2>&1 || fail "agent: project new $E"
printf 'version: 4\nassets: {}\n' | yaml_in "$E"
view run-ls-empty "$E" run ls
view project-empty "$E" project status
view home-out / status

cozy project new "$F" > /dev/null 2>&1 || fail "agent: project new $F"
yaml_in "$F" <<YAML
version: 4
assets:
  sky:
    tool: $here/project/draw.py/draw
    instructions: A clear sky at dawn over the agent cell, $RANDOM.
  sea:
    tool: $here/project/draw.py/draw
    instructions: A calm sea under <sky>.
  hill:
    tool: $here/project/draw.py/draw
    instructions: A green hill.
  later:
    tool: $here/project/draw.py/draw
    instructions: Not rendered yet.
YAML
(cd "$F" && cozy project -p "$F" render sky sea hill --on local) > "$A/render.txt" 2>&1 || check "agent: render: $(tail -c 300 "$A/render.txt")"
(cd "$F" && cozy project -p "$F" approve sky 1) > /dev/null 2>&1 || check "agent: approve sky 1"
hand "$F" 's/A green hill\./A green hill in the rain./'
(cd "$F" && cozy --json run -p "$F" "$here/weightless/reverse" "text=agent $RANDOM" --on local) > "$A/reverse.ndjson" 2> "$A/reverse.err" || check "agent: the weightless run failed"
(cd "$F" && cozy --json run -p "$F" "$here/project/shots.py/crash" "crash=the agent cell fails on purpose" --on local) > "$A/crash.ndjson" 2> "$A/crash.err" || true
n=$(run_number "$A/reverse.ndjson") failed=$(run_number "$A/crash.ndjson")
[ -n "$n" ] && [ -n "$failed" ] || fail "agent: no run numbers: $(tail -c 300 "$A/reverse.err" "$A/crash.err")"

view home "$F"
view status "$F" status
view project-status "$F" project status
view machine-ls "$F" machine ls
view run-ls "$F" run ls
view run-show "$F" run show "$n"
view run-show-failed "$F" run show "$failed"
view run-show-full "$F" run show "$n" --full
view json-home "$F" --json
view json-project-status "$F" --json project status
view json-machine-ls "$F" --json machine ls
view json-run-ls "$F" --json run ls
view json-run-show "$F" --json run show "$n"
view json-run-show-full "$F" --json --full run show "$n"
view json-run-ls-full "$F" --json --full run ls
cat "$A/bytes.tsv"
[ -z "$MEASURE" ] || return 0

# wait: nothing going is the project as it stands; a render asked with --queue is followed to its end; a failure is 1.
local W=$A/waiting code
cozy project new "$W" > /dev/null 2>&1 || fail "agent: project new $W"
yaml_in "$W" <<YAML
version: 4
assets:
  slow_one:
    tool: $here/project/tool.py/slow
    prompt: A still that takes four seconds, $RANDOM.
    seconds: 4
  broken:
    tool: $here/project/shots.py/crash
    crash: the agent cell fails on purpose
YAML
view wait-idle "$F" project wait
cmp -s "$A/wait-idle.out" "$A/project-status.out" || check "agent: wait with nothing going isn't what status says: $(diff "$A/wait-idle.out" "$A/project-status.out" | head -4)"
code=0; (cd "$F" && cozy project -p "$F" wait) > /dev/null 2>&1 || code=$?
[ "$code" = 0 ] || check "agent: project wait with nothing going and nothing failed exited $code"
(cd "$W" && cozy project -p "$W" render slow_one --on local --queue) > "$A/wait-queue.out" 2>&1 || check "agent: render --queue: $(tail -c 300 "$A/wait-queue.out")"
code=0; (cd "$W" && cozy project -p "$W" wait) > "$A/wait-follow.out" 2>&1 || code=$?
[ "$code" = 0 ] || check "agent: project wait on a render going exited $code: $(tail -c 300 "$A/wait-follow.out")"
(cd "$W" && cozy --json project -p "$W" status) | J '.assets[] | select(.name == "slow_one") | .standing == "current"' || check "agent: wait returned before slow_one was made: $(cat "$A/wait-follow.out")"
(cd "$W" && cozy project -p "$W" render broken --on local) > "$A/wait-broken.out" 2>&1 && check "agent: rendering broken exited 0"
code=0; (cd "$W" && cozy project -p "$W" wait) > "$A/wait-failed.out" 2>&1 || code=$?
[ "$code" = 1 ] || check "agent: project wait after a failed render exited $code, not 1: $(tail -c 300 "$A/wait-failed.out")"

# Calcifer's age moves between the two.
diff <(grep -v '^calcifer:' "$A/home.out") <(grep -v '^calcifer:' "$A/status.out") > "$A/home-status.diff" || check "agent: cozy and cozy status differ: $(head -5 "$A/home-status.diff")"
grep -q 'runs: none in this project' "$A/run-ls-empty.out" || check "agent: run ls of no runs: $(cat "$A/run-ls-empty.out")"
grep -q 'assets: none in project.yaml' "$A/project-empty.out" || check "agent: status of no assets: $(cat "$A/project-empty.out")"
grep -q 'project: none named' "$A/home-out.out" || check "agent: cozy naming no project: $(cat "$A/home-out.out")"
grep -q '4 assets: 1 approved, 1 current, 1 out of date, 1 missing' "$A/home.out" || check "agent: cozy's totals: $(grep project: "$A/home.out")"
grep -q 'RuntimeError' "$A/run-show-failed.out" || check "agent: run show of a failure has no traceback: $(cat "$A/run-show-failed.out")"
[ "$(wc -c < "$A/run-show-full.out")" -gt "$(wc -c < "$A/run-show.out")" ] || check "agent: run show --full says no more than run show"
for label in json-home json-project-status json-machine-ls json-run-ls json-run-show json-run-show-full; do
  [ "$(wc -l < "$A/$label.out")" = 1 ] && jq -e . "$A/$label.out" > /dev/null || check "agent: $label is not one line of JSON"
done
J 'has("measurements") or has("calls") or has("report") or has("request") | not' "$A/json-run-show.out" || check "agent: run show --json holds what --full is for"
J '.request.input.text | startswith("agent ")' "$A/json-run-show-full.out" || check "agent: run show --json --full has no request"
J --argjson n "$n" '.runs | any(.number == $n) and all(has("result") | not)' "$A/json-run-ls.out" || check "agent: run ls --json: $(head -c 300 "$A/json-run-ls.out")"
J '.runs | any(has("result"))' "$A/json-run-ls-full.out" || check "agent: run ls --json --full has no result"
J '.machines | any(.name == "local") and all(.status.kernels == null)' "$A/json-machine-ls.out" || check "agent: machine ls --json: $(head -c 300 "$A/json-machine-ls.out")"
J --arg p "$F" '.project.path == $p and .calcifer.running and (.machines | length > 0)' "$A/json-home.out" || check "agent: cozy --json: $(head -c 300 "$A/json-home.out")"
# Every next: line, run through clap with --help: the verb and its flags exist.
grep -h -oE '^ *next: cozy .*' "$A"/*.out | sed 's/^ *next: //' | sort -u > "$A/next.txt"
[ -s "$A/next.txt" ] || check "agent: no view said what's next"
while IFS= read -r line; do
  read -ra words <<< "$line"
  "$COZY" "${words[@]:1}" --help > /dev/null 2>&1 || check "agent: next: $line does not parse"
done < "$A/next.txt"

# Writes answer on stdout, and one asset's view is bounded: set, delete and the restore line it prints, status <name>.
view set "$F" project set sea instructions="A calm sea under <sky>, at dusk."
grep -q '^sea: set instructions' "$A/set.out" || check "agent: set's answer isn't on stdout: $(cat "$A/set.out")"
view status-one "$F" project status sky
view json-status-one "$F" --json project status sky
J '(.asset.name == "sky") and (.versions | length > 0) and all(.versions[]; has("text") | not)' "$A/json-status-one.out" || check "agent: status sky --json: $(head -c 300 "$A/json-status-one.out")"
view delete "$F" project delete later
line=$(grep -oE '^cozy project restore later [A-Za-z0-9_-]+$' "$A/delete.out") || check "agent: delete printed no restore line on stdout: $(cat "$A/delete.out")"
read -ra words <<< "$line"
view restore "$F" "${words[@]:1}"
grep -q '^later is back' "$A/restore.out" || check "agent: restore's answer isn't on stdout: $(cat "$A/restore.out")"
}
