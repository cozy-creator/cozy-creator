# e2e/cli/selfupdate.sh: `cozy update`, both ways: the Latest updating to the candidate, and cozy updating itself for a package that
# needs a newer cozy (cl-978).

SU_ARCHIVE=cozy-x86_64-unknown-linux-gnu.tar.gz SU_LATEST='' SU_STORE='self_forward measured no store version' FW_HOME='' FW_TOOK=0
# The Hub's software: the line before the Latest's, and the Latest's (its cozy, Runtime and TensorFS; su_latest).
SU_OLD=() SU_NEW=() SU_PREVIOUS_RUNTIME=''

# self_forward: GitHub's Latest (su_latest) updates to the candidate, as a person's would. Its Linux archive, checked against
# its release's sha256.sum, gets a home of its own (the candidate's is set aside: Calcifer's store goes one way), the Latest's
# software, and a project with a completed run. A signed candidate (publish_run's release) then comes by the Latest's own `cozy update
# <version>`, which checks its sigstore bundle. An RC build has no signed archive, so it goes in place as that update puts it after
# the check (update.rs at v0.25.0): the binary kept as .cozy-previous, the candidate renamed over it, Calcifer restarted by the
# service the Latest defined, and the Latest's client waiting for it. Then Calcifer runs the candidate, its first start on that home
# ended within 300 s, the store is at the candidate's version (its old runs cut where the version moves: cl-1003's hard cut; else
# kept), the project opens, and its next run on
# local as the Latest left it moves local to the candidate's own Runtime and completes (cl-1055); then a new one completes on the
# candidate set. A failure before the candidate runs there puts the candidate's own home back.
self_forward() {
step "the Latest updates to the candidate (cozy update)"
cd "$OUT" || return
forward || { [ -z "$FW_HOME" ] || fw_back; }
cd "$RUN_PROJECT"
}

