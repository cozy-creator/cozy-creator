# from_source (xs-195, Paul 11-13: nothing untested goes to the Hub, so RC testing runs packages from source). A dev home
# (`dev = true`, an RC build) names a packages checkout in its config (`[calcifer] packages`): a first-party package it holds runs
# from its source wherever its unversioned name is asked, here the isolated Hub's root minimax-h3-tools, whose echo says
# "source" instead of its release's 0.1.0. On local and on a stand-in (another user's cozy on this runner: this Hub's docker
# rentals never get ready), each run's record says the checkout's branch and commit. A package of the checkout that takes
# minimax-h3-tools from the Hub's index (as orbit takes minimax-h3) calls the checkout's, as cargo's [patch] does. A release
# named (`@0.1.0`) and an account's package come from the Hub, and the page's tools list the checkout's from source (`source_reads`).
# `local` follows the RC (held here by hand, so the shard keeps its candidates). With `dev = false` the same name is the Hub's again.
# said_by <name>: what run <name>'s record says of it, for the log: its number, machine, target, reply's release and its from-source line.
said_by() {
  echo "from source: $1: run $(cat "$OUT/$1.number" 2> /dev/null) on $(run_get "$OUT" "$1" .machine): $(run_get "$OUT" "$1" .target)," \
    "release $(grep -o '"release": *"[^"]*"' "$OUT/$1.ndjson" | tail -1 | cut -d'"' -f4)," \
    "$(run_get "$OUT" "$1" '[.warnings[]? | select(.code == "from_source") | .message] | first' || true)"
}

from_source() {
step "a dev home's packages checkout: first-party packages run from source, on local and a stand-in (xs-195)"
[ -n "$LOCAL" ] || return 0
[ "${RUNNER_OS:-}" = Linux ] && [ "${GITHUB_ACTIONS:-}" = true ] || { echo "from source: its stand-in is another user of a fresh GitHub runner"; return; }
case $("$COZY" --version) in *-dev+g*) ;; *) echo "from source: $("$COZY" --version) is no RC build, which reads no checkout"; return ;; esac
local co=$OUT/packages-rc cfg=$HOME/.cozy/config.toml holder=$HOME/.cozy/calcifer/calcifer.lock.holder sha short said callee lock args=()
rm -rf "$co" && mkdir -p "$co"
# minimax-h3-tools as isolated.sh publishes it at the root, its echo saying it's the source.
cp -r "$here/../../release-gate/suites/hub/pkg" "$co/minimax-h3-tools" && mv "$co/minimax-h3-tools/ci_hub_pkg" "$co/minimax-h3-tools/minimax_h3_tools"
sed -i -e 's/^name = "ci-hub-pkg"/name = "minimax-h3-tools"/' -e 's/ci_hub_pkg/minimax_h3_tools/g' "$co/minimax-h3-tools/pyproject.toml" "$co/minimax-h3-tools/package.toml"
sed -i 's/^RELEASE = "0.1.0"$/RELEASE = "source"/' "$co/minimax-h3-tools/minimax_h3_tools/__init__.py"
package_at "$co/e2e-source-relay" e2e_source_relay '"cozy-runtime>=0.48.0.dev0,<0.49", "msgspec>=0.19", "minimax-h3-tools>=0.1.0"' \
  "$(printf '\n[tool.uv.sources]\nminimax-h3-tools = { index = "tensorhub" }')" <<'PY'
import msgspec
from e2e_source_relay_calls import Text, echo

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
# The same generated seed interface; source overrides change RELEASE, not its declaration.
cp "$here/relay/e2e_relay/echo_calls.py" "$co/e2e-source-relay/e2e_source_relay_calls.py"
sed -i 's/only-include = \["e2e_source_relay.py"\]/only-include = ["e2e_source_relay.py", "e2e_source_relay_calls.py"]/' "$co/e2e-source-relay/pyproject.toml"
# Tools a box is made of, for the page's list.
cp -r "$here/project/boxed" "$co/e2e-boxed"
# The real Orbit and the H3 it calls (xs-198): after the hard cut Orbit needs the checkout's H3, never the Hub's.
if [ -n "$PACKAGES" ]; then
  cp -r "$PACKAGES/orbit" "$PACKAGES/minimax-h3" "$co/" || { check "from source: $PACKAGES holds no orbit and minimax-h3"; return; }
  # It reads with the run's candidate Runtime, as the machine runs it (cl-994): until a set lands, the RC's newest build is another.
  # Beside it the candidate TensorFS, which that Runtime floors on (xs-216: `tensorfs>=0.27.0.dev<n>`, which no index holds), for a
  # rental's platform as a lock takes it (scripts/lock.sh); uv reads a source only for a dependency the project names.
  [ ! -f "${CAND_RT:-}" ] || sed -i "/^\[tool.uv.sources\]/a cozy-runtime = { path = \"$CAND_RT\" }" "$co/orbit/pyproject.toml"
  if [ -f "${CAND_RT:-}" ] && [ -f "${CAND_TFS:-}" ]; then
    sed -i "/^\[tool.uv.sources\]/a tensorfs = { path = \"$CAND_TFS\", marker = \"sys_platform == 'linux' and platform_machine == 'x86_64'\" }" "$co/orbit/pyproject.toml"
    grep -qE '^ *"tensorfs[][ <>=!~;"]' "$co/orbit/pyproject.toml" || sed -i '0,/^dependencies = \[$/s//dependencies = [\n    "tensorfs",/' "$co/orbit/pyproject.toml"
  fi
