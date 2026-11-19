# e2e/cli/hubdown.sh: packages without TensorHub (cl-1127). Paul, 11-17: "if tensorhub is down, or unable to be reached, ideally the
# host machine (here) should be able to, on a remote machine, give it a package from our local machine ... we can install packages on
# remote machines without tensorhub being online (if we have the code locally, or the code is available somewhere else, like github)."
# A remote machine here is a stand-in (another user's cozy on this runner, machineset.sh's `stand_in`), added to this computer.
#   hub_down_table   what works with the Hub unreachable, on a machine that exists: measured, one row a line in hubdown.md (dispatch)
#   git_url          Paul: "ideally users should be able to install code from arbitrary public github links": a package installed
#                    from a public repository's URL at a tag, listed with its URL and commit, run, and installed again at another ref;
#                    a package of tools installed so is in the canvas's tool list at its commit
#   hub_down_install with the Hub down, a release this computer holds installed on a stand-in machine from here, and run there;
#                    and local code that calls a Hub package, with no uv.lock, locked, installed, described and run there
here_hubdown=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)

# hub_stop: the isolated Hub's process and hubcheck stopped, so nothing answers at the Hub's address, and its ingress, which machines
# reach it by, answers 502 as a CDN does for a Hub that's down. What it serves itself (the selected Runtime's wheels) stays.
hub_stop() {
  local pid
  HUB_ADDR=$(head -1 "$OUT/hubcheck.addr")
  pid=$(cat "${COZY_TEST_HUB_PID_FILE:?run by e2e/skew/run.sh}")
  kill "$pid" 2> /dev/null || true
  for _ in $(seq 60); do kill -0 "$pid" 2> /dev/null || break; sleep 0.5; done
  [ -z "$HUB_CHECK" ] || { kill "$HUB_CHECK" 2> /dev/null; wait "$HUB_CHECK" 2> /dev/null || true; HUB_CHECK=''; }
  ! curl -sS -m 5 -o /dev/null "http://$HUB_ADDR/openapi.yaml" 2> /dev/null || check "hub down: something still answers at $HUB_ADDR"
  [ "$(curl -s -o /dev/null -w '%{http_code}' "${COZY_TEST_HUB_HEALTH:?}")" != 204 ] || check "hub down: the Hub still answers"
}

# hub_back: the Hub again, and hubcheck at the address cozy knows, for what follows.
hub_back() {
  bash "$here_hubdown/../skew/rehub.sh" > "$OUT/rehub-${1:-back}.log" 2>&1 || check "hub back: the Hub didn't start again: $(tail -c 300 "$OUT/rehub-${1:-back}.log")"
  hub_check_again "$HUB_ADDR"
}

# hd_said <file>...: a failure's code and message, in one line.
hd_said() {
  local code message
  code=$(grep -ho '"code": *"[^"]*"' "$@" 2> /dev/null | tail -1 | cut -d'"' -f4) || code=
  message=$(grep -ho '"message": *"[^"]*"' "$@" 2> /dev/null | tail -1 | cut -d'"' -f4) || message=
  [ -n "$code$message" ] || message=$(tail -c 200 "$@" 2> /dev/null | tr '\n|' '  ') || message=
  echo "${code:+$code: }${message:0:220}"
}

# hd_row <what> <name> <cozy args...>: one row of the table, its command bounded at 15 min: works, or fails with what it said.
hd_row() {
  local what=$1 name=$2 code=0 status
  shift 2
  (cd "$RUN_PROJECT" && timeout 900 "$COZY" --json "$@") > "$OUT/hd-$name.ndjson" 2> "$OUT/hd-$name.err" || code=$?
  status=$(grep '^{' "$OUT/hd-$name.ndjson" 2> /dev/null | jq -r '.run.status? // .status? // empty' 2> /dev/null | tail -1) || status=
  if [ "$code" = 0 ] && [ "${status:-completed}" = completed ]; then
    printf '| %s | works | |\n' "$what" >> "$OUT/hubdown.md"
    echo "hub down: $what: works"
  else
    printf '| %s | fails (exit %s) | %s |\n' "$what" "$code" "$(hd_said "$OUT/hd-$name.ndjson" "$OUT/hd-$name.err")" >> "$OUT/hubdown.md"
    echo "hub down: $what: fails (exit $code): $(hd_said "$OUT/hd-$name.ndjson" "$OUT/hd-$name.err")"
  fi
}

