# from_source (xs-195, Paul 11-13: nothing untested goes to the Hub, so RC testing runs packages from source). A dev home
# (`dev = true`, an RC build) names a packages checkout in its config (`[calcifer] packages`): a first-party package it holds runs
# from its source wherever its unversioned name is asked, here the isolated Hub's root minimax-h3-tools, whose echo says
# "source" instead of its release's 0.1.0. On local and on a stand-in (another user's cozy on this runner: this Hub's docker
# rentals never get ready), each run's record says the checkout's branch and commit. A package of the checkout that takes
# minimax-h3-tools from the Hub's index (as orbit takes minimax-h3) calls the checkout's, as cargo's [patch] does. A release
# named (`@0.1.0`) and an account's package come from the Hub, and the page's tools list the checkout's from source. `local`
# follows the RC (held here by hand, so the shard keeps its candidates). With `dev = false` the same name is the Hub's again.
from_source() {
step "a dev home's packages checkout: first-party packages run from source, on local and a stand-in (xs-195)"
[ -n "$LOCAL" ] || return 0
[ "${RUNNER_OS:-}" = Linux ] && [ "${GITHUB_ACTIONS:-}" = true ] || { echo "from source: its stand-in is another user of a fresh GitHub runner"; return; }
case $("$COZY" --version) in *-dev+g*) ;; *) echo "from source: $("$COZY" --version) is no RC build, which reads no checkout"; return ;; esac
local co=$OUT/packages-rc cfg=$HOME/.cozy/config.toml holder=$HOME/.cozy/calcifer/calcifer.lock.holder sha short said callee args=()
rm -rf "$co" && mkdir -p "$co"
# minimax-h3-tools as isolated.sh publishes it at the root, its echo saying it's the source.
cp -r "$here/../../release-gate/suites/hub/pkg" "$co/minimax-h3-tools" && mv "$co/minimax-h3-tools/ci_hub_pkg" "$co/minimax-h3-tools/minimax_h3_tools"
sed -i -e 's/^name = "ci-hub-pkg"/name = "minimax-h3-tools"/' -e 's/ci_hub_pkg/minimax_h3_tools/g' "$co/minimax-h3-tools/pyproject.toml" "$co/minimax-h3-tools/package.toml"
sed -i 's/^RELEASE = "0.1.0"$/RELEASE = "source"/' "$co/minimax-h3-tools/minimax_h3_tools/__init__.py"
package_at "$co/e2e-source-relay" e2e_source_relay '"cozy-runtime>=0.45", "msgspec>=0.19", "minimax-h3-tools"' \
  "$(printf '\n[tool.uv.sources]\nminimax-h3-tools = { index = "tensorhub" }')" <<'PY'
import minimax_h3_tools
import msgspec

from cozy_runtime.author import App, call

app = App()


class Relay(msgspec.Struct, forbid_unknown_fields=True):
    text: str


class Relayed(msgspec.Struct):
    text: str
    release: str


async def relay(payload: Relay) -> Relayed:
    echoed = await call(minimax_h3_tools.echo, payload=minimax_h3_tools.Text(text=payload.text))
    return Relayed(text=echoed.text, release=echoed.release)


app.entrypoint(relay, accelerator=False)
PY
# Tools a box is made of, for the page's list.
cp -r "$here/project/boxed" "$co/e2e-boxed"
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
  grep -q '"release": *"source"' "$OUT/source-local.ndjson" || check "from source: local's echo isn't the checkout's: $(tail -c 300 "$OUT/source-local.ndjson")"
  J --arg said "minimax-h3-tools from source: $said" '[.warnings[]? | select(.code == "from_source") | .message] == [$said]' "$OUT/source-local.json" \
    || check "from source: the record doesn't say '$said': $(jq -c '{target, warnings}' "$OUT/source-local.json")"
  J '.target == "_/minimax-h3-tools/echo"' "$OUT/source-local.json" || check "from source: its target is $(jq -c .target "$OUT/source-local.json")"
fi
in_project "$RUN_PROJECT" --json run minimax-h3-tools > "$OUT/source-functions.json" 2> "$OUT/source-functions.err" \
  && J --arg said "$said" '.release_from == "from source" and .release == $said' "$OUT/source-functions.json" \
  || check "from source: its functions don't say they're from source at $said: $(head -c 300 "$OUT/source-functions.json") $(tail -c 200 "$OUT/source-functions.err")"
in_project "$RUN_PROJECT" run minimax-h3-tools/echo --describe > "$OUT/source-describe.txt" 2>&1 && grep -qF "from source: $said" "$OUT/source-describe.txt" \
  || check "from source: its contract doesn't say 'from source: $said': $(head -c 300 "$OUT/source-describe.txt")"
