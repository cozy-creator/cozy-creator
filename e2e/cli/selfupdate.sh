# e2e/cli/selfupdate.sh: cozy updates itself for a package that needs a newer cozy (cl-978).

# self_update: this computer's cozy becomes the change built as a release of the line before the Latest (E2E_COZY_BEFORE, 0.24.99:
# build.sh cozy-before), which runs no Runtime from 0.46. Its Hub (e2e.yaml) first pins 0.24's software (Runtime 0.45.2, TensorFS
# 0.24.2), so `cozy machine update local` naming nothing puts local there, held by nothing; then (rehub.sh) 0.25.0's: Runtime 0.46.0,
# TensorFS 0.25.1, cozy 0.25.0. isolated.sh's E2E_NEXT needs cozy-runtime>=0.46.0, which only a cozy of 0.25's line runs.
#  - held by hand: its install is refused runtime_held naming `cozy machine update local`, and cozy moves nothing.
#  - its install folder read-only: refused once, naming `sudo cozy update`, and cozy moves nothing.
#  - then its install says "updating cozy 0.24.99 → <Latest> for …" once, cozy and Calcifer become the Latest, and the install goes
#    on as it, local moving to its minor. 0.25.x predates this: its install moves no Runtime, so there the install may stop at the
#    floor; a later Latest's must complete.
#  - a run of it, in a project of the Latest's, completes on a Runtime that runs it.
# The release is GitHub's Latest (signed by its release workflow, which no test build is): `cozy update`'s own source.
self_update() {
step "cozy updates itself for a package that needs a newer cozy (cl-978)"
[ "${RUNNER_OS:-}" = Linux ] && [ -n "${COZY_TEST_HUB_PID_FILE:-}" ] || { echo "self update: Linux, on an isolated Hub it restarts"; return; }
[ -n "${E2E_COZY_BEFORE:-}" ] && [ -f "$E2E_COZY_BEFORE" ] && [ -n "${E2E_NEXT:-}" ] \
  || { check "self update: no cozy-before (${E2E_COZY_BEFORE:-unset}) or e2e-next (${E2E_NEXT:-unset}: ${E2E_NEXT_REFUSED:-})"; return; }
local latest before code=0 rehub=$here/../skew/rehub.sh old=(--set software.runtime=0.45.2 --set software.tensorfs=0.24.2 --set software.cozy=)
local new=(--set software.runtime=0.46.0 --set software.tensorfs=0.25.1 --set software.cozy=0.25.0)
latest=$(curl -fsSL --retry 3 https://github.com/cozy-creator/cozy-creator/releases/latest/download/dist-manifest.json | jq -r '.releases[] | select(.app_name == "cozy") | .app_version')
[ -n "$latest" ] || { check "self update: GitHub names no Latest cozy"; return; }
install -m 755 "$E2E_COZY_BEFORE" "$COZY.new" && mv -f "$COZY.new" "$COZY" || { check "self update: cozy-before not installed"; return; }
before=$(cozy --version | awk '{print $2}')
[ "$before" = 0.24.99 ] || { check "self update: cozy-before says $before"; return; }
cozy --json restart > "$OUT/su-restart.json" 2>&1 || { check "self update: cozy restart: $(tail -c 300 "$OUT/su-restart.json")"; return; }
# On 0.24's software, held by nothing: the Hub's pin, named by nothing.
cozy --json machine update local -y > "$OUT/su-old.json" 2>&1 || { check "self update: local onto 0.24's software: $(tail -c 300 "$OUT/su-old.json")"; return; }
machine_ready
jq -e '.status.runtime == "0.45.2" and .status.tensorfs == "0.24.2" and (.status.version | startswith("0.24.99"))' "$OUT/machine.json" > /dev/null \
  || { check "self update: local runs $(jq -c '.status | {runtime, tensorfs, version}' "$OUT/machine.json"), not 0.24.99 on 0.45.2/0.24.2"; return; }
su_hub "${new[@]}" || return

# Held by hand: refused, nothing moved.
cozy --json machine update local --runtime 0.45.2 -y > "$OUT/su-hold.json" 2>&1 || { check "self update: hold local: $(tail -c 300 "$OUT/su-hold.json")"; return; }
machine_ready
su_install held || code=$?
[ "$code" != 0 ] && grep -q '"runtime_held"' "$OUT/su-held.out" && grep -q 'cozy machine update local' "$OUT/su-held.out" && ! grep -q 'updating cozy' "$OUT/su-held.out" \
  || check "self update: held local wasn't refused runtime_held naming cozy machine update local: exit $code $(tail -c 400 "$OUT/su-held.out")"
[ "$(cozy --version)" = "cozy $before" ] || check "self update: held, cozy moved to $(cozy --version)"
# Let go: named by nothing on 0.24's pin, which it runs.
su_hub "${old[@]}" || return
cozy --json machine update local -y > "$OUT/su-unhold.json" 2>&1 || { check "self update: let local go: $(tail -c 300 "$OUT/su-unhold.json")"; return; }
machine_ready
su_hub "${new[@]}" || return

# Its folder read-only: refused once, naming the command, nothing moved.
chmod a-w "$(dirname "$COZY")"
code=0; su_install unwritable || code=$?
chmod u+w "$(dirname "$COZY")"
[ "$code" != 0 ] && grep -q '"cozy_unwritable"' "$OUT/su-unwritable.out" && grep -q 'sudo cozy update' "$OUT/su-unwritable.out" \
  || check "self update: a read-only install wasn't refused cozy_unwritable naming sudo cozy update: exit $code $(tail -c 400 "$OUT/su-unwritable.out")"
[ "$(cozy --version)" = "cozy $before" ] || check "self update: read-only, cozy moved to $(cozy --version)"
# A new Calcifer asks again (one update per Calcifer).
cozy --json restart > "$OUT/su-restart-2.json" 2>&1 || { check "self update: cozy restart: $(tail -c 300 "$OUT/su-restart-2.json")"; return; }

# The update, said once (as a person reads it, not --json); the install goes on as the Latest.
code=0; su_install updated --human || code=$?
[ "$(grep -c 'updating cozy' "$OUT/su-updated.out")" = 1 ] && grep -q "updating cozy $before → $latest for $E2E_NEXT 0.1.0" "$OUT/su-updated.out" \
  || check "self update: the install didn't say once \"updating cozy $before → $latest for $E2E_NEXT 0.1.0\": $(grep 'updating cozy\|cozy_' "$OUT/su-updated.out" | head -3)"
[ "$(cozy --version)" = "cozy $latest" ] || { check "self update: cozy is $(cozy --version), not $latest: $(tail -c 400 "$OUT/su-updated.out")"; return; }
case $latest in
  0.25.*) [ "$code" = 0 ] || grep -q 'runtime_floor_unmet' "$OUT/su-updated.out" || check "self update: on $latest the install ended $code, not at its floor: $(tail -c 400 "$OUT/su-updated.out")" ;;
  *) [ "$code" = 0 ] || check "self update: on $latest the install didn't complete: exit $code $(tail -c 400 "$OUT/su-updated.out")" ;;