fi
git -C "$co" init -q -b rc/e2e && git -C "$co" add -A && git -C "$co" -c user.name=e2e -c user.email=e2e@localhost commit -qm checkout \
  || { check "from source: the checkout's commit failed"; return; }
sha=$(git -C "$co" rev-parse HEAD) short=${sha:0:8} said="rc/e2e @ ${sha:0:8}"

# local held on its candidates by hand: following the RC, it would move to the RC's builds at rest.
machine_ready
[ -z "$CAND_TFS" ] || args+=(--tensorfs "$CAND_TFS")
args+=(--runtime "${CAND_RT:-$(jq -r '.status.runtime // empty' "$OUT/machine.json")}")
cozy --json machine update local "${args[@]}" -y > "$OUT/source-hold.json" 2>&1 \
  || { check "from source: local refused its candidates by hand: $(tail -c 300 "$OUT/source-hold.json")"; return; }
cp "$cfg" "$OUT/source-config.toml"
{ echo 'dev = true'; grep -v '^dev *=' "$OUT/source-config.toml"; } > "$cfg.next"
if grep -q '^\[calcifer\]' "$cfg.next"; then
  sed -i "/^\[calcifer\]/a packages = \"$co\"" "$cfg.next"
else
  printf '\n[calcifer]\npackages = "%s"\n' "$co" >> "$cfg.next"
fi
mv "$cfg.next" "$cfg"
systemctl --user restart cozy-calcifer || { check "from source: Calcifer didn't restart"; cp "$OUT/source-config.toml" "$cfg"; return; }
machine_ready

if cell source-local minimax-h3-tools/echo "text=from the checkout"; then
  said_by source-local
  grep -q '"release": *"source"' "$OUT/source-local.ndjson" || check "from source: local's echo isn't the checkout's: $(tail -c 300 "$OUT/source-local.ndjson")"
  J --arg said "minimax-h3-tools from source: $said" '[.warnings[]? | select(.code == "from_source") | .message] == [$said]' "$OUT/source-local.json" \
    || check "from source: the record doesn't say '$said': $(jq -c '{target, warnings}' "$OUT/source-local.json")"
  J '.target == "_/minimax-h3-tools/echo"' "$OUT/source-local.json" || check "from source: its target is $(jq -c .target "$OUT/source-local.json")"
fi
in_project "$RUN_PROJECT" --json run e2e-boxed > "$OUT/source-functions.json" 2> "$OUT/source-functions.err" \
  && J --arg said "$said" '.release_from == "from source" and .release == $said' "$OUT/source-functions.json" \
  || check "from source: its functions don't say they're from source at $said: $(head -c 300 "$OUT/source-functions.json") $(tail -c 200 "$OUT/source-functions.err")"
in_project "$RUN_PROJECT" run minimax-h3-tools/echo --describe > "$OUT/source-describe.txt" 2>&1 && grep -qF "from source: $said" "$OUT/source-describe.txt" \
  || check "from source: its contract doesn't say 'from source: $said': $(head -c 300 "$OUT/source-describe.txt")"
cozy --json machine show local > "$OUT/source-machine.json" && echo "from source: local follows the RC: $(jq -c .rc "$OUT/source-machine.json")" && J '.rc.state == "held"' "$OUT/source-machine.json" \
  || check "from source: local doesn't follow the RC (held by hand): $(jq -c .rc "$OUT/source-machine.json")"

