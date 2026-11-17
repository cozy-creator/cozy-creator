# e2e/cli/hubdown.sh: packages without TensorHub (cl-1127). Paul, 11-17: "if tensorhub is down, or unable to be reached, ideally the
# host machine (here) should be able to, on a remote machine, give it a package from our local machine ... we can install packages on
# remote machines without tensorhub being online (if we have the code locally, or the code is available somewhere else, like github)."
# A remote machine here is a stand-in (another user's cozy on this runner, machineset.sh's `stand_in`), added to this computer.
#   hub_down_table   what works with the Hub unreachable, on a machine that exists: measured, one row a line in hubdown.md (dispatch)
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
  local exe='' dir=$OUT/hub-again-${1:-back}
  bash "$here_hubdown/../skew/rehub.sh" > "$OUT/rehub-${1:-back}.log" 2>&1 || check "hub back: the Hub didn't start again: $(tail -c 300 "$OUT/rehub-${1:-back}.log")"
  [ "${RUNNER_OS:-}" != Windows ] || exe=.exe
  mkdir -p "$dir"
  "${RUNNER_TEMP:-$OUT}/hubcheck$exe" --hub "${HUB_CHECK_UPSTREAM:?}" --out "$dir" --listen "$HUB_ADDR" --keep 'POST /v1/run-reports' > /dev/null 2> "$dir/hubcheck.log" &
  HUB_CHECK=$!
  for _ in $(seq 30); do curl -sS -m 5 -o /dev/null "http://$HUB_ADDR/openapi.yaml" 2> /dev/null && break; sleep 1; done
}

# hd_said <file>...: a failure's code and message, in one line.
hd_said() {
  local code message
  code=$(grep -ho '"code": *"[^"]*"' "$@" 2> /dev/null | tail -1 | cut -d'"' -f4)
  message=$(grep -ho '"message": *"[^"]*"' "$@" 2> /dev/null | tail -1 | cut -d'"' -f4)
  [ -n "$code$message" ] || message=$(tail -c 200 "$@" 2> /dev/null | tr '\n|' '  ')
  echo "${code:+$code: }${message:0:220}"
}

# hd_row <what> <name> <cozy args...>: one row of the table, its command bounded at 15 min: works, or fails with what it said.
hd_row() {
  local what=$1 name=$2 code=0 status
  shift 2
  (cd "$RUN_PROJECT" && timeout 900 "$COZY" --json "$@") > "$OUT/hd-$name.ndjson" 2> "$OUT/hd-$name.err" || code=$?
  status=$(grep '^{' "$OUT/hd-$name.ndjson" 2> /dev/null | jq -r '.run.status? // .status? // empty' 2> /dev/null | tail -1)
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
  package_at "$1" "$2" '"cozy-runtime>=0.47.0.dev0,<0.48", "msgspec>=0.19", "minimax-h3-tools>=0.1.0"' \
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
  # lock takes minimax-h3-tools from the root's index); and local code locked against that index.
  counting | package_at "$dir/held" hd_held '"cozy-runtime>=0.30", "msgspec>=0.19"'
  hd_row "(Hub up) local code on $m" up-code run "$dir/held" "text=one two" --on "$m"
  hd_row "(Hub up) minimax-h3-tools on $m" up-release run minimax-h3-tools/echo text=up --on "$m"
  hd_row "(Hub up) e2e-relay on local" up-relay run "$E2E_RELAY/relay" "text=on local" --on local
  hd_relay "$dir/locked" hd_locked
  hd_row "(Hub up) local code locked against the root's index, on local" up-locked run "$dir/locked/relay" "text=locked" --on local
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
