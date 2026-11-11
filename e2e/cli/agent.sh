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
    project-status) echo 650 ;; project-empty) echo 100 ;; machine-ls) echo 300 ;; run-ls) echo 600 ;; run-ls-empty) echo 100 ;;
    run-show) echo 300 ;; run-show-failed) echo 1500 ;; json-project-status) echo 1300 ;; json-machine-ls) echo 500 ;; json-run-ls) echo 3500 ;;
    json-run-show) echo 700 ;; wait-idle) echo 650 ;; set) echo 100 ;; delete) echo 450 ;; undo) echo 100 ;; status-one) echo 900 ;; json-status-one) echo 2000 ;;
  esac
}

agent() {
step "what an agent reads"
local A=$OUT/agent F=$OUT/agent/film E=$OUT/agent/empty MEASURE=${1:-} n failed line words
rm -rf "$A"; mkdir -p "$A"
: > "$A/bytes.tsv"
# view <label> <dir> <cozy args…>: one view from the working directory <dir>, its stdout kept in <label>.out and
# measured.
view() {
  local label=$1 dir=$2 bytes stream=out; shift 2
  (cd "$dir" && cozy "$@") > "$A/$label.out" 2> "$A/$label.err" || true
  [ "$label" != run-show-failed ] || stream=err
  bytes=$(wc -c < "$A/$label.$stream")
  local most; most=$(budget "$label")
  printf '%s\t%s\t%s\tcozy %s\n' "$label" "$bytes" "$most" "$*" >> "$A/bytes.tsv"
  [ -z "$MEASURE" ] || return 0
  [ -z "$most" ] || [ "$bytes" -le "$most" ] || check "agent: cozy $* printed $bytes bytes, over its $most"
  if [ "$label" = run-show-failed ]; then
    [ ! -s "$A/$label.out" ] && [ -s "$A/$label.err" ] || check "agent: failed run summary must use stderr"
  else
    [ ! -s "$A/$label.err" ] || check "agent: cozy $* wrote to stderr, not in its block: $(head -c 300 "$A/$label.err")"
  fi
}

# A project's boxes are project.db's: what an agent writes goes in by `cozy import` (yaml_in, cl-803).
starter_project "$E" > /dev/null 2>&1 || fail "agent: project new $E"
printf 'version: 4\nassets: {}\n' | yaml_in "$E"
view run-ls-empty "$E" run ls
view project-empty "$E" status
view home-out /

starter_project "$F" > /dev/null 2>&1 || fail "agent: project new $F"
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
(cd "$F" && in_project "$F" render sky sea hill --on local) > "$A/render.txt" 2>&1 || check "agent: render: $(tail -c 300 "$A/render.txt")"
(cd "$F" && in_project "$F" approve sky 1) > /dev/null 2>&1 || check "agent: approve sky 1"
hand "$F" 's/A green hill\./A green hill in the rain./'
(cd "$F" && in_project "$F" --json run "$here/weightless/reverse" "text=agent $RANDOM" --on local) > "$A/reverse.ndjson" 2> "$A/reverse.err" || check "agent: the weightless run failed"
(cd "$F" && in_project "$F" --json run "$here/project/shots.py/crash" "crash=the agent cell fails on purpose" --on local) > "$A/crash.ndjson" 2> "$A/crash.err" || true
n=$(run_number "$A/reverse.ndjson") failed=$(run_number "$A/crash.ndjson")
[ -n "$n" ] && [ -n "$failed" ] || fail "agent: no run numbers: $(tail -c 300 "$A/reverse.err" "$A/crash.err")"

view home "$F"
view status "$F" status
view project-status "$F" status
view machine-ls "$F" machine ls
view run-ls "$F" run ls
view run-show "$F" run watch "$n"
view run-show-failed "$F" run watch "$failed"
view run-show-failed-full "$F" run watch "$failed" --full
view run-show-full "$F" run watch "$n" --full
view json-home "$F" --json
view json-project-status "$F" --json status
view json-machine-ls "$F" --json machine ls
view json-run-ls "$F" --json run ls
view json-run-show "$F" --json run watch "$n"
view json-run-show-full "$F" --json --full run watch "$n"
view json-run-ls-full "$F" --json --full run ls
cat "$A/bytes.tsv"
[ -z "$MEASURE" ] || return 0

# wait: nothing going is the project as it stands; a render asked with --queue is followed to its end; a failure is 1.
local W=$A/waiting code
starter_project "$W" > /dev/null 2>&1 || fail "agent: project new $W"
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
view wait-idle "$F" wait
cmp -s "$A/wait-idle.out" "$A/project-status.out" || check "agent: wait with nothing going isn't what status says: $(diff "$A/wait-idle.out" "$A/project-status.out" | head -4)"
code=0; (cd "$F" && in_project "$F" wait) > /dev/null 2>&1 || code=$?
[ "$code" = 0 ] || check "agent: wait with nothing going and nothing failed exited $code"
(cd "$W" && in_project "$W" render slow_one --on local --queue) > "$A/wait-queue.out" 2>&1 || check "agent: render --queue: $(tail -c 300 "$A/wait-queue.out")"
code=0; (cd "$W" && in_project "$W" wait) > "$A/wait-follow.out" 2>&1 || code=$?
[ "$code" = 0 ] || check "agent: wait on a render going exited $code: $(tail -c 300 "$A/wait-follow.out")"
(cd "$W" && in_project "$W" --json status) | J '.assets[] | select(.name == "slow_one") | .standing == "current"' || check "agent: wait returned before slow_one was made: $(cat "$A/wait-follow.out")"
(cd "$W" && in_project "$W" render broken --on local) > "$A/wait-broken.out" 2>&1 && check "agent: rendering broken exited 0"
code=0; (cd "$W" && in_project "$W" wait) > "$A/wait-failed.out" 2>&1 || code=$?
[ "$code" = 1 ] || check "agent: wait after a failed render exited $code, not 1: $(tail -c 300 "$A/wait-failed.out")"

cmp -s "$A/home.out" "$A/home-out.out" || check "agent: help depends on the working directory"
cmp -s "$A/home.out" "$A/json-home.out" || check "agent: bare --json changes command help"
grep -q 'Commands:' "$A/home.out" || check "agent: bare cozy lists no commands"
! grep -qE '^  project +' "$A/home.out" || check "agent: removed project group is listed"
grep -q 'runs: none in this project' "$A/run-ls-empty.out" || check "agent: run ls of no runs: $(cat "$A/run-ls-empty.out")"
grep -q 'assets: none in project.yaml' "$A/project-empty.out" || check "agent: status of no assets: $(cat "$A/project-empty.out")"
grep -q '4 assets: 1 approved, 1 current, 1 out of date, 1 missing' "$A/status.out" || check "agent: status totals: $(cat "$A/status.out")"
grep -q 'RuntimeError' "$A/run-show-failed-full.out" || check "agent: run watch --full of a failure has no traceback: $(cat "$A/run-show-failed-full.out")"
grep -q "next: cozy run watch $failed --full" "$A/run-show-failed.err" || check "agent: failed run summary has no command for full diagnostics"
[ "$(wc -c < "$A/run-show-full.out")" -gt "$(wc -c < "$A/run-show.out")" ] || check "agent: run watch --full says no more than run watch"
for label in json-project-status json-machine-ls json-run-ls json-run-show json-run-show-full; do
  [ "$(wc -l < "$A/$label.out")" = 1 ] && jq -e . "$A/$label.out" > /dev/null || check "agent: $label is not one line of JSON"
done
J 'has("measurements") or has("calls") or has("report") or has("request") | not' "$A/json-run-show.out" || check "agent: run watch --json holds what --full is for"
J '.request.input.text | startswith("agent ")' "$A/json-run-show-full.out" || check "agent: run watch --json --full has no request"
J --argjson n "$n" '.runs | any(.number == $n) and all(has("result") | not)' "$A/json-run-ls.out" || check "agent: run ls --json: $(head -c 300 "$A/json-run-ls.out")"
J '.runs | any(has("result"))' "$A/json-run-ls-full.out" || check "agent: run ls --json --full has no result"
J '.machines | any(.name == "local") and all(.status.kernels == null)' "$A/json-machine-ls.out" || check "agent: machine ls --json: $(head -c 300 "$A/json-machine-ls.out")"
# Every next: line, run through clap with --help: the verb and its flags exist.
grep -h -oE '^ *next: cozy .*' "$A"/*.out | sed 's/^ *next: //' | sort -u > "$A/next.txt"
[ -s "$A/next.txt" ] || check "agent: no view said what's next"
while IFS= read -r line; do
  read -ra words <<< "$line"
  "$COZY" "${words[@]:1}" --help > /dev/null 2>&1 || check "agent: next: $line does not parse"
done < "$A/next.txt"

# Writes answer on stdout, and one asset's view is bounded: set, delete and the undo line it prints, status <name>.
view set "$F" set sea instructions="A calm sea under <sky>, at dusk."
grep -q '^sea: set instructions' "$A/set.out" || check "agent: set's answer isn't on stdout: $(cat "$A/set.out")"
view status-one "$F" status sky
view json-status-one "$F" --json status sky
J '(.asset.name == "sky") and (.versions | length > 0) and all(.versions[]; has("text") | not)' "$A/json-status-one.out" || check "agent: status sky --json: $(head -c 300 "$A/json-status-one.out")"
view delete "$F" delete later
line=$(grep -oE '^cozy undo [0-9]+$' "$A/delete.out") || check "agent: delete printed no undo line on stdout: $(cat "$A/delete.out")"
read -ra words <<< "$line"
view undo "$F" "${words[@]:1}"
grep -qE '^undid [0-9]+: Deleted later$' "$A/undo.out" || check "agent: undo's answer isn't on stdout: $(cat "$A/undo.out")"
}