# The checkout's caller calls the checkout's minimax-h3-tools, named as the package it is.
if cell source-callee e2e-source-relay/relay "text=called from the checkout"; then
  said_by source-callee
  grep -q '"release": *"source"' "$OUT/source-callee.ndjson" || check "from source: the caller's minimax-h3-tools isn't the checkout's: $(tail -c 300 "$OUT/source-callee.ndjson")"
  # A project's local code is installed in the project (.vault-machine).
  callee=$(find "$RUN_PROJECT/.vault-machine" "$HOME/.cozy/machine" -name generation.json -path '*generations*' -exec jq -r \
    'select(.package == "e2e-source-relay") | .callees[]? | select(.distribution | ascii_downcase | gsub("[-_.]+"; "-") == "minimax-h3-tools") | .package' \
    {} + 2> /dev/null | sort -u | paste -sd, -)
  echo "from source: e2e-source-relay's environment names its callee ${callee:-nothing}"
  [ "$callee" = _/minimax-h3-tools ] || check "from source: e2e-source-relay's environment names its callee ${callee:-nothing}, not _/minimax-h3-tools"
fi

# The real Orbit from source: its contract says the checkout, and the lock it was read with takes minimax-h3 from the checkout's folder (its
# `{ index = "tensorhub" }` source staged as a path): the Hub's published H3 has the old call syntax and would not describe it.
if [ -d "$co/orbit" ]; then
  in_project "$RUN_PROJECT" run orbit/orbit --describe > "$OUT/source-orbit.txt" 2>&1 && grep -qF "from source: $said" "$OUT/source-orbit.txt" \
    || check "from source: orbit's contract doesn't say 'from source: $said': $(head -c 600 "$OUT/source-orbit.txt")"
  lock=$(grep -l '^name = "orbit"$' $(find "$HOME/.cozy" "$RUN_PROJECT" -path '*/locked-*/lock' -newer "$co/orbit/pyproject.toml" 2> /dev/null) /dev/null 2> /dev/null | head -1)
  echo "from source: orbit's lock ${lock:-was not made}: $(grep -A2 '^name = "minimax-h3"$' "${lock:-/dev/null}" 2> /dev/null | tr '\n' ' ')"
  grep -A2 '^name = "minimax-h3"$' "${lock:-/dev/null}" 2> /dev/null | grep -q 'source = { directory = "../minimax-h3" }' \
    || check "from source: orbit's lock doesn't take minimax-h3 from the checkout's folder: ${lock:-no lock made}"
fi

# A release named, and an account's package, are the Hub's.
if cell source-release minimax-h3-tools/echo@0.1.0 "text=a release"; then
  said_by source-release
  grep -q '"release": *"0.1.0"' "$OUT/source-release.ndjson" || check "from source: @0.1.0 isn't the Hub's: $(tail -c 300 "$OUT/source-release.ndjson")"
  J '[.warnings[]? | select(.code == "from_source")] == []' "$OUT/source-release.json" || check "from source: @0.1.0 says it's from source"
fi
if [ -n "${E2E_RELAY:-}" ] && cell source-account "$E2E_RELAY/relay" "text=an account's"; then
  said_by source-account
  grep -q '"release": *"0.1.0"' "$OUT/source-account.ndjson" || check "from source: $E2E_RELAY's callee isn't the Hub's 0.1.0: $(tail -c 300 "$OUT/source-account.ndjson")"
  J '[.warnings[]? | select(.code == "from_source")] == []' "$OUT/source-account.json" || check "from source: $E2E_RELAY says it's from source"
fi

# The page's tools: the checkout's, saying its branch and commit. A package's first reading (its lock, its description) can outlast a
# list: it's listed once read.
for _ in 1 2 3 4 5 6; do
  curl --fail-with-body -sS -H "Authorization: Bearer $(jq -r .token "$holder")" "http://$(jq -r .address "$holder")/v1/tools" > "$OUT/source-tools.json" 2>&1 \
    && J '[.tools[] | select(.id == "e2e-boxed/one_field")] != []' "$OUT/source-tools.json" && break
  sleep 5
done
echo "from source: the page's tools: $(jq -c '[.tools[]? | select(.id | startswith("e2e-boxed/")) | {id, release, source}] | .[:2]' "$OUT/source-tools.json" 2> /dev/null)"
J --arg said "$said" '[.tools[] | select(.id == "e2e-boxed/one_field") | .source] == [$said]' "$OUT/source-tools.json" \
  || check "from source: the page's tools don't list e2e-boxed/one_field from $said: $(jq -c '[.tools[]? | {id, release, source}] | .[:8]' "$OUT/source-tools.json" 2> /dev/null || head -c 300 "$OUT/source-tools.json")"

