# e2e/cli/commands.sh: the rest of cozy's PR-tier commands and flags (e2e/coverage.toml), parts cli/run.sh calls
# after its runs or in its own shards: signed out, CPU, the local machine up (where this OS has one). Each call's
# answer is checked, not its exit code alone. Signed-in and renting commands are the release suite's (e2e/release/run.sh).
here=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
J() { jq -e "$@" > /dev/null; }  # J <filter> <file>: the answer holds what the filter asks

# What needs no machine: Calcifer, the Hub setting, signing out, the Hub's packages and models.
catalog() {
step status
cozy --json status > "$OUT/status.json" || check "cozy status failed"
J '.calcifer.running == true and (.calcifer.version | length > 0)' "$OUT/status.json" || check "status: Calcifer is not running"
if [ -n "$LOCAL" ]; then
  J '.machines | any(.name == "local")' "$OUT/status.json" || check "status: the local machine is not listed"
else
  J '.machines | all(.name != "local")' "$OUT/status.json" || check "status: lists a local machine this OS cannot run"
fi

step hub
# tensorhub.com unless COZY_HUB_URL (or config.toml's hub_url) names another: one there is none at fails.
! COZY_HUB_URL=http://127.0.0.1:9 cozy --json package search birefnet --limit 1 > "$OUT/hub-env.json" 2>&1 \
  || check "COZY_HUB_URL=http://127.0.0.1:9: package search did not go there"
# A run made at that other Hub (local code reads none), then the variable unset: Calcifer still lists and shows it.
if [ -n "$LOCAL" ] && (cd "$here" && COZY_HUB_URL=http://127.0.0.1:9 cell other-hub ./weightless/reverse "text=hub$RANDOM"); then
  n=$(cat "$OUT/other-hub.number")
  cozy --json run list --limit 0 | J --argjson n "$n" 'any(.runs[]; .number == $n and .hub == "http://127.0.0.1:9")' \
    || check "run list: run $n, made at another Hub, is not listed"
  cozy --json run show "$n" --once | J '.hub == "http://127.0.0.1:9" and .status == "completed"' || check "run show $n: not its record at the other Hub"
fi

step "signed out"
cozy --json whoami > "$OUT/whoami.json" && J '.logged_in == false' "$OUT/whoami.json" || check "whoami: not signed out"
cozy --json logout > "$OUT/logout.json" || check "logout while signed out failed"

step packages
cozy --json package search birefnet --limit 1 > "$OUT/search.json" && J '(.packages | length) == 1 and .packages[0].package == "birefnet"' "$OUT/search.json" \
  || check "package search --limit 1: not exactly birefnet"
cozy --json package info birefnet > "$OUT/package-info.json" && J '(.latest_release | length > 0) and (.releases | length > 0)' "$OUT/package-info.json" \
  || check "package info birefnet: no releases"
cozy --json run birefnet/remove_background --describe > "$OUT/describe-published.json" 2> "$OUT/describe-published.err" \
  && grep -Eq '(^|[[:space:]"])image("|:)' "$OUT/describe-published.json" || check "run birefnet/remove_background --describe: no image input"
cozy run birefnet > "$OUT/functions.txt" 2>&1 && grep -q remove_background "$OUT/functions.txt" || check "run birefnet: remove_background is not listed"

step models
cozy --json model search birefnet --limit 1 > "$OUT/models-search.json" && J '(.models | length) == 1' "$OUT/models-search.json" \
  || check "model search --limit 1: not one model"
cozy --json model search --limit 100 > "$OUT/models-all.json" || check "model search --limit 100: $(head -c 300 "$OUT/models-all.json")"
family=$(jq -r '[.models[].family | select(. != null)][0] // empty' "$OUT/models-all.json") || true
if [ -n "$family" ]; then
  cozy --json model search --family "$family" > "$OUT/family.json" && J --arg f "$family" '(.models | length > 0) and all(.models[]; .family == $f)' "$OUT/family.json" \
    || check "model search --family $family: other families listed"
else
  cozy --json model search --family e2e-no-such-family > "$OUT/family.json" && J '(.models | length) == 0' "$OUT/family.json" \
    || check "model search --family: an unknown family listed models"
fi
cozy --json model info birefnet > "$OUT/model-info.json" && J '.releases[0].lanes | length > 0' "$OUT/model-info.json" || check "model info birefnet: no lanes"
lane=$(jq -r '.releases[0].lanes[0].lane' "$OUT/model-info.json") || true

step "renting, signed out"
cozy machine rent > "$OUT/rent-catalog.txt" 2>&1 && grep -q 'rtx-4090' "$OUT/rent-catalog.txt" || check "machine rent: no catalog: $(head -c 300 "$OUT/rent-catalog.txt")"
code=0; cozy --json run birefnet/remove_background --asset "image=$(picture dog1.jpg)" --on new > "$OUT/rent-signed-out.json" 2>&1 || code=$?
[ "$code" != 0 ] && grep -q 'cozy login' "$OUT/rent-signed-out.json" || check "run --on new signed out: exit $code, not refused for sign-in: $(head -c 300 "$OUT/rent-signed-out.json")"
printf -- '-----BEGIN OPENSSH PRIVATE KEY-----\nAAAA\n-----END OPENSSH PRIVATE KEY-----\n' > "$OUT/id_private"
code=0; cozy --json machine rent rtx-4090 --ssh "$OUT/id_private" > "$OUT/rent-ssh.json" 2> "$OUT/rent-ssh.err" || code=$?
[ "$code" = 2 ] && J '.error.code == "ssh_key_invalid"' "$OUT/rent-ssh.json" || check "machine rent --ssh <a private key>: exit $code, not refused before any ask: $(head -c 300 "$OUT/rent-ssh.json")"
}

# This computer's machine: packages and models on it.
basics() {
step "packages on local"
# Install, list and remove: this suite's own package from its directory (local/e2e-weightless), so the step holds
# whatever the Hub has published (a published name is installed by the release suite's throwaway, and by every run).
cozy --json package install "$here/weightless" --on local > "$OUT/install.ndjson" \
  || check "package install ./weightless --on local failed: $(grep -oE '"message": *"[^"]*"' "$OUT/install.ndjson" | tail -1)"
cozy --json package list --on local > "$OUT/packages.json" && grep -q 'e2e-weightless' "$OUT/packages.json" || check "package list --on local: no local/e2e-weightless"
cozy --json package remove local/e2e-weightless --on local > "$OUT/remove.ndjson" || check "package remove local/e2e-weightless --on local failed"
cozy --json package list --on local > "$OUT/packages-after.json" && ! grep -q 'e2e-weightless' "$OUT/packages-after.json" \
  || check "package remove: local/e2e-weightless is still installed"

step "models on local"
cozy --json model download birefnet --lane "$lane" --on local > "$OUT/download.ndjson" || check "model download --lane $lane failed"
cozy --json model download birefnet local/e2e-birefnet > "$OUT/download-alias.ndjson" || check "model download birefnet local/e2e-birefnet failed"
# A local .safetensors file, made into a model the machine keeps by name: the file is written to the machine
# and made from its object (object://sha256:…/<name>).
tiny "$OUT/tiny.safetensors"
cozy --json model download "$OUT/tiny.safetensors" local/e2e-tiny > "$OUT/download-file.ndjson" || check "model download ./tiny.safetensors local/e2e-tiny failed: $(grep -oE '"message": *"[^"]*"' "$OUT/download-file.ndjson" | tail -1)"
cozy --json model list --on local > "$OUT/models.json" || check "model list --on local failed"
grep -q e2e-tiny "$OUT/models.json" || check "model list: the local file's local/e2e-tiny is not listed"
J --arg lane "$lane" 'any(.models[]; .model == "_/birefnet" and .lane == $lane)' "$OUT/models.json" || check "model list: no _/birefnet in lane $lane after --lane $lane"
grep -q e2e-birefnet "$OUT/models.json" || check "model list: the local/e2e-birefnet alias is not listed"
}

# tiny <file>: a 4×4 F16 .safetensors file.
tiny() {
uv run -q --no-project python -I - "$1" <<'PY'
import json, struct, sys
header = json.dumps({"w": {"dtype": "F16", "shape": [4, 4], "data_offsets": [0, 32]}}).encode()
header += b" " * (-len(header) % 8)
open(sys.argv[1], "wb").write(struct.pack("<Q", len(header)) + header + bytes(32))
PY
}

# Where this OS runs no models itself (macOS, Windows): every verb that would use this computer's machine refuses
# `local_unavailable` (exit 1) with the rental to use instead, before anything is staged or started; nothing ever
# starts a machine here or makes its root; `machine list` and `status` say so in one line; doctor is healthy.
refusals() {
step "local runs refused"
dog=$(picture dog2.jpg) key=AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA
printf 'image: %s\n' "$dog" > "$OUT/refused.yaml"
tiny "$OUT/refused.safetensors"
refused() {  # refused <name> <cozy args...>
  local name=$1 code=0; shift
  cozy --json "$@" > "$OUT/refused-$name.json" 2> "$OUT/refused-$name.err" || code=$?
  [ "$code" = 1 ] && J '.error.code == "local_unavailable" and (.error.remedy | contains("--on new"))' "$OUT/refused-$name.json" \
    || check "cozy $*: exit $code, not refused local_unavailable: $(head -c 300 "$OUT/refused-$name.json") $(head -c 200 "$OUT/refused-$name.err")"
}
refused run run birefnet/remove_background --asset "image=$dog"
refused run-on-local run birefnet/remove_background --input "$OUT/refused.yaml" --on local
refused run-warm run birefnet/remove_background --warm installed
(cd "$here" && refused run-code run ./weightless/reverse text=refused)
(cd "$here" && refused run-describe run ./weightless/reverse --describe)
refused install package install birefnet
refused install-code package install "$here/weightless"
refused remove package remove birefnet
refused download model download birefnet
refused upload model upload "$OUT/refused.safetensors" fidika/e2e-refused --lane f16
refused quantize model quantize anima --lane int8 --package anima
refused update machine update local --runtime 0.30.0 -y
refused show machine show local
refused logs machine logs local --lines 5
refused authorize machine authorize "$key"
refused doctor doctor --on local
cozy run birefnet/remove_background --asset "image=$dog" > "$OUT/refused.txt" 2>&1 && check "a local run without --on was not refused"
grep -q "local runs aren't available on" "$OUT/refused.txt" && grep -q -- '--on new' "$OUT/refused.txt" \
  || check "the refusal does not say local runs aren't available and --on new: $(head -c 300 "$OUT/refused.txt")"

step "no local machine"
cozy --json machine list > "$OUT/machines.json" && J '.machines | all(.name != "local")' "$OUT/machines.json" || check "machine list: a local machine is listed"
cozy machine list > "$OUT/machines.txt" 2>&1 && grep -q "local runs aren't available on" "$OUT/machines.txt" || check "machine list does not say local runs aren't available"
cozy status > "$OUT/status.txt" 2>&1 && grep -q "local runs aren't available on" "$OUT/status.txt" || check "status does not say local runs aren't available"
cozy --json doctor > "$OUT/doctor-gated.json" && J '.healthy and (.checks | any(.check == "machine" and (.state | contains("aren'"'"'t available"))))' "$OUT/doctor-gated.json" \
  || check "doctor: not healthy, or no line that local runs aren't available: $(jq -c .checks "$OUT/doctor-gated.json")"
cozy machine logs local calcifer --lines 5 > "$OUT/log-calcifer.txt" && [ -s "$OUT/log-calcifer.txt" ] || check "machine logs local calcifer: no Calcifer log"
cozy --json package list > "$OUT/packages.json" || check "package list failed"
cozy --json model list > "$OUT/models.json" || check "model list failed"
[ ! -e "$HOME/.cozy/machine/var/lib/cozy/tensord" ] || check "a machine root was made on this computer: $(ls "$HOME/.cozy/machine/var/lib/cozy")"
! pgrep -f "cozy.* machine serve" > /dev/null 2>&1 || check "a machine runs on this computer: $(pgrep -af 'machine serve')"
}

# macOS: the Homebrew tap's cozy (cozy-promote.yaml pushes each promoted release's formula) installs and runs, then goes.
homebrew() {
[ "$RUNNER_OS" = macOS ] || return 0
step homebrew
HOMEBREW_NO_AUTO_UPDATE=1 HOMEBREW_NO_INSTALL_CLEANUP=1 brew install cozy-creator/tap/cozy > "$OUT/brew.log" 2>&1 \
  || { check "brew install cozy-creator/tap/cozy failed: $(tail -c 300 "$OUT/brew.log")"; return; }
"$(brew --prefix)/bin/cozy" --version > "$OUT/brew-version.txt" 2>&1 && grep -q '^cozy [0-9]' "$OUT/brew-version.txt" \
  || check "Homebrew's cozy does not run: $(head -c 300 "$OUT/brew-version.txt")"
echo "homebrew: $(cat "$OUT/brew-version.txt")"
brew uninstall cozy > /dev/null 2>&1 || check "brew uninstall cozy failed"
}

yaml_requests() {
step "a YAML request, then inline inputs over it"
text=yaml-$RANDOM back=''
for ((i = ${#text} - 1; i >= 0; i--)); do back+=${text:i:1}; done
printf 'text: %s\n' "$text" > "$OUT/reverse.yaml"
if (cd "$here" && cell yaml ./weightless/reverse --input "$OUT/reverse.yaml"); then
  grep -q "$back" "$OUT/yaml.ndjson" || check "yaml: the reply is not $back"
fi
if (cd "$here" && cell yaml-over ./weightless/reverse --input "$OUT/reverse.yaml" text=inline); then
  grep -q enilni "$OUT/yaml-over.ndjson" || check "yaml-over: the inline text did not override the file's"
fi
dog=$(picture dog1.jpg)
printf 'image: %s\n' "$dog" > "$OUT/birefnet.yaml"
if cell birefnet-yaml "$(birefnet)" --input "$OUT/birefnet.yaml" --lane "$lane"; then
  white_background "$(output birefnet-yaml webp)" "$dog" || check "birefnet-yaml: not a background removal"
  J --arg lane "$lane" '[.. | .lane? // empty] | any(. == $lane)' "$OUT/birefnet-yaml.json" || check "birefnet-yaml --lane $lane: not that lane"
fi
}

# After a BiRefNet run: the warm levels find it installed.
run_flags() {
step "run flags"
before=$(cozy --json run list --limit 0 | jq '.runs | length') || true
(cd "$here" && cozy --json run ./weightless/reverse --describe) > "$OUT/describe.json" || check "run --describe failed"
J '.fields | any(.name == "text")' "$OUT/describe.json" || check "run --describe: no text field"
[ "$(cozy --json run list --limit 0 | jq '.runs | length')" = "$before" ] || check "run --describe started a run"
note=$OUT/inputs/note.txt; echo "an asset" > "$note"
if (cd "$here" && cell fidelity ./weightless/fidelities --asset "notes=$note" --asset-fidelity notes=high); then
  grep -q 'notes=high' "$OUT/fidelity.ndjson" || check "--asset-fidelity notes=high: the executor saw $(grep -o '"assets":\[[^]]*\]' "$OUT/fidelity.ndjson")"
fi
for level in installed downloaded imported host; do
  cozy --json run "$(birefnet)" --warm "$level" --on local > "$OUT/warm-$level.ndjson" || check "run --warm $level failed"
  cozy --json machine show local > "$OUT/warm-$level.json" || true
  # The warm set holds the level asked, or says why it holds less (a CPU binds no model on a GPU).
  J --arg l "$level" '.status.warm | any(.entrypoint == "remove_background" and .level == $l and (.holds == $l or (.held_back | length > 0)))' "$OUT/warm-$level.json" \
    || check "--warm $level: the warm set does not hold birefnet at $level: $(jq -c '.status.warm' "$OUT/warm-$level.json")"
done
cozy --json run "$(birefnet)" --warm off --on local > "$OUT/warm-off.ndjson" || check "run --warm off failed"
cozy --json machine show local | J '.status.warm | all(.entrypoint != "remove_background")' || check "--warm off: birefnet is still in the warm set"
}

run_follow() {
step "a run followed, a snapshot, a cancel"
(cd "$here" && cozy --json run ./weightless/wait seconds:=120 --on local > "$OUT/wait.ndjson" 2>&1) &
waiting=$!
# Its own events say its number, then that it runs (e2e/run.sh); its follower exiting first ends the wait.
until n=$(run_number "$OUT/wait.ndjson"); [ -n "$n" ] || ! kill -0 "$waiting" 2> /dev/null; do sleep 0.2; done
if [ -z "$n" ]; then check "the waiting run never got a number"; else
  until run_reached "$OUT/wait.ndjson" in_progress || ! kill -0 "$waiting" 2> /dev/null; do sleep 0.2; done
  cozy --json run show "$n" --once > "$OUT/snapshot.json" || true
  J '.status == "in_progress"' "$OUT/snapshot.json" || check "run show --once: run $n is $(jq -r .status "$OUT/snapshot.json"), not in_progress"
  cozy --json run show "$n" > "$OUT/followed.ndjson" &
  following=$!
  sleep 3
  kill -0 "$following" 2> /dev/null || check "run show $n stopped before the run settled (it should follow it)"
  cozy --json run cancel "$n" > "$OUT/cancel.json" || check "run cancel $n failed"
  wait "$following" || true
  wait "$waiting" || true
  grep -qE '"status": *"canceled"' "$OUT/followed.ndjson" || check "run show $n did not follow it to canceled"
  cozy --json run show "$n" --once > "$OUT/canceled.json" || true
  J '.status == "canceled"' "$OUT/canceled.json" || check "run cancel: run $n is $(jq -r '.status, .reason.code? // empty' "$OUT/canceled.json" | paste -sd' ' -), not canceled"
fi

step "run list filters"
cozy --json run list --state canceled > "$OUT/canceled.json" && J --argjson n "${n:-0}" '(.runs | length > 0) and all(.runs[]; .status == "canceled") and any(.runs[]; .number == $n)' "$OUT/canceled.json" \
  || check "run list --state canceled: wrong runs"
}

# After `basics`: its model downloads are runs of birefnet.
run_filters() {
step "run list filters"
cozy --json run list --package birefnet > "$OUT/by-package.json" && J '(.runs | length > 0) and all(.runs[]; .target | test("birefnet"))' "$OUT/by-package.json" \
  || check "run list --package birefnet: other packages listed"
cozy --json run list --on local > "$OUT/by-machine.json" && J '(.runs | length > 0) and all(.runs[]; .machine == "local")' "$OUT/by-machine.json" \
  || check "run list --on local: other machines listed"
cozy --json run list --limit 1 > "$OUT/limit.json" && J '(.runs | length) == 1' "$OUT/limit.json" || check "run list --limit 1: not one run"
}

# After the first BiRefNet run (run.sh's first_run).
run_play() {
step "run play"
# The link opens the browser player (GitHub Pages) with this machine, the output and a token in its fragment; the
# token's expiry is --expires from now.
first=$(cat "$OUT/first.number")
name=$(jq -r '.output[0].name // empty' "$OUT/first.json") || true
expiry() {  # the token's expiry (unix s), from the link's fragment (unpadded base64url JSON before its `.`)
  local t; t=$(sed -n 's/.*[#&]c=\([^.&]*\).*/\1/p' <<< "$1" | tr '_-' '/+')
  while (( ${#t} % 4 )); do t+==; done
  base64 -d <<< "$t" 2> /dev/null | jq -r '.e // empty' 2> /dev/null || true
}
if [ "$first" = none ] || [ -z "$name" ]; then check "run play: no BiRefNet output to play"; else
  for expires in 30s 2h; do
    url=$(cozy --json run play "$first" --output "$name" --expires "$expires" | tee "$OUT/play-$expires.json" | jq -r '.link // empty') || true
    curl -fsS -o /dev/null "${url%%#*}" || check "run play: its player page ${url%%#*} does not answer"
    [[ $url == *"o=$name"* && $url == *"a="* ]] || check "run play: the link names no machine or not output $name: $url"
    want=$(( $(date +%s) + ${expires%[sh]} * $([ "${expires: -1}" = h ] && echo 3600 || echo 1) )) e=$(expiry "$url")
    [ -n "$e" ] && [ $(( e - want )) -le 10 ] && [ $(( want - e )) -le 10 ] || check "run play --expires $expires: the token expires at ${e:-?}, not about $want"
  done
fi
}

machines() {
step machines
# The same wheels again, an update to what the machine runs: it reports their versions.
if [ -n "$CAND_RT" ] && [ -n "$CAND_TFS" ]; then
  cozy --json machine update local --runtime "$CAND_RT" --tensorfs "$CAND_TFS" -y > "$OUT/update-again.json" || check "machine update local --runtime --tensorfs failed"
  rt=$(jq -r 'select(.machine) | .runtime' "$OUT/update-again.json" | tail -1 || true)
  tfs=$(jq -r 'select(.machine) | .tensorfs' "$OUT/update-again.json" | tail -1 || true)
  cozy --json machine show local | J --arg rt "$rt" --arg tfs "$tfs" '$rt != "" and .status.runtime == $rt and .status.tensorfs == $tfs' \
    || check "machine update --runtime --tensorfs reported Runtime '$rt', TensorFS '$tfs': the machine reports other versions"
fi
cozy --json machine list > "$OUT/machines.json" && J '.machines | any(.name == "local")' "$OUT/machines.json" || check "machine list: no local"
cozy --json machine show local > "$OUT/local.json" && J '.state == "ready" and (.address | length > 0)' "$OUT/local.json" || check "machine show local: not ready"

step "machine logs"
cozy machine logs local --lines 5 > "$OUT/log-5.txt" || check "machine logs failed"
[ "$(wc -l < "$OUT/log-5.txt")" -le 5 ] && [ -s "$OUT/log-5.txt" ] || check "machine logs --lines 5: $(wc -l < "$OUT/log-5.txt") lines"
cozy machine logs local --since 1h > "$OUT/log-since.txt" && [ -s "$OUT/log-since.txt" ] || check "machine logs --since 1h: nothing from the last hour"
cozy machine logs local calcifer --lines 5 > "$OUT/log-calcifer.txt" && [ -s "$OUT/log-calcifer.txt" ] || check "machine logs local calcifer: no Calcifer log"
code=0; timeout 8 "$COZY" machine logs local --follow > "$OUT/log-follow.txt" 2>&1 || code=$?
printf 'machine\x1flogs\x1flocal\x1f--follow\x1f\x1e' >> "$OUT/argv.log"  # timeout runs the binary itself
[ "$code" = 124 ] && [ -s "$OUT/log-follow.txt" ] || check "machine logs --follow: exited $code instead of following"

step "this computer's machine, by its address and key"
# Another computer adds it (release-gate session multi-machine): here, by its own address, it is refused.
addr=$(jq -r .address "$OUT/local.json") || true; fp=$(jq -r .fingerprint "$OUT/local.json") || true
code=0; cozy --json machine add e2e-self "$addr" --fingerprint "$fp" > "$OUT/added.json" 2> "$OUT/added.err" || code=$?
[ "$code" = 1 ] && J '.error.code == "machine_is_local"' "$OUT/added.json" || check "machine add of this computer's own machine: exit $code, not refused: $(head -c 300 "$OUT/added.json")"
! cozy --json machine list | J '.machines | any(.name == "e2e-self")' || check "machine add: this computer's own machine is listed again as e2e-self"
J '.controller_key | test("^[A-Za-z0-9_-]{43}$")' "$OUT/local.json" || check "machine show local: no controller key: $(head -c 300 "$OUT/local.json")"
code=0; cozy --json machine ssh local > "$OUT/ssh-local.json" 2> "$OUT/ssh-local.err" || code=$?
[ "$code" = 2 ] && J '.error.code == "machine_ssh_none"' "$OUT/ssh-local.json" || check "machine ssh local: exit $code, not refused as no rental: $(head -c 300 "$OUT/ssh-local.json")"
openssl genpkey -algorithm ed25519 -out "$OUT/controller.pem" 2> /dev/null || check "openssl could not make an Ed25519 key"
key=$(openssl pkey -in "$OUT/controller.pem" -pubout -outform DER | tail -c 32 | base64 | tr '+/' '-_' | tr -d '=\n') || true
cozy --json machine authorize "$key" > "$OUT/authorize.json" && J '.changed == true' "$OUT/authorize.json" || check "machine authorize: the key was not added"
grep -qx "$key" "$HOME/.cozy/keys/authorized" || check "machine authorize: the key is not in keys/authorized"
# Authorized, it listens on every interface and says where other computers add it.
J '.machine.reach | test(":[0-9]+$")' "$OUT/authorize.json" || check "machine authorize: no address to add it at: $(jq -c .machine.reach "$OUT/authorize.json")"
cozy --json machine authorize "$key" | J '.changed == false' || check "machine authorize twice: added it again"
cozy --json machine authorize "$key" --remove > "$OUT/unauthorize.json" && J '.changed == true' "$OUT/unauthorize.json" || check "machine authorize --remove: the key was not removed"
! grep -qx "$key" "$HOME/.cozy/keys/authorized" || check "machine authorize --remove: the key is still in keys/authorized"
code=0; cozy --json machine end local > "$OUT/end-local.json" 2> "$OUT/end-local.err" || code=$?
[ "$code" = 1 ] && J '.error.code == "machine_local"' "$OUT/end-local.json" || check "machine end local: exit $code, not refused: $(head -c 300 "$OUT/end-local.json")"

step "doctor --on"
cozy --json doctor --on local > "$OUT/doctor-local.json" || check "cozy doctor --on local found a problem"
}

completions() {
step completion
shells=(bash)
[ "$RUNNER_OS" != Linux ] || { sudo apt-get install -y -qq zsh fish > /dev/null 2>&1 || true; }
command -v zsh > /dev/null && shells+=(zsh)
command -v fish > /dev/null && shells+=(fish)
command -v pwsh > /dev/null && shells+=(powershell)
for sh in "${shells[@]}"; do cozy completion "$sh" > "$OUT/completion.$sh" && [ -s "$OUT/completion.$sh" ] || check "completion $sh: no script"; done
[ ! -s "$OUT/completion.powershell" ] || cp "$OUT/completion.powershell" "$OUT/completion.ps1"  # pwsh dot-sources .ps1 only
bash -c ". '$OUT/completion.bash'; complete -p cozy" > /dev/null || check "completion bash: does not complete cozy"
[ ! -s "$OUT/completion.zsh" ] || zsh -c "autoload -U compinit; compinit -u; . '$OUT/completion.zsh'; (( \$+functions[_cozy] ))" \
  || check "completion zsh: no _cozy"
[ ! -s "$OUT/completion.fish" ] || fish -c "source '$OUT/completion.fish'; complete -C 'cozy mach'" | grep -q machine \
  || check "completion fish: cozy mach does not complete to machine"
[ ! -s "$OUT/completion.powershell" ] || PATH="$(dirname "$COZY"):$PATH" pwsh -NoProfile -c ". '$OUT/completion.ps1'; (TabExpansion2 'cozy mach' 9).CompletionMatches.CompletionText" | grep -q machine \
  || check "completion powershell: cozy mach does not complete to machine"
echo "completion: ${shells[*]}"
}

# run.sh's last step before removal (after the steps that stop Calcifer): this computer's cozy (the candidate) updates itself to the newest release, as a person's `cozy update`
# does: the binary is that release's and Calcifer restarted on it. (An older release's own update is its code,
# frozen: run 40084450876 found 0.11.14's update waiting on Calcifer for good while an authorized machine served.)
update_to_newest() {
cp "$COZY" "$OUT/cozy-under-test"
unset COZY_DOWNLOAD_URL  # GitHub's newest release, not --release's files
newest=$(curl -fsSL ${GH_TOKEN:+-H "Authorization: Bearer $GH_TOKEN"} "https://api.github.com/repos/cozy-creator/cozy-creator/releases?per_page=30" \
  | jq -r '[.[] | select(.prerelease | not) | .tag_name | select(test("^v[0-9]+\\.[0-9]+\\.[0-9]+$"))][0] // empty') || true
[ -n "$newest" ] || { check "update: no release to update to"; return; }
t0=$SECONDS
cozy --json update "${newest#v}" > "$OUT/update.json" || check "update ${newest#v} failed: $(head -c 300 "$OUT/update.json")"
echo "update ${newest#v}: $((SECONDS - t0)) s"
[ "$(cozy --version)" = "cozy ${newest#v}" ] || check "update: cozy is $(cozy --version), not ${newest#v}"
J '.calcifer == "restarted"' "$OUT/update.json" || check "update: Calcifer was not restarted ($(jq -c .calcifer "$OUT/update.json"))"
cozy --json status | J --arg v "${newest#v}" '.calcifer.version == $v' || check "update: Calcifer does not run ${newest#v}"
}