cozy --json machine show local > "$OUT/source-machine.json" && J '.rc.state == "held"' "$OUT/source-machine.json" \
  || check "from source: local doesn't follow the RC (held by hand): $(jq -c .rc "$OUT/source-machine.json")"

# The checkout's caller calls the checkout's minimax-h3-tools, named as the package it is.
if cell source-callee e2e-source-relay/relay "text=called from the checkout"; then
  grep -q '"release": *"source"' "$OUT/source-callee.ndjson" || check "from source: the caller's minimax-h3-tools isn't the checkout's: $(tail -c 300 "$OUT/source-callee.ndjson")"
  callee=$(find "$HOME/.cozy/machine" -name generation.json -path '*generations*' -exec jq -r \
    'select(.package == "e2e-source-relay") | .callees[]? | select(.distribution | ascii_downcase | gsub("[-_.]+"; "-") == "minimax-h3-tools") | .package' \
    {} + 2> /dev/null | sort -u | paste -sd, -)
  [ "$callee" = _/minimax-h3-tools ] || check "from source: e2e-source-relay's environment names its callee ${callee:-nothing}, not _/minimax-h3-tools"
fi

# A release named, and an account's package, are the Hub's.
if cell source-release minimax-h3-tools/echo@0.1.0 "text=a release"; then
  grep -q '"release": *"0.1.0"' "$OUT/source-release.ndjson" || check "from source: @0.1.0 isn't the Hub's: $(tail -c 300 "$OUT/source-release.ndjson")"
  J '[.warnings[]? | select(.code == "from_source")] == []' "$OUT/source-release.json" || check "from source: @0.1.0 says it's from source"
fi
if [ -n "${E2E_RELAY:-}" ] && cell source-account "$E2E_RELAY/relay" "text=an account's"; then
  grep -q '"release": *"0.1.0"' "$OUT/source-account.ndjson" || check "from source: $E2E_RELAY's callee isn't the Hub's 0.1.0: $(tail -c 300 "$OUT/source-account.ndjson")"
  J '[.warnings[]? | select(.code == "from_source")] == []' "$OUT/source-account.json" || check "from source: $E2E_RELAY says it's from source"
fi

# The page's tools: the checkout's, saying its branch and commit.
curl --fail-with-body -sS -H "Authorization: Bearer $(jq -r .token "$holder")" "http://$(jq -r .address "$holder")/v1/tools?on=local" > "$OUT/source-tools.json" 2>&1 \
  && J --arg said "$said" '[.tools[] | select(.id == "e2e-boxed/one_field") | .source] == [$said]' "$OUT/source-tools.json" \
  || check "from source: the page's tools don't list e2e-boxed/one_field from $said: $(jq -c '[.tools[]? | {id, release, source}] | .[:8]' "$OUT/source-tools.json" 2> /dev/null || head -c 300 "$OUT/source-tools.json"), $(jq -c .note "$OUT/source-tools.json" 2> /dev/null)"

# Another machine is sent the source by content.
cozy --json machine show local > "$OUT/set-local.json"
if stand_in stand-s; then
  if run_cell "$OUT" source-stand minimax-h3-tools/echo "text=on a stand-in" --on stand-s; then
    grep -q '"release": *"source"' "$OUT/source-stand.ndjson" || check "from source: stand-s's echo isn't the checkout's: $(tail -c 300 "$OUT/source-stand.ndjson")"
    J '.machine == "stand-s"' "$OUT/source-stand.json" || check "from source: it ran on $(jq -c .machine "$OUT/source-stand.json"), not stand-s"
    J --arg said "minimax-h3-tools from source: $said" '[.warnings[]? | select(.code == "from_source") | .message] == [$said]' "$OUT/source-stand.json" \
      || check "from source: stand-s's record doesn't say '$said': $(jq -c .warnings "$OUT/source-stand.json")"
  else
    check "from source: on stand-s: $(run_get "$OUT" source-stand .reason.message) $(tail -c 300 "$OUT/source-stand.err" 2> /dev/null)"
  fi
  cozy --json machine end stand-s -y > "$OUT/source-stand-end.json" 2>&1 || check "from source: stand-s not removed: $(tail -c 300 "$OUT/source-stand-end.json")"
fi

# Off a dev home, the name is the Hub's again.
cp "$OUT/source-config.toml" "$cfg"
systemctl --user restart cozy-calcifer || { check "from source: Calcifer didn't restart"; return; }
machine_ready
if cell source-off minimax-h3-tools/echo "text=off"; then
  grep -q '"release": *"0.1.0"' "$OUT/source-off.ndjson" || check "from source: with dev off, the echo isn't the Hub's 0.1.0: $(tail -c 300 "$OUT/source-off.ndjson")"
fi
}