esac
jq -e --arg v "$latest" '.version == $v' "$HOME/.cozy/calcifer/calcifer.lock.holder" > /dev/null \
  || check "self update: Calcifer runs $(jq -r .version "$HOME/.cozy/calcifer/calcifer.lock.holder"), not $latest"
machine_ready
jq -e --arg m "${latest%.*}." '.status.version | startswith($m)' "$OUT/machine.json" > /dev/null \
  || check "self update: local runs cozy $(jq -r .status.version "$OUT/machine.json"), not $latest's minor"

# A run of it, in a project of the Latest's: it completes, local moved to a Runtime that runs it.
local P=$OUT/selfupdate-project
mkdir -p "$P" && in_project "$P" --json new --unencrypted > "$OUT/su-new.json" 2>&1 || { check "self update: cozy new: $(tail -c 300 "$OUT/su-new.json")"; return; }
RUN_PROJECT=$P cell su-run "$E2E_NEXT/echo" text=updated || true
machine_ready
jq -e '.status.runtime | test("^0\\.(4[6-9]|[5-9][0-9])\\.")' "$OUT/machine.json" > /dev/null \
  || check "self update: after its run local runs Runtime $(jq -r .status.runtime "$OUT/machine.json"), not 0.46 or newer"
}

# su_hub <--set key=value>...: the isolated Hub restarted with this software, and Calcifer with it, which reads it afresh.
su_hub() {
  bash "$here/../skew/rehub.sh" "$@" > "$OUT/su-rehub.log" 2>&1 || { check "self update: the Hub with $*: $(tail -c 300 "$OUT/su-rehub.log")"; return 1; }
  cozy --json restart > "$OUT/su-restart-hub.json" 2>&1 || { check "self update: cozy restart: $(tail -c 300 "$OUT/su-restart-hub.json")"; return 1; }
}

# su_install <name> [--human]: `cozy [--json] package install E2E_NEXT --on local`, all it said in $OUT/su-<name>.out; its exit.
su_install() {
  local json=--json
  [ "${2:-}" != --human ] || json=
  timeout 1800 "$COZY" $json package install "$E2E_NEXT" --on local > "$OUT/su-$1.out" 2>&1 < /dev/null
}