forward() {
[ "${RUNNER_OS:-}" = Linux ] && [ -n "${COZY_TEST_HUB_PID_FILE:-}" ] || { echo "forward update: Linux, on an isolated Hub"; return 0; }
[ -n "${E2E_NEXT:-}" ] || { check "forward update: no e2e-next: ${E2E_NEXT_REFUSED:-}"; return 1; }
su_latest || { check "forward update: GitHub names no Latest cozy"; return 1; }
local latest=$SU_LATEST candidate dir=$RUNNER_TEMP/forward-latest bin bindir cand_store latest_store id code=0 up=() P=$OUT/forward-project
candidate=$(cozy --version | awk '{print $2}')
if [ "$(printf '%s\n%s\n' "${candidate%%[-+]*}" "$latest" | sort -V | tail -1)" = "$latest" ]; then
  SU_STORE=''
  echo "forward update: skipped: the candidate $candidate is not newer than the Latest $latest"
  return 0
fi
rm -rf "$dir" && mkdir -p "$dir/x" \
  && curl -fsSL --retry 5 --retry-all-errors -o "$dir/$SU_ARCHIVE" "https://github.com/cozy-creator/cozy-creator/releases/download/v$latest/$SU_ARCHIVE" \
  && curl -fsSL --retry 5 --retry-all-errors -o "$dir/sha256.sum" "https://github.com/cozy-creator/cozy-creator/releases/download/v$latest/sha256.sum" \
  && (cd "$dir" && awk -v f="$SU_ARCHIVE" '{ n = $2; sub(/^\*/, "", n) } n == f' sha256.sum | sha256sum -c --strict -) \
  && tar -xzf "$dir/$SU_ARCHIVE" -C "$dir/x" --no-same-owner && bin=$(find "$dir/x" -type f -name cozy -print -quit) && [ -n "$bin" ] \
  && [ "$("$bin" --version)" = "cozy $latest" ] || { check "forward update: cozy $latest's $SU_ARCHIVE didn't check against its sha256.sum or run"; return 1; }

# The candidate's home set aside, the Latest's own on the Latest's software, as isolated.sh's Hub left it (config.toml's hub_url).
cand_store=$(su_store)
fw_stop
bash "$here/../skew/rehub.sh" "${SU_NEW[@]}" > "$OUT/fw-rehub.log" 2>&1 || { check "forward update: the Hub with ${SU_NEW[*]}: $(tail -c 300 "$OUT/fw-rehub.log")"; return 1; }
FW_TFS=${TENSORFS_HOME:-$HOME/.cache/tensorfs}
rm -rf ~/.cozy-candidate "$FW_TFS.candidate" && mv ~/.cozy ~/.cozy-candidate && FW_HOME=$HOME/.cozy-candidate \
  && { [ ! -e "$FW_TFS" ] || mv "$FW_TFS" "$FW_TFS.candidate"; } && mkdir -p "$(dirname "$COZY")" \
  && cp "$FW_HOME/config.toml" ~/.cozy/config.toml && install -m 755 "$bin" "$COZY" || { check "forward update: cozy $latest's home not made"; return 1; }
(machine_ready) || return 1
cozy --json machine update local -y > "$OUT/fw-software.json" 2>&1 || { check "forward update: cozy $latest's local onto its software: $(tail -c 300 "$OUT/fw-software.json")"; return 1; }
(machine_ready) || return 1
echo "forward update: cozy $latest runs local on $(jq -c '.status | {version, runtime, tensorfs}' "$OUT/machine.json")"
latest_store=$(su_store)
[ -n "$latest_store" ] && [ -n "$cand_store" ] || { check "forward update: no store version: the Latest's ${latest_store:-none}, the candidate's ${cand_store:-none}"; return 1; }
SU_STORE=''
[ "$latest_store" -ge "$cand_store" ] || SU_STORE="the Latest $latest reads Calcifer's store to version $latest_store, the candidate's is $cand_store"
mkdir -p "$P" && in_project "$P" --json new --unencrypted > "$OUT/fw-new.json" 2>&1 || { check "forward update: cozy $latest's cozy new: $(tail -c 300 "$OUT/fw-new.json")"; return 1; }
RUN_PROJECT=$P cell fw-before "$E2E_NEXT/echo" text=before || return 1
id=$(run_get "$OUT" fw-before .id)

bindir=$(dirname "$COZY")
if [ -f "${RELEASE:-}/dist-manifest.json" ]; then
  # The release's files, served as COZY_DOWNLOAD_URL (lib.sh's serve_release).
  fw_timed update cozy --json update "$candidate" || code=$?
  [ "$code" = 0 ] && tail -n 1 "$OUT/fw-update.out" | jq -e --arg v "$candidate" '.to == $v and .calcifer == "restarted" and (.signer | contains("/.github/workflows/cozy-publish.yaml@"))' > /dev/null \
    || { [ "$code" = 124 ] || check "forward update: cozy $latest's cozy update $candidate: exit $code $(tail -c 400 "$OUT/fw-update.out")"; return 1; }
  echo "forward update: cozy update $candidate checked its signature: $(tail -n 1 "$OUT/fw-update.out" | jq -r .signer)"
else
  echo "forward update: ${RELEASE:+this publish run holds no dist-manifest.json and }an RC build has no signed archive: the candidate goes in place as cozy update puts it after its signature check, which runs only with publish_run"
  cp "$FW_HOME/bin/cozy" "$bindir/.cozy-update-e2e" && chmod 755 "$bindir/.cozy-update-e2e" \
    && [ "$("$bindir/.cozy-update-e2e" --version)" = "cozy $candidate" ] \
    && { ln -f "$COZY" "$bindir/.cozy-previous" 2> /dev/null || cp "$COZY" "$bindir/.cozy-previous"; } \
    && mv -f "$bindir/.cozy-update-e2e" "$COZY" || { check "forward update: the candidate not put in place"; return 1; }
  systemctl --user restart cozy-calcifer > "$OUT/fw-restart.log" 2>&1 || { check "forward update: Calcifer's service: $(tail -c 300 "$OUT/fw-restart.log")"; return 1; }
  fw_timed start "$bindir/.cozy-previous" --json machine ls || code=$?
  [ "$code" != 124 ] || return 1
  echo "forward update: cozy $latest's client reached Calcifer after its restart (exit $code): $(tail -c 200 "$OUT/fw-start.out")"
fi
jq -e --arg v "$candidate" '.version == $v' "$HOME/.cozy/calcifer/calcifer.lock.holder" > /dev/null 2>&1 \
  || { check "forward update: Calcifer runs $(jq -r .version "$HOME/.cozy/calcifer/calcifer.lock.holder" 2> /dev/null), not $candidate: $(tail -c 300 "$OUT/fw-start.out" "$OUT/fw-update.out" 2> /dev/null)"; return 1; }
FW_HOME=''
[ "$(cozy --version)" = "cozy $candidate" ] || check "forward update: cozy is $(cozy --version), not $candidate"
[ "$("$bindir/.cozy-previous" --version 2> /dev/null)" = "cozy $latest" ] || check "forward update: .cozy-previous isn't cozy $latest"
[ "$(su_store)" = "$cand_store" ] || check "forward update: Calcifer's store is at version $(su_store) after its first start, not the candidate's $cand_store"
echo "forward update: cozy $latest → $candidate: the update and Calcifer's first start on its home took $FW_TOOK s, its store version $latest_store → $(su_store)"

in_project "$P" --json status > "$OUT/fw-status.json" 2>&1 || check "forward update: cozy $latest's project doesn't open on $candidate: $(tail -c 300 "$OUT/fw-status.json")"
# Its run's history is cut where the store's version moves (cl-1003's hard cut: store.rs rebuilds 7 and 8 with their history
# cut), else kept: listed. (What a project's runs made stays in it: hard_cut.)
if [ "$latest_store" != "$cand_store" ]; then
  in_project "$P" --json run ls --limit 0 > "$OUT/fw-runs.json" 2>&1 && jq -e --arg id "$id" 'all(.runs[]; .id != $id)' "$OUT/fw-runs.json" > /dev/null \
    || check "forward update: cozy $latest's run $id is still listed on $candidate across the store's cut ($latest_store → $cand_store), or the list failed: $(tail -c 300 "$OUT/fw-runs.json")"
else
  in_project "$P" --json run ls --limit 0 > "$OUT/fw-runs.json" 2>&1 && jq -e --arg id "$id" 'any(.runs[]; .id == $id)' "$OUT/fw-runs.json" > /dev/null \
    || check "forward update: cozy $latest's run $id isn't listed on $candidate, though its store's version $cand_store is the Latest's (no cut), or the list failed: $(tail -c 300 "$OUT/fw-runs.json")"
fi
# The upgrade a person meets (cl-1055): local still on the Latest's TensorD and Runtime, below the candidate's minor, nothing moved by
# hand. The next run in the Latest's own project (nothing of it pending) moves local to the candidate's
# own Runtime first, saying so, completes, and its call's temporary file is in its project's scratch.
local moved_from temporary
(machine_ready) || return 0
moved_from=$(jq -r '.status.runtime' "$OUT/machine.json")
in_project "$P" run e2e-scratch/probe "marker=FW_MOVED_$RANDOM" --on local > "$OUT/fw-moved.out" 2> "$OUT/fw-moved.err" \
  || check "forward update: the next run in cozy $latest's project on local (Runtime $moved_from) failed on $candidate: $(tail -c 400 "$OUT/fw-moved.err")"
grep -q "Updating local.s Runtime from $moved_from to [^ ]* for this project" "$OUT/fw-moved.err" \
  || check "forward update: the run didn't say it upgrades local's Runtime $moved_from for this project: $(tail -c 400 "$OUT/fw-moved.err")"
temporary=$(awk '$1 == "temporary" { print $2 }' "$OUT/fw-moved.out" | tail -1)
case $temporary in
  "$(realpath "$P")"/.vault-machine/scratch/*) ;;
  *) check "forward update: the moved run's temporary file was ${temporary:-nowhere said}, not in its project's scratch" ;;
esac
(machine_ready) || return 0
echo "forward update: the next run in cozy $latest's project moved local from Runtime $moved_from to $(jq -r .status.runtime "$OUT/machine.json")"
# The candidate set on local, as the suite's start puts it: a dev build is no release for the update to move it to.
[ ! -f "${CAND_COZY:-}" ] || up+=(--tensord "$CAND_COZY")
[ -z "$CAND_TFS" ] || up+=(--tensorfs "$CAND_TFS")
[ -z "$CAND_RT" ] || up+=(--runtime "$CAND_RT")
[ ${#up[@]} = 0 ] || cozy --json machine update local "${up[@]}" -y > "$OUT/fw-machine.json" 2>&1 \
  || { check "forward update: local onto the candidates: $(tail -c 300 "$OUT/fw-machine.json")"; return 0; }
(machine_ready) || return 0
jq -e --arg v "${candidate%%+*}" '.status.version | startswith($v)' "$OUT/machine.json" > /dev/null || check "forward update: local runs cozy $(jq -r .status.version "$OUT/machine.json"), not $candidate"
RUN_PROJECT=$P cell fw-after "$E2E_NEXT/echo" text=after || true
}

# fw_timed <name> <command...>: Calcifer's first start on the Latest's home, inside the command: its time and the store's version said
# every 15 s, a failure past 300 s (a 0.26.0 build once hung there 28 min). What it said in $OUT/fw-<name>.out and .err; FW_TOOK its
# seconds.
fw_timed() {
  local name=$1 pid began=$SECONDS said=15; shift
  FW_TOOK=0
  "$@" > "$OUT/fw-$name.out" 2> "$OUT/fw-$name.err" < /dev/null &
  pid=$!
  while kill -0 "$pid" 2> /dev/null; do
    sleep 1
    FW_TOOK=$((SECONDS - began))
    [ "$FW_TOOK" -lt "$said" ] || { echo "forward update: $name: $FW_TOOK s, Calcifer's store at version $(su_store)"; said=$((said + 15)); }
    [ "$FW_TOOK" -ge 300 ] || continue
    kill "$pid" 2> /dev/null; wait "$pid" 2> /dev/null
    check "forward update: Calcifer's first start on cozy $SU_LATEST's home didn't end in 300 s, its store at version $(su_store): $(tail -c 300 "$OUT/fw-$name.out")"
    return 124
  done
  wait "$pid"
}

# fw_stop: Calcifer, the local machine and its executors ended, as a service manager ends them.
fw_stop() {
  local pid unit
  pid=$(jq -r '.pid // empty' "$HOME/.cozy/calcifer/calcifer.lock.holder" 2> /dev/null) || pid=
  systemctl --user stop cozy-calcifer > /dev/null 2>&1 || true
  [ -z "$pid" ] || kill "$pid" 2> /dev/null || true
  for _ in $(seq 30); do { [ -z "$pid" ] || ! kill -0 "$pid" 2> /dev/null; } && break; sleep 1; done
  [ -z "$pid" ] || kill -9 "$pid" 2> /dev/null || true
  machine_stop > /dev/null 2>&1 || true
  for unit in $(systemctl --user list-units --type=scope --no-legend --plain 2> /dev/null | awk '$1 ~ /^run-/ {print $1}'); do
    systemctl --user stop "$unit" > /dev/null 2>&1 || true
  done
}

# fw_back: the candidate's own home again; the Latest's set aside, its logs in $OUT/forward-logs.
fw_back() {
  fw_stop
  mkdir -p "$OUT/forward-logs" && (cd ~/.cozy && find . -name '*.log' -size -20M | tar -cf - -T - | (cd "$OUT/forward-logs" && tar -xf -)) || true
  { [ ! -e ~/.cozy ] || { rm -rf ~/.cozy-forward && mv ~/.cozy ~/.cozy-forward; }; } && mv "$FW_HOME" ~/.cozy || check "forward update: the candidate's home not put back"
  [ ! -e "$FW_TFS.candidate" ] || { rm -rf "$FW_TFS" && mv "$FW_TFS.candidate" "$FW_TFS"; }
  FW_HOME=''
  echo "forward update: the candidate's own home is back"
}

# su_latest: GitHub's Latest cozy in SU_LATEST and the Hub's software of its line in SU_NEW; the line before it, the software
# cozy-before runs, in SU_OLD and SU_PREVIOUS_RUNTIME. Each release's notes name its pair (e2e/releases.py), read at every run: a
# promotion moves them.
su_latest() {
  local said
  [ -n "$SU_LATEST" ] && return 0
  said=$(python3 -I "$here/../releases.py" 2> "$OUT/su-releases.err") || { echo "su_latest: $(cat "$OUT/su-releases.err")"; return 1; }
  eval "$said"
  SU_LATEST=$LATEST SU_PREVIOUS_RUNTIME=$PREVIOUS_RUNTIME
  SU_NEW=(--set "software.runtime=$LATEST_RUNTIME" --set "software.tensorfs=$LATEST_TENSORFS" --set "software.cozy=$LATEST")
  SU_OLD=(--set "software.runtime=$PREVIOUS_RUNTIME" --set "software.tensorfs=$PREVIOUS_TENSORFS" --set software.cozy=)
  [ -n "$SU_LATEST" ]
}

# su_store: the version of this home's Calcifer store (PRAGMA user_version), read only; empty with none.
su_store() {
  python3 -I -c 'import sqlite3, sys; print(sqlite3.connect(f"file:{sys.argv[1]}?mode=ro", uri=True).execute("PRAGMA user_version").fetchone()[0])' \
    "$HOME/.cozy/calcifer/state.db" 2> /dev/null || true
}

# self_update: this computer's cozy becomes the change built as a release of the line before the Latest (E2E_COZY_BEFORE, build.sh
# cozy-before: 0.25.99 while 0.26.0 is Latest), which runs none of the Latest's Runtime minor. Its Hub first pins that line's
# software (its newest release's), so `cozy machine update local` naming nothing puts local there, held by nothing; then the
# Latest's. isolated.sh's E2E_NEXT needs the Latest's Runtime minor, which only a cozy of the Latest's line or later runs.
#  - first, the candidate itself (above the Latest): `cozy update` naming nothing is refused cozy_downgrade, and stays.
#  - held by hand: its install is refused runtime_held naming `cozy machine update local`, and cozy moves nothing.
#  - its install folder read-only: refused once, naming `sudo cozy update`, and cozy moves nothing.
#  - then, once the Latest reads the candidate's store (self_forward measures both; until then a notice says so), its install says
#    "Updating cozy from <cozy-before> to <Latest> for …" once, cozy and Calcifer become the Latest, and the install
#    completes as it, local moving to its minor; and a run of it, in a project of the Latest's, completes on a Runtime that runs it.
# The release is GitHub's Latest (signed by its release workflow, which no test build is): `cozy update`'s own source.
self_update() {
step "cozy updates itself for a package that needs a newer cozy (cl-978)"
[ "${RUNNER_OS:-}" = Linux ] && [ -n "${COZY_TEST_HUB_PID_FILE:-}" ] || { echo "self update: Linux, on an isolated Hub it restarts"; return; }
[ -n "${E2E_COZY_BEFORE:-}" ] && [ -f "$E2E_COZY_BEFORE" ] && [ -n "${E2E_NEXT:-}" ] \
  || { check "self update: no cozy-before (${E2E_COZY_BEFORE:-unset}) or e2e-next (${E2E_NEXT:-unset}: ${E2E_NEXT_REFUSED:-})"; return; }
su_latest || { check "self update: GitHub names no Latest cozy"; return; }
local latest=$SU_LATEST before code=0
# Never back unnamed (xs-196): a candidate that sorts above the Latest (the RC's 0.x.0-dev) is refused cozy_downgrade, naming the
# version, and stays. GitHub's Latest, not a publish run's own release (COZY_DOWNLOAD_URL).
local candidate; candidate=$(cozy --version | awk '{print $2}')
if [ "${candidate%%[-+]*}" != "$latest" ] && [ "$(printf '%s\n%s\n' "${candidate%%[-+]*}" "$latest" | sort -V | tail -1)" != "$latest" ]; then
  code=0; (unset COZY_DOWNLOAD_URL; cozy --json update) > "$OUT/su-downgrade.out" 2>&1 || code=$?
  [ "$code" != 0 ] && grep -q '"cozy_downgrade"' "$OUT/su-downgrade.out" && grep -q "cozy update $latest" "$OUT/su-downgrade.out" \
    || check "self update: cozy update from $candidate to the older Latest $latest wasn't refused cozy_downgrade: exit $code $(tail -c 400 "$OUT/su-downgrade.out")"
  [ "$(cozy --version)" = "cozy $candidate" ] || check "self update: an unnamed downgrade moved cozy to $(cozy --version)"
fi
install -m 755 "$E2E_COZY_BEFORE" "$COZY.new" && mv -f "$COZY.new" "$COZY" || { check "self update: cozy-before not installed"; return; }
before=$(cozy --version | awk '{print $2}')
[ "${before%.*}" = "${PREVIOUS%.*}" ] && [ "$(printf '%s\n%s\n' "$before" "$latest" | sort -V | tail -1)" = "$latest" ] \
  || { check "self update: cozy-before says $before, not of $PREVIOUS's line below the Latest $latest"; return; }
# On the line before's software, held by nothing: the Hub's pin, named by nothing.
su_hub "${SU_OLD[@]}" || return
cozy --json machine update local -y > "$OUT/su-old.json" 2>&1 || { check "self update: local onto $PREVIOUS's software: $(tail -c 300 "$OUT/su-old.json")"; return; }
machine_ready
jq -e --arg r "$PREVIOUS_RUNTIME" --arg t "$PREVIOUS_TENSORFS" --arg v "$before" \
  '.status.runtime == $r and .status.tensorfs == $t and (.status.version | startswith($v))' "$OUT/machine.json" > /dev/null \
  || { check "self update: local runs $(jq -c '.status | {runtime, tensorfs, version}' "$OUT/machine.json"), not $before on $PREVIOUS_RUNTIME/$PREVIOUS_TENSORFS"; return; }
su_hub "${SU_NEW[@]}" || return

# Held by hand: refused, nothing moved.
cozy --json machine update local --runtime "$PREVIOUS_RUNTIME" -y > "$OUT/su-hold.json" 2>&1 || { check "self update: hold local: $(tail -c 300 "$OUT/su-hold.json")"; return; }
machine_ready
code=0; su_install held || code=$?
[ "$code" != 0 ] && grep -q '"runtime_held"' "$OUT/su-held.out" && grep -q 'cozy machine update local' "$OUT/su-held.out" && ! grep -q 'Updating cozy' "$OUT/su-held.out" \
  || check "self update: held local wasn't refused runtime_held naming cozy machine update local: exit $code $(tail -c 400 "$OUT/su-held.out")"
[ "$(cozy --version)" = "cozy $before" ] || check "self update: held, cozy moved to $(cozy --version)"
# Let go: named by nothing on the line before's pin, which it runs.
su_hub "${SU_OLD[@]}" || return
cozy --json machine update local -y > "$OUT/su-unhold.json" 2>&1 || { check "self update: let local go: $(tail -c 300 "$OUT/su-unhold.json")"; return; }
machine_ready
su_hub "${SU_NEW[@]}" || return

# Its folder read-only: refused once, naming the command, nothing moved.
chmod a-w "$(dirname "$COZY")"
code=0; su_install unwritable || code=$?
chmod u+w "$(dirname "$COZY")"
[ "$code" != 0 ] && grep -q '"cozy_unwritable"' "$OUT/su-unwritable.out" && grep -q 'sudo cozy update' "$OUT/su-unwritable.out" \
  || check "self update: a read-only install wasn't refused cozy_unwritable naming sudo cozy update: exit $code $(tail -c 400 "$OUT/su-unwritable.out")"
[ "$(cozy --version)" = "cozy $before" ] || check "self update: read-only, cozy moved to $(cozy --version)"
[ -z "$SU_STORE" ] || { echo "::notice::self update: the update to the Latest and its run skipped: $SU_STORE (they run once a Latest reads it)"; return; }
# A new Calcifer asks again (one update per Calcifer).
cozy --json restart > "$OUT/su-restart-2.json" 2>&1 || { check "self update: cozy restart: $(tail -c 300 "$OUT/su-restart-2.json")"; return; }

# The update, said once (as a person reads it, not --json); the install goes on as the Latest.
code=0; su_install updated --human || code=$?
[ "$(grep -c 'Updating cozy' "$OUT/su-updated.out")" = 1 ] && grep -q "Updating cozy from $before to $latest for $E2E_NEXT 0.1.0" "$OUT/su-updated.out" \
  || check "self update: the install didn't say once \"Updating cozy from $before to $latest for $E2E_NEXT 0.1.0\": $(grep 'Updating cozy\|cozy_' "$OUT/su-updated.out" | head -3)"
[ "$(cozy --version)" = "cozy $latest" ] || { check "self update: cozy is $(cozy --version), not $latest: $(tail -c 400 "$OUT/su-updated.out")"; return; }
[ "$code" = 0 ] || check "self update: on $latest the install didn't complete: exit $code $(tail -c 400 "$OUT/su-updated.out")"
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
local runtime; runtime=$(jq -r .status.runtime "$OUT/machine.json")
[ "$(printf '%s\n%s\n' "${runtime%%[+]*}" "${LATEST_RUNTIME%.*}" | sort -V | tail -1)" = "${runtime%%[+]*}" ] \
  || check "self update: after its run local runs Runtime $runtime, not ${LATEST_RUNTIME%.*} (the Latest's) or newer"
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

# update_offer (cl-1069; Paul 11-16: "on the user's machine, when there is a new update available, and they start cozy creator, we
# should ask them if they want to update, rather than forcibly updating them … that gives them the ability to remain on the older
# version"): the change built as a release (cozy-before: 0.25.99 while 0.26.0 is Latest) at a terminal (script's pty), GitHub's Latest served as
# COZY_DOWNLOAD_URL, in a home of its own whose `calcifer` is a file: each command ends at once after the question
# (calcifer_start_failed), and the suite's Calcifer and machine are never touched.
#  - asked "cozy <Latest> is out (this is <cozy-before>). Update now? [Y/n]"; `n` keeps it, the command goes on, the next isn't asked;
#  - `--json`, or stdin not a terminal: nothing asked;
#  - `y`: cozy becomes the Latest, its signed archive checked as `cozy update` checks it, and the command goes on.
OF_HOME='' OF_URL=''
update_offer() {
step "a newer release is offered at a terminal, never forced (cl-1069)"
[ "${RUNNER_OS:-}" = Linux ] || { echo "update offer: Linux"; return; }
[ -n "${E2E_COZY_BEFORE:-}" ] && [ -f "$E2E_COZY_BEFORE" ] || { check "update offer: no cozy-before (${E2E_COZY_BEFORE:-unset})"; return; }
su_latest || { check "update offer: GitHub names no Latest cozy"; return; }
local latest=$SU_LATEST rel=$RUNNER_TEMP/offer-release port=$((20000 + RANDOM % 20000)) server f before
before=$("$E2E_COZY_BEFORE" --version | awk '{print $2}')
local asked="cozy $latest is out (this is $before). Update now? [Y/n]"
rm -rf "$rel" && mkdir -p "$rel" || { check "update offer: $rel not made"; return; }
for f in dist-manifest.json "$SU_ARCHIVE" "$SU_ARCHIVE.sigstore.jsonl"; do
  curl -fsSL --retry 5 --retry-all-errors -o "$rel/$f" "https://github.com/cozy-creator/cozy-creator/releases/download/v$latest/$f" \
    || { check "update offer: cozy $latest's $f not fetched"; return; }
done
(cd "$rel" && exec python3 -m http.server "$port" --bind 127.0.0.1 > "$OUT/offer-server.log" 2>&1) &
server=$! OF_URL=http://127.0.0.1:$port OF_HOME=$RUNNER_TEMP/offer-home
for _ in $(seq 30); do curl -fsS -o /dev/null "$OF_URL/dist-manifest.json" && break; sleep 1; done
of_fresh || { kill "$server" 2> /dev/null || true; return; }

# `n`: kept, said, and the command goes on; not asked again.
of_run declined 'n\n' "$OF_HOME/.cozy/bin/cozy whoami"
grep -qF "$asked" "$OUT/offer-declined.out" || check "update offer: a terminal wasn't asked \"$asked\": $(tail -c 300 "$OUT/offer-declined.out")"
grep -qF "staying on cozy $before" "$OUT/offer-declined.out" || check "update offer: declined, it didn't say it stays on $before"
grep -qF 'error[calcifer_start_failed]' "$OUT/offer-declined.out" || check "update offer: declined, the command didn't go on"
[ "$(of_version)" = "cozy $before" ] || check "update offer: declined, cozy is $(of_version)"
of_run again 'n\n' "$OF_HOME/.cozy/bin/cozy whoami"
! grep -qF 'is out' "$OUT/offer-again.out" || check "update offer: declined $latest, the next command was asked again"
grep -qF 'error[calcifer_start_failed]' "$OUT/offer-again.out" || check "update offer: the next command didn't run: $(tail -c 300 "$OUT/offer-again.out")"

# No one to ask: nothing asked.
of_fresh || { kill "$server" 2> /dev/null || true; return; }
of_run json 'n\n' "$OF_HOME/.cozy/bin/cozy --json whoami"
! grep -qF 'is out' "$OUT/offer-json.out" || check "update offer: --json was asked"
of_run stdin 'n\n' "$OF_HOME/.cozy/bin/cozy whoami < /dev/null"
! grep -qF 'is out' "$OUT/offer-stdin.out" || check "update offer: stdin not a terminal, it was asked"

# `y`: the Latest, and the command goes on.
of_run updated 'y\n' "$OF_HOME/.cozy/bin/cozy whoami"
grep -qF "$asked" "$OUT/offer-updated.out" || check "update offer: a fresh home wasn't asked \"$asked\""
[ "$(of_version)" = "cozy $latest" ] || check "update offer: yes, cozy is $(of_version), not $latest: $(tail -c 400 "$OUT/offer-updated.out")"
grep -qF "cozy $before → $latest" "$OUT/offer-updated.out" || check "update offer: yes, the update didn't say cozy $before → $latest"
grep -qF 'error[calcifer_start_failed]' "$OUT/offer-updated.out" || check "update offer: yes, the command didn't go on"
kill "$server" 2> /dev/null || true
}

# of_fresh: the offer's home, cozy-before and nothing else, its `calcifer` a file.
of_fresh() {
  rm -rf "$OF_HOME" && mkdir -p "$OF_HOME/.cozy/bin" && install -m 755 "$E2E_COZY_BEFORE" "$OF_HOME/.cozy/bin/cozy" && : > "$OF_HOME/.cozy/calcifer" \
    || { check "update offer: its home not made"; return 1; }
}

# of_run <name> <typed> <command>: <command> at a terminal in the offer's home, <typed> its input; all it showed in $OUT/offer-<name>.out.
of_run() {
  printf '%b' "$2" | HOME=$OF_HOME COZY_DOWNLOAD_URL=$OF_URL timeout 300 script -qec "$3" /dev/null > "$OUT/offer-$1.out" 2>&1 || true
}

of_version() { HOME=$OF_HOME "$OF_HOME/.cozy/bin/cozy" --version 2>&1; }
