# machine_set (cl-873): a project pinned to a set of machines. Two stand-in machines on this runner, each another user's cozy as
# another computer's would be (release-gate session multi-machine), authorized for this controller and added here. `cozy machine
# use local,stand-a` pins the project: a burst of `cozy run`s naming no machine stays on those two, and stand-b, which automatic
# placement prefers to this computer, gets none; `--on stand-b` overrides the project's set for one run; `--on stand-a,stand-b`
# places among them; the page's Run on pins it the same (web/runon.js). With neither stand-in up, `--on stand-a,stand-b` is
# refused naming both, and nothing runs on another machine.
# stand_as <name> <cozy args...>: the stand-in's own cozy, in its user's default home. From /: the Calcifer it starts detached (no
# user manager here) keeps the folder it started in, where its machine's uv would trip over this user's project .venv.
stand_as() {
  local name=$1
  shift
  (cd / && sudo -u "cozy-$name" -H env ${SSL_CERT_FILE:+SSL_CERT_FILE="$SSL_CERT_FILE"} ${COZY_HUB_URL:+COZY_HUB_URL="$COZY_HUB_URL"} \
    "/opt/cozy-$name/cozy" "$@" < /dev/null 2>> "$OUT/$name.err")
}

# stand_in <name>: the machine of a user of this runner, authorized for this controller and added here as <name>.
stand_in() {
  local name=$1 key reach fp
  id "cozy-$name" > /dev/null 2>&1 || sudo useradd -m "cozy-$name" || { check "$name: no user"; return 1; }
  sudo install -D -m 755 "$COZY" "/opt/cozy-$name/cozy" || { check "$name: cozy not installed for its user"; return 1; }
  stand_as "$name" --json machine show local > "$OUT/$name-local.json" || { check "$name: machine show local: $(tail -c 300 "$OUT/$name.err")"; return 1; }
  fp=$(jq -r '.fingerprint // empty' "$OUT/$name-local.json")
  key=$(jq -r '.controller_key // empty' "$OUT/set-local.json")
  stand_as "$name" --json machine authorize "$key" > "$OUT/$name-authorize.json" \
    || { check "$name: authorize: $(tail -c 300 "$OUT/$name.err")"; return 1; }
  reach=$(jq -r '.machine.reach // empty' "$OUT/$name-authorize.json")
  cozy --json machine add "$name" "$reach" --fingerprint "$fp" > "$OUT/$name-add.json" 2> "$OUT/$name-add.err" \
    || { check "$name: machine add $reach: $(head -c 300 "$OUT/$name-add.json") $(tail -c 300 "$OUT/$name-add.err")"; return 1; }
  local args=()
  [ -z "$CAND_TFS" ] || args+=(--tensorfs "$CAND_TFS")
  [ -z "$CAND_RT" ] || args+=(--runtime "$CAND_RT")
  [ ${#args[@]} -eq 0 ] || cozy --json machine update "$name" "${args[@]}" -y > "$OUT/$name-update.json" 2>&1 \
    || { check "$name: the candidates: $(jq -r 'select(.type == "run.failed") | .run.reason.message' "$OUT/$name-update.json" 2> /dev/null | tail -c 400)"; return 1; }
}

# set_runs <project> <file>: the project's runs of the echo, `number machine status` each.
set_runs() { in_project "$1" --json run ls --limit 0 | jq -r '.runs[] | select(.kind == "call" and (.target | test("/echo"))) | "\(.number) \(.machine) \(.status)"' > "$2"; }

machine_set() {
step "machine set (cl-873): a project pinned to local and stand-a, its runs only there; --on a,b; one run's override; none of them up, refused"
[ "${RUNNER_OS:-}" = Linux ] && [ "${GITHUB_ACTIONS:-}" = true ] \
  || { echo "machine set: its stand-in machines are other users of a fresh GitHub runner"; return; }
local P=$OUT/machine-set echo=minimax-h3-tools/echo before a b pids
cozy --json machine show local > "$OUT/set-local.json" || { check "machine set: machine show local"; return; }
stand_in stand-a & a=$!
stand_in stand-b & b=$!
wait $a; a=$?
wait $b; b=$?
[ "$a" = 0 ] && [ "$b" = 0 ] || return
cozy --json machine ls > "$OUT/set-machines.json"
J '[.machines[] | select(.name == "stand-a" or .name == "stand-b") | select(.state == "ready")] | length == 2' "$OUT/set-machines.json" \
  || { check "machine set: the stand-ins aren't ready: $(jq -c '[.machines[] | {name, state, unreachable}]' "$OUT/set-machines.json")"; return; }
cozy --json new "$P" > "$OUT/set-new.json" 2>&1 || { check "machine set: new: $(tail -c 300 "$OUT/set-new.json")"; return; }

# Pinned, as the CLI and the page both read it.
in_project "$P" --json machine use local,stand-a > "$OUT/set-use.json" 2> "$OUT/set-use.err" \
  && J '.machines == ["local", "stand-a"]' "$OUT/set-use.json" || check "machine use local,stand-a: $(cat "$OUT/set-use.json") $(tail -c 300 "$OUT/set-use.err")"
in_project "$P" --json status | J '.project.machines == ["local", "stand-a"]' || check "status doesn't name the project's machines"
code=0; in_project "$P" --json machine use nowhere > "$OUT/set-unknown.json" 2>&1 || code=$?
[ "$code" != 0 ] && J '.error.code == "machine_not_found"' "$OUT/set-unknown.json" || check "machine use nowhere: exit $code: $(head -c 300 "$OUT/set-unknown.json")"
in_project "$P" --json status | J '.project.machines == ["local", "stand-a"]' || check "a refused machine use changed the project's machines"

# Four at once, naming no machine: each on local or stand-a, none on stand-b. Each waited for by its pid: the suite's shell has others.
pids=()
for i in 1 2 3 4; do (in_project "$P" --json run "$echo" "text=pinned-$i" > "$OUT/set-pinned-$i.ndjson" 2> "$OUT/set-pinned-$i.err") & pids+=($!); done
wait "${pids[@]}"
set_runs "$P" "$OUT/set-pinned.txt"
echo "pinned to local,stand-a: $(tr '\n' ';' < "$OUT/set-pinned.txt")"
[ "$(wc -l < "$OUT/set-pinned.txt")" = 4 ] && ! grep -qv ' \(local\|stand-a\) completed$' "$OUT/set-pinned.txt" \
  || check "pinned runs left local,stand-a or failed: $(tr '\n' ';' < "$OUT/set-pinned.txt") $(tail -c 300 "$OUT/set-pinned-1.err")"

# One run's --on wins over the project's.
in_project "$P" --json run "$echo" text=override --on stand-b > "$OUT/set-override.ndjson" 2> "$OUT/set-override.err" \
  || check "run --on stand-b in a pinned project: $(tail -c 300 "$OUT/set-override.err")"
set_runs "$P" "$OUT/set-override.txt"
grep -q ' stand-b completed$' "$OUT/set-override.txt" || check "run --on stand-b didn't run on stand-b: $(tr '\n' ';' < "$OUT/set-override.txt")"

# --on a,b: among those two.
before=$(wc -l < "$OUT/set-override.txt")
pids=()
for i in 1 2; do (in_project "$P" --json run "$echo" "text=pair-$i" --on stand-a,stand-b > "$OUT/set-pair-$i.ndjson" 2> "$OUT/set-pair-$i.err") & pids+=($!); done
wait "${pids[@]}"
set_runs "$P" "$OUT/set-pair.txt"
head -n $(($(wc -l < "$OUT/set-pair.txt") - before)) "$OUT/set-pair.txt" > "$OUT/set-pair-new.txt"
[ "$(wc -l < "$OUT/set-pair-new.txt")" = 2 ] && ! grep -qv ' stand-[ab] completed$' "$OUT/set-pair-new.txt" \
  || check "run --on stand-a,stand-b: $(tr '\n' ';' < "$OUT/set-pair-new.txt") $(tail -c 300 "$OUT/set-pair-1.err")"
code=0; in_project "$P" --json run "$echo" text=mixed --on stand-a,new > "$OUT/set-mixed.json" 2>&1 || code=$?
[ "$code" != 0 ] && J '.error.code == "on_set"' "$OUT/set-mixed.json" || check "run --on stand-a,new: exit $code: $(head -c 300 "$OUT/set-mixed.json")"

# The page's Run on: the same pin from the top bar, Render all inside it, and the CLI's choice live on the open page.
local pw=$OUT/playwright chrome
chrome=$(web_chrome)
if [ -n "$chrome" ] && [ -x "$chrome" ] && npm install -s --prefix "$(web_path "$pw")" playwright-core@1.55.0 > "$OUT/web-npm.log" 2>&1; then
  step "web-runon: the page's Run on pins a project to local and stand-a; Render all stays inside; the CLI's choice shows live"
  STANDS="stand-a stand-b" web_proof runon "$chrome" "$pw" ''
  jq -c '.seen.placed // empty' "$OUT/web-runon.json" 2> /dev/null || true
else
  check "web-runon: no Chrome or playwright here (${chrome:-none}): $(tail -c 300 "$OUT/web-npm.log" 2> /dev/null)"
fi

# Neither stand-in up: refused, naming both, and run nowhere else.
step "machine set: none of the set up, refused"
sudo pkill -KILL -u cozy-stand-a || true
sudo pkill -KILL -u cozy-stand-b || true
for _ in $(seq 120); do
  cozy --json machine ls > "$OUT/set-down.json"
  J '[.machines[] | select(.name == "stand-a" or .name == "stand-b") | select(.unreachable != null or .state == "ended")] | length == 2' "$OUT/set-down.json" && break
  sleep 1
done
set_runs "$P" "$OUT/set-before-down.txt"
code=0; in_project "$P" --json run "$echo" text=down --on stand-a,stand-b > "$OUT/set-refused.json" 2>&1 || code=$?
[ "$code" != 0 ] && J '.error.code == "machines_unavailable" and (.error.message | test("stand-a") and test("stand-b"))' "$OUT/set-refused.json" \
  || check "run --on stand-a,stand-b with neither up: exit $code: $(head -c 400 "$OUT/set-refused.json")"
set_runs "$P" "$OUT/set-after-down.txt"
cmp -s "$OUT/set-before-down.txt" "$OUT/set-after-down.txt" || check "a run none of its set could take ran: $(head -2 "$OUT/set-after-down.txt" | tr '\n' ';')"

in_project "$P" --json machine use > "$OUT/set-any.json" && J '.machines == []' "$OUT/set-any.json" || check "machine use with none: $(cat "$OUT/set-any.json")"
for name in stand-a stand-b; do timeout 60 "$COZY" --json machine end "$name" --force -y > "$OUT/$name-end.json" 2>&1 || true; done
}