# hd_relay <dir> <name>: local code calling minimax-h3-tools's echo, its source the `tensorhub` index, with no uv.lock (hub_dependency).
hd_relay() {
  rm -rf "$1"
  package_at "$1" "$2" '"cozy-runtime>=0.49.0.dev0,<0.50", "msgspec>=0.19", "minimax-h3-tools>=0.1.0"' \
    "$(printf '\n[tool.uv.sources]\nminimax-h3-tools = { index = "tensorhub" }')" <<PY
import msgspec
from ${2}_calls import Text, echo

from cozy_runtime.author import App, call

app = App()


class Relay(msgspec.Struct, forbid_unknown_fields=True):
    text: str


class Relayed(msgspec.Struct):
    text: str
    release: str


async def relay(payload: Relay) -> Relayed:
    echoed = await call(echo, Text(text=payload.text))
    return Relayed(text=echoed.text, release=echoed.release)


app.entrypoint(relay, accelerator=False)
PY
  cp "$here_hubdown/relay/e2e_relay/echo_calls.py" "$1/${2}_calls.py"
  sed -i "s/only-include = \[\"$2.py\"\]/only-include = [\"$2.py\", \"${2}_calls.py\"]/" "$1/pyproject.toml"
}

# hub_down_table (by dispatch, shard hubdown): a stand-in machine made ready with the Hub up, then each of the issue's questions asked
# of it with the Hub down, through cozy. Measured, not judged: what the Hub's absence does today, before and after cl-1127.
hub_down_table() {
  step "what works with the Hub unreachable, on a machine that exists (cl-1127)"
  [ "${RUNNER_OS:-}" = Linux ] && [ "${GITHUB_ACTIONS:-}" = true ] || { echo "hub down: its stand-in is another user of a fresh GitHub runner"; return 0; }
  [ -n "${E2E_RELAY:-}" ] || { check "hub down: isolated.sh published no e2e-relay: ${E2E_RELAY_REFUSED:-not seeded}"; return; }
  local m=stand-h dir=$OUT/hubdown
  mkdir -p "$dir"
  printf '| with the Hub unreachable | | said |\n|---|---|---|\n' > "$OUT/hubdown.md"
  cozy --json machine show local > "$OUT/set-local.json" || { check "hub down: machine show local"; return; }
  stand_in "$m" || return
  # With the Hub up: local code on the stand-in; a release on it (minimax-h3-tools); a release on this computer alone (e2e-relay, whose
  # lock takes minimax-h3-tools from the root's index); local code locked against that index; and the stand-in's update, whose target
  # this computer then knows.
  counting | package_at "$dir/held" hd_held '"cozy-runtime>=0.30", "msgspec>=0.19"'
  hd_row "(Hub up) local code on $m" up-code run "$dir/held" "text=one two" --on "$m"
  hd_row "(Hub up) minimax-h3-tools on $m" up-release run minimax-h3-tools/echo text=up --on "$m"
  hd_row "(Hub up) e2e-relay on local" up-relay run "$E2E_RELAY/relay" "text=on local" --on local
  hd_relay "$dir/locked" hd_locked
  hd_row "(Hub up) local code locked against the root's index, on local" up-locked run "$dir/locked/relay" "text=locked" --on local
  hd_row "(Hub up) cozy machine update $m" up-update machine update "$m" -y
  hub_stop
  counting | sed 's/len(payload.text.split())/len(payload.text.split()) * 1/' | package_at "$dir/new" hd_new '"cozy-runtime>=0.30", "msgspec>=0.19"'
  hd_row "Local code new to $m (cozy run ./dir --on $m)" code-new run "$dir/new" "text=a b c" --on "$m"
  hd_row "Local code $m holds" code-held run "$dir/held" "text=a b" --on "$m"
  hd_row "A Hub release $m holds (cozy run minimax-h3-tools/echo --on $m)" release-held run minimax-h3-tools/echo text=down --on "$m"
  hd_row "A Hub release $m holds, pinned (minimax-h3-tools/echo@0.1.0)" release-pinned run minimax-h3-tools/echo@0.1.0 text=pinned --on "$m"
  hd_row "Install a Hub release this computer holds (cozy package install e2e-relay --on $m)" install-held package install "$E2E_RELAY" --on "$m"
  hd_row "Run it there (cozy run e2e-relay/relay --on $m)" run-held run "$E2E_RELAY/relay" "text=there" --on "$m"
  hd_relay "$dir/relock" hd_relock
  hd_row "Lock local code that depends on a Hub package (cozy run ./dir/relay --on $m)" lock-hub run "$dir/relock/relay" "text=relocked" --on "$m"
  hd_row "cozy machine update $m" update machine update "$m" -y
  hub_back hubdown
  timeout 120 "$COZY" --json machine end "$m" --force -y > "$OUT/$m-end.json" 2>&1 || true
  echo "hub down, measured:"
  cat "$OUT/hubdown.md"
  [ -z "${GITHUB_STEP_SUMMARY:-}" ] || { printf '\n### Hub down (cl-1127)\n\n'; cat "$OUT/hubdown.md"; } >> "$GITHUB_STEP_SUMMARY"
}