# Its reading is kept across Calcifer's restart: the first list after one has it, nothing left to read.
systemctl --user restart cozy-calcifer || check "from source: Calcifer didn't restart"
machine_ready
curl --fail-with-body -sS -H "Authorization: Bearer $(jq -r .token "$holder")" "http://$(jq -r .address "$holder")/v1/tools" > "$OUT/source-tools-again.json" 2>&1
echo "from source: after a restart, the page's tools: $(jq -c '{boxed: [.tools[]? | select(.id | startswith("e2e-boxed/")) | .id] | length, note}' "$OUT/source-tools-again.json" 2> /dev/null)"
J --arg said "$said" '[.tools[] | select(.id == "e2e-boxed/one_field") | .source] == [$said] and .note == null' "$OUT/source-tools-again.json" \
  || check "from source: after a restart, the first list read the checkout again: $(head -c 300 "$OUT/source-tools-again.json")"

source_reads "$co" "$said"

# Another machine (stand-s, added by source_reads) is sent the source by content.
if [ "${SOURCE_STAND:-}" = 1 ]; then
  if run_cell "$OUT" source-stand minimax-h3-tools/echo "text=on a stand-in" --on stand-s; then
    said_by source-stand
    grep -q '"release": *"source"' "$OUT/source-stand.ndjson" || check "from source: stand-s's echo isn't the checkout's: $(tail -c 300 "$OUT/source-stand.ndjson")"
    J '.machine == "stand-s"' "$OUT/source-stand.json" || check "from source: it ran on $(jq -c .machine "$OUT/source-stand.json"), not stand-s"
    J --arg said "minimax-h3-tools from source: $said" '[.warnings[]? | select(.code == "from_source") | .message] == [$said]' "$OUT/source-stand.json" \
      || check "from source: stand-s's record doesn't say '$said': $(jq -c .warnings "$OUT/source-stand.json")"
  else
    check "from source: on stand-s: $(run_get "$OUT" source-stand .reason.message) $(tail -c 300 "$OUT/source-stand.err" 2> /dev/null)"
  fi
  cozy --json machine end stand-s -y > "$OUT/source-stand-end.json" 2>&1 || check "from source: stand-s not removed: $(tail -c 300 "$OUT/source-stand-end.json")"
fi

# A slow reading never empties the page's tools (11-13: a busy Calcifer read the checkout past every list's wait, and Paul's palette
# showed no first-party tool). e2e-boxed takes a new dependency in a new commit, so its next reading locks and describes it again:
# the list asked at once has its tools within a few seconds, the last good ones or the new, and a later list the new commit's.
sed -i 's/"msgspec>=0.19"\]/"msgspec>=0.19", "six>=1.16"]/' "$co/e2e-boxed/pyproject.toml"
git -C "$co" -c user.name=e2e -c user.email=e2e@localhost commit -qam "e2e-boxed takes six" || check "from source: the checkout's second commit failed"
local said2 t0 took shown
said2="rc/e2e @ $(git -C "$co" rev-parse HEAD | cut -c1-8)"
t0=$(date +%s%3N)
curl --fail-with-body -sS -H "Authorization: Bearer $(jq -r .token "$holder")" "http://$(jq -r .address "$holder")/v1/tools" > "$OUT/source-tools-slow.json" 2>&1
took=$(( $(date +%s%3N) - t0 ))
shown=$(jq -r '[.tools[]? | select(.id == "e2e-boxed/one_field") | .source] | first // empty' "$OUT/source-tools-slow.json" 2> /dev/null)
echo "from source: listed at once after a new commit, in ${took} ms: e2e-boxed/one_field from ${shown:-nothing} (before: $said, now: $said2)"
{ [ "$shown" = "$said" ] || [ "$shown" = "$said2" ]; } && [ "$took" -lt 10000 ] \
  || check "from source: while e2e-boxed is read again, the list took ${took} ms and shows it from '${shown:-nothing}'"
for _ in $(seq 30); do
  curl --fail-with-body -sS -H "Authorization: Bearer $(jq -r .token "$holder")" "http://$(jq -r .address "$holder")/v1/tools" > "$OUT/source-tools-new.json" 2>&1
  J --arg said "$said2" '[.tools[] | select(.id == "e2e-boxed/one_field") | .source] == [$said]' "$OUT/source-tools-new.json" && break
  sleep 3