# git_url (cl-1127; Paul, 11-17: "ideally users should be able to install code from arbitrary public github links, for example"): the
# hub suite's echo package from cozy-creator's own public repository, by its URL at a tag, installed on local; `cozy package ls` names
# it by URL and commit; it runs by the same URL; installed again at the repository's commit of this run, the listing moves to it. The
# CLI before it refused any URL as a package reference.
git_url() {
  step "a package installed and run from a public repository's URL (cl-1127)"
  [ -n "$LOCAL" ] || return 0
  local repo=https://github.com/cozy-creator/cozy-creator folder=release-gate/suites/hub/pkg tagged now said
  tagged=$(git ls-remote "$repo" 'v0.25.0^{}' | cut -f1) || tagged=
  [ -n "$tagged" ] || { check "git url: $repo has no tag v0.25.0 to install from"; return; }
  cozy --json package install "$repo@v0.25.0/$folder" --on local > "$OUT/git-install.ndjson" 2> "$OUT/git-install.err" \
    || { check "git url: cozy package install $repo@v0.25.0/$folder failed: $(hd_said "$OUT/git-install.ndjson" "$OUT/git-install.err")"; return; }
  cozy --json package ls --on local > "$OUT/git-installed.json" 2>&1
  said=$(jq -r '[.packages[] | select(.package == "local/ci-hub-pkg") | .source | select(startswith("git+"))] | unique | join(" ")' "$OUT/git-installed.json") || said=
  echo "git url: local/ci-hub-pkg on local from ${said:-nothing}"
  [ "$said" = "git+$repo@$tagged#subdirectory=$folder" ] || check "git url: local/ci-hub-pkg is listed from '${said:-nothing}', not git+$repo@$tagged#subdirectory=$folder"
  if cell git-run "$repo@v0.25.0/$folder/echo" "text=from github"; then
    grep -q '"text": *"from github"' "$OUT/git-run.ndjson" && grep -q '"release": *"0.1.0"' "$OUT/git-run.ndjson" \
      || check "git url: the reply isn't the repository's echo: $(tail -c 300 "$OUT/git-run.ndjson")"
  fi
  # Installed again at another ref, it is that commit's.
  now=$(git -C "$here_hubdown" rev-parse HEAD 2> /dev/null) || now=
  [ -n "$now" ] && [ "$now" != "$tagged" ] || { echo "git url: no other commit to move to"; return 0; }
  cozy --json package install "$repo/tree/$now/$folder" --on local > "$OUT/git-again.ndjson" 2> "$OUT/git-again.err" \
    || { check "git url: installing it again at $now failed: $(hd_said "$OUT/git-again.ndjson" "$OUT/git-again.err")"; return; }
  cozy --json package ls --on local > "$OUT/git-moved.json" 2>&1
  said=$(jq -r '[.packages[] | select(.package == "local/ci-hub-pkg") | .source | select(startswith("git+"))] | unique | join(" ")' "$OUT/git-moved.json") || said=
  [ "$said" = "git+$repo@$now#subdirectory=$folder" ] || check "git url: installed again at $now, it's listed from '${said:-nothing}'"
  # A repository's tools are in the canvas's list (`GET /v1/tools`) once installed, named at their commit and said from it.
  local boxed=e2e/cli/project/boxed holder=$HOME/.cozy/calcifer/calcifer.lock.holder id
  cozy --json package install "$repo/tree/$now/$boxed" --on local > "$OUT/git-tools.ndjson" 2> "$OUT/git-tools.err" \
    || { check "git url: installing $boxed at $now failed: $(hd_said "$OUT/git-tools.ndjson" "$OUT/git-tools.err")"; return; }
  id="$repo@$now/$boxed/plain"
  for _ in $(seq 12); do
    curl --fail-with-body -sS -H "Authorization: Bearer $(jq -r .token "$holder")" "http://$(jq -r .address "$holder")/v1/tools" > "$OUT/git-tools.json" 2>&1 \
      && J --arg id "$id" '[.tools[] | select(.id == $id)] != []' "$OUT/git-tools.json" && break
    sleep 5
  done
  said=$(jq -r --arg id "$id" '[.tools[]? | select(.id == $id) | .source] | join(" ")' "$OUT/git-tools.json" 2> /dev/null) || said=
  echo "git url: the tool list has $id from ${said:-nothing}"
  [ "$said" = "git+$repo@$now#subdirectory=$boxed" ] \
    || check "git url: the tool list doesn't list $id from git+$repo@$now#subdirectory=$boxed: '${said:-nothing}' $(jq -c '[.tools[]? | select(.source // "" | startswith("git+")) | .id]' "$OUT/git-tools.json" 2> /dev/null | head -c 300)"
}

# held_wheel_describe: whether the Runtime under test reads a pinned callee wheel the machine holds in place of its URL (cozy-runtime
# #2229, cl-1127), by its source: the candidate wheel, else the one local runs. 0.26.0's Runtime (ca841fe72) doesn't: local code that
# calls a Hub package describes only with the Hub up there. One it can't read is judged.
HELD_PROBED=''
held_wheel_describe() {
  local wheel=${CAND_RT:-}
  [ -f "$wheel" ] || wheel=$(ls "$HOME"/.cozy/machine/root/var/lib/cozy/tensord/sdk/current/cozy_runtime-*.whl 2> /dev/null | head -1) || wheel=
  HELD_PROBED=${wheel##*/}
  [ -f "$wheel" ] || return 0
  python3 -I - "$wheel" <<'PY'
import sys, zipfile
try:
    source = zipfile.ZipFile(sys.argv[1]).read("cozy_runtime/internal/locked_sources.py").decode()
except (KeyError, OSError, zipfile.BadZipFile):
    sys.exit(0)
sys.exit(0 if "self.held" in source else 1)
PY
}

# hub_down_install (cl-1127; Paul, 11-17: "if tensorhub is down, or unable to be reached, ideally the host machine (here) should be able
# to, on a remote machine, give it a package from our local machine"): e2e-relay, an account's release whose lock takes minimax-h3-tools
# from the root's index, installed on local with the Hub up, so this computer holds it; then with the Hub down, installed on a stand-in
# machine that never had it, and run there: its card and wheels go from this computer by content. Before, the stand-in's install read
# the release's install plan at the Hub and failed (run 44049460019). Then local code calling minimax-h3-tools, with no uv.lock (the
# coordinator, 11-17: "local code that depends on a Hub package locks, installs, describes and runs with the Hub down"): locked from
# what this computer holds, the callee's wheel carried, and described from the machine's copy of it. Before, its describer fetched
# the pinned wheel at the Hub (`static_module`, run 44067585439's table).
hub_down_install() {
  step "with the Hub down, a release this computer holds installed on another machine and run there (cl-1127)"
  [ "${RUNNER_OS:-}" = Linux ] && [ "${GITHUB_ACTIONS:-}" = true ] || { echo "hub down install: its stand-in is another user of a fresh GitHub runner"; return 0; }
  [ -n "${E2E_RELAY:-}" ] || { check "hub down install: isolated.sh published no e2e-relay: ${E2E_RELAY_REFUSED:-not seeded}"; return; }
  local m=stand-k held
  # Why a release isn't held here is a warning, in Calcifer's diagnostics log (tests only).
  calcifer_diagnostics
  cozy --json package install "$E2E_RELAY" --on local > "$OUT/hdi-local.ndjson" 2> "$OUT/hdi-local.err" \
    || { check "hub down install: installing $E2E_RELAY on local failed: $(hd_said "$OUT/hdi-local.ndjson" "$OUT/hdi-local.err")"; return; }
  # It is held here behind that install.
  for _ in $(seq 120); do
    held=$(find "$HOME/.cozy/calcifer/held" -path "*/${E2E_RELAY}/0.1.0.json" 2> /dev/null | head -1) || held=
    [ -n "$held" ] && break
    sleep 1
  done
  [ -n "$held" ] || check "hub down install: this computer didn't hold $E2E_RELAY 0.1.0 two minutes after its install: $(grep -a 'held here' "$HOME/.cozy/calcifer/diagnostics.log" 2> /dev/null | tail -2)"
  # Local code calling the same root package, locked and run here with the Hub up: what uv holds for a lock with it down.
  hd_relay "$OUT/hdi-warm" hdi_warm
  cell hdi-warm "$OUT/hdi-warm/relay" "text=warm" || true
  cozy --json machine show local > "$OUT/set-local.json" || { check "hub down install: machine show local"; return; }
  stand_in "$m" || return
  hub_stop
  cozy --json package install "$E2E_RELAY" --on "$m" > "$OUT/hdi-install.ndjson" 2> "$OUT/hdi-install.err" \
    || check "hub down install: with the Hub down, installing $E2E_RELAY on $m failed: $(hd_said "$OUT/hdi-install.ndjson" "$OUT/hdi-install.err")"
  (cd "$RUN_PROJECT" && timeout 900 "$COZY" --json run "$E2E_RELAY/relay" "text=hub down" --on "$m") > "$OUT/hdi-run.ndjson" 2> "$OUT/hdi-run.err" \
    && grep -q '"text": *"hub down"' "$OUT/hdi-run.ndjson" && grep -q '"release": *"0.1.0"' "$OUT/hdi-run.ndjson" \
    || check "hub down install: with the Hub down, $E2E_RELAY/relay on $m didn't answer minimax-h3-tools 0.1.0's echo: $(hd_said "$OUT/hdi-run.ndjson" "$OUT/hdi-run.err")"
  if held_wheel_describe; then
    hd_relay "$OUT/hdi-code" hdi_code
    (cd "$RUN_PROJECT" && timeout 900 "$COZY" --json run "$OUT/hdi-code/relay" "text=hub down code" --on "$m") > "$OUT/hdi-code.ndjson" 2> "$OUT/hdi-code.err" \
      && grep -q '"text": *"hub down code"' "$OUT/hdi-code.ndjson" && grep -q '"release": *"0.1.0"' "$OUT/hdi-code.ndjson" \
      || check "hub down install: with the Hub down, local code calling minimax-h3-tools didn't lock, install, describe and run on $m: $(hd_said "$OUT/hdi-code.ndjson" "$OUT/hdi-code.err")"
    grep -q 'registry = "http[^"]*/v1/index/_/simple/"' "$OUT/hdi-code/uv.lock" 2> /dev/null \
      || check "hub down install: the lock made with the Hub down doesn't name the root's index as the Hub's would: $(grep -m1 'registry' "$OUT/hdi-code/uv.lock" 2> /dev/null)"
  else
    echo "hub down install: local code calling a Hub package: not applicable, the Runtime under test ($HELD_PROBED) lacks cozy-runtime #2229's held-wheel describe"
  fi
  hub_back hdi
  timeout 120 "$COZY" --json machine end "$m" --force -y > "$OUT/$m-end.json" 2>&1 || true
}