done
J --arg said "$said2" '[.tools[] | select(.id == "e2e-boxed/one_field") | .source] == [$said]' "$OUT/source-tools-new.json" \
  || check "from source: e2e-boxed's new commit never listed: $(jq -c '[.tools[]? | select(.id | startswith("e2e-boxed/")) | .source] | unique' "$OUT/source-tools-new.json" 2> /dev/null)"

# Off a dev home, the name is the Hub's again.
cp "$OUT/source-config.toml" "$cfg"
systemctl --user restart cozy-calcifer || { check "from source: Calcifer didn't restart"; return; }
machine_ready
if cell source-off minimax-h3-tools/echo "text=off"; then
  said_by source-off
  grep -q '"release": *"0.1.0"' "$OUT/source-off.ndjson" || check "from source: with dev off, the echo isn't the Hub's 0.1.0: $(tail -c 300 "$OUT/source-off.ndjson")"
fi
}

# source_reads <checkout> <what it says of its commit> (cl-1126). Paul, 11-17, of the page's tool panel saying "still reading from source, listed once read: anima,
# birefnet, …": "What is this? why is this text here? I don't like it. First of all, this doesn't need to be displayed to users. Second
# of all, what is it doing? is it trying to read all available packages from tensorhub over and over again or something?" And of the
# list he wants: "That list is not a list of what is installed on the machine, but what is available to install … Don't do polling,
# but ideally update this information as needed if the local packages are being updated / installed … perhaps calcifer can let our UI
# know when something changes?" The list is the same for every machine, and a dev home reads a checkout package's source once per
# state of it. Counted by Calcifer's uv, which logs each call here: a machine added (stand-s), a rental ordered (`rent-…`) and named by
# its Hub, and lists for each, again and again, run no uv lock or describe, and no list carries a note. One file of e2e-boxed changed
# is one describe, of it alone, and `tools.changed` reaches a page's stream, after which the list has the new tool.
source_reads() {
step "source reads (cl-1126): a machine added or renamed and lists again and again read no source; one file changed is one read, told to the page"
local co=$1 said=$2 lock=$HOME/.cozy/calcifer/calcifer.lock.holder uv real log=$OUT/source-uv.log page sse=$OUT/source-page.sse rental='' handle='' name='' on
local described failed locks told lists
SOURCE_STAND=''
: > "$OUT/source-reads.ndjson"
: > "$log"
# Calcifer's uv, logging each call it makes, and again as `failed <code>: ` one that failed: swapped in by a rename, so a call
# never finds it missing.
uv=$(ls -d "$HOME"/.cozy/tools/uv/*/uv 2> /dev/null | head -1)
[ -x "$uv" ] || { check "source reads: Calcifer's uv isn't in $HOME/.cozy/tools/uv"; return; }
real=$uv.real
ln -f "$uv" "$real" && printf '#!/bin/sh\nprintf "%%s\\n" "$*" >> "%s"\n"%s" "$@"; s=$?\n[ $s = 0 ] || printf "failed %%s: %%s\\n" "$s" "$*" >> "%s"\nexit $s\n' \
  "$log" "$real" "$log" > "$uv.counting" && chmod 755 "$uv.counting" \
  && mv -f "$uv.counting" "$uv" || { check "source reads: Calcifer's uv isn't counted"; rm -f "$real" "$uv.counting"; return; }
# A page's events stream, as the page's own fetch opens it (prefetch.sh's pre_page).
curl -sSN -H "Host: $(jq -r .address "$lock")" -H 'Sec-Fetch-Site: same-origin' -H 'Sec-Fetch-Mode: cors' -H 'Sec-Fetch-Dest: empty' \
  -H 'Accept: text/event-stream' "http://$(jq -r .address "$lock")/v1/events" > "$sse" 2>&1 & page=$!
# source_listed <name> [machine]: a list as the page asks it, naming the machine it shows (by default <name>), in source-reads-<name>.json;
# a line of what it said in source-reads.ndjson.
source_listed() {
  local file=$OUT/source-reads-$1.json on=${2:-$1} t0
  t0=$(date +%s%3N)
  curl --fail-with-body -sS -H "Authorization: Bearer $(jq -r .token "$lock")" "http://$(jq -r .address "$lock")/v1/tools?on=$on" > "$file" 2>&1 \
    || { check "source reads: GET /v1/tools?on=$on: $(head -c 300 "$file")"; return 1; }
  jq -c --arg on "$on" --argjson ms $(($(date +%s%3N) - t0)) '{on: $on, ms: $ms, note, boxed: [.tools[] | select(.id | startswith("e2e-boxed/")) | .id]}' "$file" >> "$OUT/source-reads.ndjson"
}
source_listed local
cozy --json machine show local > "$OUT/set-local.json"
stand_in stand-s && SOURCE_STAND=1
[ -z "$SOURCE_STAND" ] || source_listed stand-s
# A rental under its handle until its Hub names the host, as rent-24713671 became kigurumi, then cecilia, on Paul's laptop.
if signin "$E2E_HUB_EMAIL" > "$OUT/source-reads-login.json"; then
  rename_order source
  [ -n "$rental" ] || check "source reads: no rental accepted: $(tail -c 300 "$OUT/rename-rent-source.ndjson")"
  for _ in $(seq 120); do
    [ -n "$rental" ] || break
    name=$(cozy --json machine ls 2> /dev/null | jq -r --arg r "$rental" '.machines[] | select(.rental_id == $r and .placeholder == null) | .name' 2> /dev/null || true)
    [ -n "$name" ] && [[ $name != rent-* ]] && break
    source_listed "${handle:-$rental}"
    sleep 1
  done
  [ -z "$rental" ] || [[ $name != rent-* && -n $name ]] || check "source reads: its Hub never named $rental's host in 2 min"
  for on in "$name" local stand-s "$name"; do [ -z "$on" ] || source_listed "$on"; done
  [ -z "$rental" ] || cozy --json machine end "$rental" -y > "$OUT/source-reads-end.json" 2>&1 || check "source reads: machine end $rental: $(tail -c 300 "$OUT/source-reads-end.json")"
  cozy --json logout > /dev/null 2>&1 || true
else
  check "source reads: cozy login failed: $(tail -c 300 "$OUT/source-reads-login.json")"
fi
described=$(grep -v '^failed ' "$log" | grep -c ' describe$' || true) locks=$(grep -c '^lock ' "$log" || true) lists=$(wc -l < "$OUT/source-reads.ndjson")
echo "source reads: $lists lists, for local, stand-s, ${handle:-no handle} and ${name:-no name}: uv ran $described describes and $locks locks;" \
  "slowest list $(jq -s 'map(.ms) | max' "$OUT/source-reads.ndjson") ms"
[ "$described" = 0 ] && [ "$locks" = 0 ] \
  || check "source reads: a machine added or renamed, and $lists lists, ran uv $described describes and $locks locks: $(grep -E ' describe$|^lock ' "$log" | cut -c1-200 | head -3 | tr '\n' ';')"
jq -se 'all(.[]; .note == null)' "$OUT/source-reads.ndjson" > /dev/null \
  || check "source reads: a list carried a note: $(jq -sc '[.[] | select(.note != null) | {on, note}] | .[:3]' "$OUT/source-reads.ndjson")"
jq -se 'all(.[]; .boxed | index("e2e-boxed/one_field"))' "$OUT/source-reads.ndjson" > /dev/null \
  || check "source reads: a list left out the checkout's tools: $(jq -sc '[.[] | select(.boxed | index("e2e-boxed/one_field") | not) | .on] | unique' "$OUT/source-reads.ndjson")"

# One file of e2e-boxed changed, uncommitted: read once, of it alone, and the page told.
: > "$log"
told=$(sse "$sse" | jq -c 'select(.type == "tools.changed")' | wc -l)
cat >> "$co/e2e-boxed/e2e_boxed/__init__.py" << 'PY'


@app.tool()
def edited(payload: Shaded) -> Picture:
    """A still added while the checkout is watched (cl-1126)."""
    return _picture()
PY
for _ in $(seq 240); do
  if [ "$(sse "$sse" | jq -c 'select(.type == "tools.changed")' | wc -l)" -gt "$told" ]; then
    told=$(sse "$sse" | jq -c 'select(.type == "tools.changed")' | wc -l)
    source_listed edited local && J '[.tools[] | select(.id == "e2e-boxed/edited")] != []' "$OUT/source-reads-edited.json" && break
  fi
  sleep 0.5
done
# What each describe read: the package its folder's pyproject.toml names. One that failed (GitHub refusing a wheel's download,
# 44294540054) is read again by its retry, not a second reading.
read_of() { sed -n 's/.* --dir \([^ ]*\) describe$/\1/p' | while read -r dir; do sed -n 's/^name = "\(.*\)"$/\1/p' "$dir/pyproject.toml" | head -1; done | sort; }
failed=$(grep '^failed ' "$log" | read_of | paste -sd, - || true)
described=$(comm -23 <(grep -v '^failed ' "$log" | read_of) <(grep '^failed ' "$log" | read_of) | paste -sd, -)
echo "source reads: e2e-boxed changed: the page's stream was told tools.changed $told times; uv described ${described:-nothing}${failed:+ (and failed: $failed)}; listed:" \
  "$(jq -c '[.tools[]? | select(.id | startswith("e2e-boxed/")) | {id, source}]' "$OUT/source-reads-edited.json" 2> /dev/null)"
J '[.tools[] | select(.id == "e2e-boxed/edited")] != []' "$OUT/source-reads-edited.json" 2> /dev/null \
  || check "source reads: e2e-boxed's new tool wasn't listed after the page was told tools.changed $told times in 2 min"
[ "$described" = e2e-boxed ] || check "source reads: one changed file of e2e-boxed described ${described:-nothing}, not e2e-boxed once"
kill "$page" 2> /dev/null || true
mv -f "$real" "$uv" || check "source reads: Calcifer's uv wasn't put back"
# The checkout as it was, for what follows: listed at its commit again.
git -C "$co" checkout -q -- e2e-boxed
for _ in $(seq 120); do
  source_listed back local && J --arg said "$said" '[.tools[] | select(.id | startswith("e2e-boxed/")) | .source] | unique == [$said]' "$OUT/source-reads-back.json" && break
  sleep 1
done
J --arg said "$said" '[.tools[] | select(.id | startswith("e2e-boxed/")) | .source] | unique == [$said]' "$OUT/source-reads-back.json" \
  || check "source reads: e2e-boxed put back isn't listed at $said: $(jq -c '[.tools[]? | select(.id | startswith("e2e-boxed/")) | {id, source}]' "$OUT/source-reads-back.json" 2> /dev/null)"
}

# tools_offline (cl-1126). Paul, 11-17: "If the user is offline, or tensorhub is down, presumably we'll have to only display whatever
# packages are already installed (we should also include those installed from remote sources, like just some github link, or some
# alternative tensorhub, or ones being built locally). That will be a good fallback, and also these tools should be listed anyway."
# e2e-final, installed on local, is listed at once and with no note with its Hub away, after a restart that kept no catalog.
tools_offline() {
step "tools offline (cl-1126): its Hub away and no catalog kept, the list holds what's installed, at once, with no note"
[ -n "$LOCAL" ] || return 0
[ -n "${E2E_FINAL:-}" ] || { check "tools offline: isolated.sh published no e2e-final: ${E2E_FINAL_REFUSED:-not seeded}"; return; }
[ -n "$HUB_CHECK" ] || { check "tools offline: no hubcheck to stop"; return; }
local lock=$HOME/.cozy/calcifer/calcifer.lock.holder tool=$E2E_FINAL/echo addr t0 took
offline_list() {
  t0=$(date +%s%3N)
  curl --fail-with-body -sS -H "Authorization: Bearer $(jq -r .token "$lock")" "http://$(jq -r .address "$lock")/v1/tools" > "$OUT/$1.json" 2>&1
  took=$(($(date +%s%3N) - t0))
}
cozy --json package install "$E2E_FINAL" --on local > "$OUT/offline-install.ndjson" 2> "$OUT/offline-install.err" \
  || { check "tools offline: package install $E2E_FINAL --on local: $(tail -c 300 "$OUT/offline-install.err")"; return; }
for _ in $(seq 12); do
  offline_list offline-up && J --arg id "$tool" '[.tools[] | select(.id == $id)] != []' "$OUT/offline-up.json" && break
  sleep 5
done
J --arg id "$tool" '[.tools[] | select(.id == $id)] != []' "$OUT/offline-up.json" \
  || { check "tools offline: with its Hub up, $tool isn't listed once installed: $(head -c 300 "$OUT/offline-up.json")"; return; }
addr=$(head -1 "$OUT/hubcheck.addr")
kill "$HUB_CHECK" && wait "$HUB_CHECK" 2> /dev/null || true
curl -sS -m 5 -o /dev/null "http://$addr/" 2> /dev/null && check "tools offline: something still answers at $addr"
rm -f "$HOME"/.cozy/packages/*/listed.json
systemctl --user restart cozy-calcifer || check "tools offline: Calcifer didn't restart"
machine_ready
offline_list offline-down
echo "tools offline: with its Hub away and no catalog kept, the list answered in $took ms: $(jq -c '{tools: [.tools[]? | .id], note}' "$OUT/offline-down.json" 2> /dev/null | head -c 400)"
J --arg id "$tool" '[.tools[] | select(.id == $id)] != [] and .note == null' "$OUT/offline-down.json" && [ "$took" -lt 3000 ] \
  || check "tools offline: with its Hub away, the list took $took ms: $(head -c 400 "$OUT/offline-down.json")"
hub_check_again "$addr"
}

# catalog_unchanged (cl-1174, cl-1126's leftover). Paul, 11-17: "we just fetch them from tensorhub once, and maybe re-fetch every couple
# minutes (if there is an update / new packages / tools released)". Each refresh asks the Hub's catalog with the ETag it last answered
# (`If-None-Match`), and keeps its copy on a 304 (th-401). Two pages connect, each refreshing it, 11 s apart: the second is answered 304
# (hubcheck counts it), and the list is what it was. A Hub that sends no ETag (before th-401) answers each page whole.
catalog_unchanged() {
step "catalog unchanged (cl-1174): a refresh asks the Hub's catalog only if it changed, and keeps its copy on a 304"
[ -n "$LOCAL" ] || return 0
local lock=$HOME/.cozy/calcifer/calcifer.lock.holder addr etag before after whole unchanged
addr=$(jq -r .address "$lock")
# hubcheck's count of the catalog's reads: all of them, and those the Hub answered 304. Read again while hubcheck rewrites the file.
catalog_calls() {
  local n
  for _ in $(seq 20); do
    n=$(jq -r --arg op "GET /v1/packages${1:-}" '.[$op] // 0' "$OUT/hub-calls.json" 2> /dev/null) && [ -n "$n" ] && { echo "$n"; return; }
    sleep 0.1
  done
  echo 0
}
catalog_tools() { curl --fail-with-body -sS -H "Authorization: Bearer $(jq -r .token "$lock")" "http://$addr/v1/tools" > "$OUT/catalog-tools-$1.json" 2>&1; }
etag=$(curl -sS -D - -o /dev/null "$COZY_HUB_URL/v1/packages?limit=200&offset=0" 2> /dev/null | tr -d '\r' | sed -n 's/^[Ee][Tt][Aa][Gg]: *//p' | head -1)
for i in 1 2; do
  before=$(catalog_calls) unchanged=$(catalog_calls ' 304')
  # A page connecting: its events stream, as the page's own fetch opens it, held 10 s (its answer waits on the machines' list). One
  # within 10 s of the last read is that read: again until one reads it.
  for _ in $(seq 6); do
    timeout 10 curl -sSN -H "Host: $addr" -H 'Sec-Fetch-Site: same-origin' -H 'Sec-Fetch-Mode: cors' -H 'Sec-Fetch-Dest: empty' \
      -H 'Accept: text/event-stream' "http://$addr/v1/events" > /dev/null 2>&1 || true
    [ "$(catalog_calls)" -gt "$before" ] && break
  done
  [ "$(catalog_calls)" -gt "$before" ] || { check "catalog unchanged: page $i connecting read no catalog in 60 s: $(jq -c . "$OUT/hub-calls.json" 2> /dev/null | head -c 300)"; return; }
  # A refresh within 10 s is the one already made.
  [ "$i" = 2 ] || { sleep 11; catalog_tools first; }
done
sleep 2
after=$(catalog_calls) whole=$(($(catalog_calls) - before)) unchanged=$(($(catalog_calls ' 304') - unchanged))
catalog_tools second
echo "catalog unchanged: the Hub's ETag ${etag:-none}; the second refresh asked $whole pages, $unchanged answered 304 ($after catalog reads in all)"
if [ -n "$etag" ]; then
  [ "$whole" -ge 1 ] && [ "$unchanged" = "$whole" ] \
    || check "catalog unchanged: the Hub tags its catalog ($etag), yet the second refresh's $whole pages had $unchanged answered 304"
else
  [ "$whole" -ge 1 ] && [ "$unchanged" = 0 ] || check "catalog unchanged: a Hub with no ETag: $whole pages read, $unchanged answered 304"
fi
J --slurpfile was "$OUT/catalog-tools-first.json" '([.tools[].id] | sort) == ($was[0].tools | map(.id) | sort) and (.tools | length) > 0' "$OUT/catalog-tools-second.json" \
  || check "catalog unchanged: the list after the second refresh isn't the one before it: $(jq -c '[.tools[]? | .id]' "$OUT/catalog-tools-first.json" 2> /dev/null | head -c 300) then $(jq -c '[.tools[]? | .id]' "$OUT/catalog-tools-second.json" 2> /dev/null | head -c 300)"
}
