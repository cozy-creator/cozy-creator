#!/usr/bin/env bash
# The `cli` suite: cozy as a person uses it on a computer with no GPU, Linux, macOS or Windows. Install,
# status, the local machine, the candidates through `cozy machine update local` (cozy doctor restarts the
# installer's Calcifer on them and must find everything healthy), a weightless request through them, a real
# BiRefNet removal on the CPU from a YAML request, run ls/show, a second executor (the machine restarted)
# with an --file input, a stuck Calcifer that cozy doctor restarts, and removal as the README says. Where the OS
# runs no models itself (macOS, Windows: lib.sh's LOCAL), what needs no machine, every local verb refused, and the
# rest of that list but the machine. On every OS, the page Calcifer serves (web.sh).
# A broken step is recorded and the suite goes on, so one run reports all of them.
# --shard runs one of `shards` below after the same start, each on its own runner: e2e.yaml checks coverage over
# all their calls. Whole, the suite checks it itself.
#   e2e/cli/run.sh --out DIR [--shard runs|executors|commands|follow|web|web-core|web-palette|web-drawer|web-toolword|web-legible|web-beats|web-paste|web-dismiss|web-scale|web-canvas|web-dropped|web-layout-1280|web-layout-1600|web-layout-2560|project-stream|project-stream-web|projectdb|projectdb-latency|projectdb-existing|projectdb-web|projectvault|projectdb-cancel|projectdb-live|projectdb-scale|local-code]
#                  [--tensord F|V] [--tensorfs F|V] [--runtime F|V] [--packages DIR]
. "$(dirname "$0")/../lib.sh"
parse "$@"
. "$(dirname "$0")/commands.sh"
. "$(dirname "$0")/lifetime.sh"
. "$(dirname "$0")/control.sh"
. "$(dirname "$0")/project.sh"
. "$(dirname "$0")/web.sh"
. "$(dirname "$0")/projectdb.sh"
. "$(dirname "$0")/statedb.sh"
. "$(dirname "$0")/projectvault.sh"
. "$(dirname "$0")/machineset.sh"
. "$(dirname "$0")/rental.sh"
. "$(dirname "$0")/fallback.sh"
. "$(dirname "$0")/previews.sh"

start() {
step install; install_cozy; run_project "$OUT/project" || fail "project initialization failed"
user_services
step status; fixture_calcifer | tee "$OUT/status.json"
[ -z "$LOCAL" ] || { step "the local machine"; machine_ready; }
[ "$SHARD" != commands ] || version_skew
step candidates; candidates
step doctor; cozy --json doctor > "$OUT/doctor.json" || check "cozy doctor is not healthy: $(grep -o '"state": *"PROBLEM[^"]*"' "$OUT/doctor.json" | head -3)"
}

# The candidate set end to end: a request through TensorD, the Runtime and TensorFS to the reply, no model.
weightless() {
step "a weightless request"
text=e2e-$RANDOM$RANDOM back=''
for ((i = ${#text} - 1; i >= 0; i--)); do back+=${text:i:1}; done
# Local code spelled as this OS spells paths (`.\weightless\reverse` on Windows), and `..` is a path too.
spelled=./weightless/reverse
[ "${RUNNER_OS:-}" != Windows ] || spelled='.\weightless\reverse'
if (cd "$here" && cell weightless "$spelled" "text=$text"); then
  grep -q "$back" "$OUT/weightless.ndjson" || check "weightless: the reply is not $back"
fi
(in_project "$RUN_PROJECT" --json run "$here/weightless/e2e_weightless/.." --describe --on local > "$OUT/parent.json" 2> "$OUT/parent.err") \
  || check "cozy run .. did not describe the package above: $(tail -c 300 "$OUT/parent.err")"

# An idle executor killed is reaped at once (gone from ps, no zombie), and the next request works.
step "a killed idle executor"
if executor_reaped; then
  if (cd "$here" && cell reaped ./weightless/reverse "text=$text"); then
    grep -q "$back" "$OUT/reaped.ndjson" || check "reaped: the reply is not $back"
  fi
fi
}

# A script is an ordinary one-file package (xs-099): it and the same file in a package folder describe alike but for
# their names, both run on the CPU, and its function's `accelerator=True` is taken (the GPU when there is one).
script_package() {
step "a script is a package"
script=$here/script/words.py folder=$OUT/words-package
mkdir -p "$folder" && cp "$script" "$folder/words.py"
cat > "$folder/pyproject.toml" <<'TOML'
[project]
name = "words-package"
version = "0.0.0"
requires-python = ">=3.12"
dependencies = ["cozy-runtime>=0.30", "msgspec>=0.19"]

[project.entry-points."cozy.application"]
default = "words:app"

[build-system]
requires = ["hatchling>=1.25"]
build-backend = "hatchling.build"

[tool.hatch.build.targets.wheel]
only-include = ["words.py"]
TOML
for form in script folder; do
  at=${!form}
  cozy --json run "$at" --describe --on local > "$OUT/describe-$form.json" 2> "$OUT/describe-$form.err" || check "cozy run $form --describe failed: $(tail -c 300 "$OUT/describe-$form.err")"
  cozy run "$at" --describe --on local 2>> "$OUT/describe-$form.err" | sed "s#local/words-package/#local/words/#g" > "$OUT/describe-$form.txt" || check "cozy run $form --describe (text) failed"
done
cmp "$OUT/describe-script.json" "$OUT/describe-folder.json" || check "a script's --describe --json differs from its package folder's"
cmp "$OUT/describe-script.txt" "$OUT/describe-folder.txt" || check "a script's --describe differs from its package folder's (but for the name)"
grep -q '"text"' "$OUT/describe-script.json" || check "the script's description names no text input"
for form in script folder; do
  if cell "words-$form" "${!form}" "text=one two three $RANDOM"; then
    grep -q '"words": *4' "$OUT/words-$form.ndjson" || check "words-$form: the reply is not 4 words"
  fi
done
# accelerator=True is the default's GPU-when-there-is-one: an interface writes only `false`.
kept=$(ls ~/.cozy/packages/local/words/*.json 2> /dev/null | head -1)
[ -n "$kept" ] && ! grep -q '"accelerator":false' "$kept" || check "the script's interface is not accelerator=True (${kept:-none kept})"
}

# The shared image contract's canvas envelope (cozy-runtime #1830): an output ratio from 1:16 through 16:1 runs and its
# canvas stays inside it; one outside is refused while the request decodes, before the function runs.
image_ratio() {
step "image ratios"
ratio=$here/script/ratio.py
# Past 16:1 a ratio holds there with a warning (xs-149).
for r in 16:1 1:16 32000:2000 17:1 1:17 32768:32; do
  name=ratio-${r/:/-}
  if cell "$name" "$ratio" "prompt=ratio $RANDOM" "aspect_ratio=$r"; then
    w=$(grep -o '"width": *[0-9]*' "$OUT/$name.ndjson" | tail -1 | grep -o '[0-9]*$')
    h=$(grep -o '"height": *[0-9]*' "$OUT/$name.ndjson" | tail -1 | grep -o '[0-9]*$')
    { [ -n "$w" ] && [ -n "$h" ] && [ "$w" -le $((16 * h)) ] && [ "$h" -le $((16 * w)) ]; } || check "$name: canvas ${w:-?}x${h:-?} is not within 16:1"
    echo "$name: canvas ${w}x${h}"
  fi
done
# What a function doesn't take is said, and the run goes on: these are no input of `ImageRequest`.
if cell ratio-unknown "$ratio" "prompt=ratio $RANDOM" negative_prompt=blurry guidance:=3 prompt_upsampling:=true; then
  for field in guidance negative_prompt prompt_upsampling; do
    grep -q "reads no input $field" "$OUT/ratio-unknown.ndjson" "$OUT/ratio-unknown.err" \
      || check "ratio-unknown: $field, which it doesn't take, isn't said"
  done
fi
}

# Describe reads a checkout's editable install statically (direct_url.json and .pth paths, never their code).
describe() {
step "describe"
cozy --json run "$(birefnet)" --describe > "$OUT/describe.json" 2> "$OUT/describe.err" || check "cozy run $(birefnet) --describe failed: $(tail -c 300 "$OUT/describe.err")"
# (`--describe` prints its table even under --json.)
grep -Eq '(^|[[:space:]"])image("|:)' "$OUT/describe.json" || check "the description of $(birefnet) names no image input"
}

# The forward pass (~190 s on a runner's two cores) runs when the change can alter it (--model).
# run_flags separately checks BiRefNet preparation through the warming API.
first_run() {
[ -n "$MODEL" ] || { echo "BiRefNet's forward pass: not selected (ci/affected's model)"; return 0; }
step "BiRefNet on the CPU, from a YAML request"
dog=$(picture dog1.jpg)
# The request file names its image relative to itself, as a person writes one beside their files.
cp "$dog" "$OUT/request-dog.jpg"
printf 'image: request-dog.jpg\n' > "$OUT/request.yaml"
if cell first "$(birefnet)" --input "$OUT/request.yaml"; then white_background "$(output first webp)" "$dog" || check "first: not a background removal"; fi
first=$(cat "$OUT/first.number")
if [ "$first" != none ]; then
  grep -q '"number": *'"$first" "$OUT/first.json" || check "cozy run watch $first does not show it"
  cozy --json run ls > "$OUT/runs.json" || check "cozy run ls failed"
  grep -Eq "\"number\": *$first([^0-9]|\$)" "$OUT/runs.json" || check "cozy run ls does not list run $first"
fi
}

# The restarted machine runs a request with a file input in a new executor (weightless: the first run loads weights).
second_executor() {
step "a second executor"
machine_stop || check "the local machine did not stop"
machine_ready
note=$OUT/second-note.txt; echo "second $RANDOM" > "$note"
(cd "$here" && cell second ./weightless/fidelities --file "notes=$note") || true

step "Calcifer reaps the machine it started"; machine_reaped
step "a restarted Calcifer adopts the machine"; calcifer_restart_adopts
}

# After the other parts, as these stop Calcifer, and not straight after calcifer_restart_adopts: systemd starts a
# unit at most five times in 10 s (run 40132725041: doctor's restart was refused).
calcifer_faults() {
if [ -n "$LOCAL" ]; then
  step "Calcifer stopped under the machine it started"
  if calcifer_orphaned; then
    (cd "$here" && cell after-orphan ./weightless/reverse "text=orphan$RANDOM") || check "no run went after cozy doctor recovered Calcifer"
  fi
fi

step "a stuck Calcifer"
if calcifer_stuck; then
  if [ -n "$LOCAL" ]; then
    (cd "$here" && cell after-stuck ./weightless/reverse "text=after$RANDOM") || check "no run went after cozy doctor restarted Calcifer"
  else
    fixture_calcifer | jq -e '.calcifer.running' > /dev/null || check "Calcifer does not answer after cozy doctor restarted it"
  fi
fi
}

# cl-716 B: a run followed while Calcifer restarts goes on being followed to its end; a burst of restarts past
# systemd's start limit (five in 10 s) leaves Calcifer startable by the next command. Last, as these stop Calcifer.
calcifer_restarts() {
[ "$RUNNER_OS" = Linux ] || return 0
step "a run followed across a Calcifer restart"
local code n
(cd "$here" && in_project "$RUN_PROJECT" run "$here/weightless/wait" seconds:=15 --on local > "$OUT/across.txt" 2>&1; echo $? > "$OUT/across.code") &
local follower=$!
for _ in $(seq 120); do n=$(grep -oE '^run [0-9]+ ' "$OUT/across.txt" | head -1 | grep -oE '[0-9]+') && break; sleep 0.5; done
[ -n "$n" ] || check "across: the run never began"
systemctl --user restart cozy-calcifer || check "systemctl --user restart cozy-calcifer failed"
wait "$follower" || true
code=$(cat "$OUT/across.code" 2>/dev/null) || code=
{ [ "$code" = 0 ] && grep -q "Calcifer restarted; following run $n again" "$OUT/across.txt"; } \
  || check "across: the follow of run ${n:-?} did not go on past the restart (exit $code): $(tail -c 300 "$OUT/across.txt")"
cozy --json run watch "$n" --once | J '.status == "completed"' || check "across: run $n did not complete"

step "a burst of Calcifer restarts past systemd's start limit"
for _ in 1 2 3 4 5 6 7; do systemctl --user restart cozy-calcifer > /dev/null 2>&1 || true; done
systemctl --user show -p Result --value cozy-calcifer > "$OUT/burst-result.txt" || true
echo "after the burst: $(cat "$OUT/burst-result.txt")"
cozy --json machine ls > "$OUT/burst-machines.json" 2> "$OUT/burst-machines.err" && fixture_calcifer | J '.calcifer.running == true' \
  || check "after a burst of restarts ($(cat "$OUT/burst-result.txt")), the next command did not start Calcifer: $(tail -c 300 "$OUT/burst-machines.err")"
(cd "$here" && cell after-burst ./weightless/reverse "text=burst$RANDOM") || check "no run went after the burst of restarts"
}

update() { step update; update_to_newest; }
# Every log cozy wrote (Calcifer, TensorD, executors), before removal erases ~/.cozy: what a failed step left behind.
# As files, which e2e/publish can scrub.
logs() { step logs; mkdir -p "$OUT/cozy-logs"; (cd ~/.cozy && find . -name '*.log' -size -20M | tar -cf - -T - | (cd "$OUT/cozy-logs" && tar -xf -)) || true; }
remove() { logs; step "remove cozy"; remove_cozy; }

# Local code of one name at two paths, each saying which it is, run in turn: each runs its own, not the first's install (cl-733: a
# folder; cl-763: a script).
one_name_two_paths() {
step "one name at two paths"
local words at turn=0
words=$here/script/words.py
for at in 1 2; do
  mkdir -p "$OUT/paths/$at/one-name"
  sed "s/words=len(payload.text.split())/words=$at/" "$words" > "$OUT/paths/$at/one-name/one_name.py"
  cat > "$OUT/paths/$at/one-name/pyproject.toml" <<'TOML'
[project]
name = "one-name"
version = "0.0.0"
requires-python = ">=3.12"
dependencies = ["cozy-runtime>=0.30", "msgspec>=0.19"]

[project.entry-points."cozy.application"]
default = "one_name:app"

[build-system]
requires = ["hatchling>=1.25"]
build-backend = "hatchling.build"

[tool.hatch.build.targets.wheel]
only-include = ["one_name.py"]
TOML
done
for at in 1 2 1; do
  turn=$((turn + 1))
  if cell "folder-$turn" "$OUT/paths/$at/one-name" "text=turn $turn $RANDOM"; then
    grep -q "\"words\": *$at\b" "$OUT/folder-$turn.ndjson" || check "one package name at two paths: path $at ran another's code"
  fi
done
cozy --json package uninstall local/one-name --on local > /dev/null || check "package uninstall local/one-name failed"
for at in 1 2; do
  mkdir -p "$OUT/paths/$at/scripts"
  sed "s/words=len(payload.text.split())/words=$at/" "$words" > "$OUT/paths/$at/scripts/one_script.py"
done
for at in 1 2 1; do
  turn=$((turn + 1))
  if cell "script-$turn" "$OUT/paths/$at/scripts/one_script.py" "text=turn $turn $RANDOM"; then
    grep -q "\"words\": *$at\b" "$OUT/script-$turn.ndjson" || check "one script name at two paths: path $at ran another's code"
  fi
done
cozy --json package uninstall local/one_script --on local > /dev/null || check "package uninstall local/one_script failed"
}

# Balanced by step times (runs 40128128311, 40132063815, 40132725041). A shard keeps its parts' order; whole, the suite runs every part.
case $SHARD in
  '') parts="entrypoints weightless script_package one_name_two_paths image_ratio describe first_run previews second_executor lifetime kept_preparing catalog basics yaml_requests run_flags run_follow time_left broken_answers run_filters run_playback hub_away lock_refresh bound_runs own_references no_descriptions orbit_angles old_runtime hostile_media unseen local_edits path_manifest rental_end machine_show_runs writes_once project_stream web_serves page_resend queued_download machines local_idle machine_set fall_back completions calcifer_faults calcifer_restarts update remove" ;;
  runs) parts="weightless describe first_run previews run_flags run_playback hub_away lock_refresh logs" ;;
  executors) parts="second_executor lifetime kept_preparing update remove" ;;
  local-wake) parts="local_wake_update logs" ;;
  entrypoints) parts="entrypoints" ;;
  commands) parts="entrypoints catalog basics yaml_requests run_filters bound_runs own_references no_descriptions orbit_angles old_runtime hostile_media machine_show_runs writes_once project_stream logs" ;;
  # Its own runner: under `set -e` a cell that dies ends its shard, and a page's proofs shouldn't go with it.
  web) parts="tools_read web_serves page_resend logs" ;;
  project-stream) parts="project_stream logs" ;;
  project-stream-web) DB_WEB="drawers activity versions timeline"; parts="tools_read db_existing_web logs" ;;
  web-core) parts="tools_read web_core page_resend logs" ;;
  web-firefox) parts="tools_read web_firefox logs" ;;
  # The palette against a Hub holding every first-party package (e2e/isolated.sh), and with none (cl-896).
  web-palette) export DB_WEB=palette; parts="tools_read db_existing_web logs" ;;
  # The tool beside a box's name (cl-927).
  web-toolword) export DB_WEB=toolword; parts="tools_read db_existing_web logs" ;;
  # The right-side panel closed by a click off it (cl-929).
  web-drawer) export DB_WEB=drawer; parts="tools_read db_existing_web logs" ;;
  # A press from inside never dismisses (cl-923).
  web-dismiss) export DB_WEB=dismiss; parts="tools_read db_existing_web logs" ;;
  # Typed words over a picture (cl-901).
  web-legible) export DB_WEB=legible; parts="tools_read db_existing_web logs" ;;
  # A Shot's beats laid out (cl-903).
  web-beats) export DB_WEB=beats; parts="tools_read db_existing_web logs" ;;
  # Pasted mentions (cl-902).
  web-paste) export DB_WEB=paste; parts="tools_read db_existing_web logs" ;;
  # A rendering box at rest, and new takes not looked at (cl-928).
  web-newtakes) export DB_WEB=newtakes; parts="tools_read db_existing_web logs" ;;
  web-scale) export DB_WEB=scale; parts="tools_read db_existing_web logs" ;;
  web-canvas) export DB_WEB=canvas; parts="tools_read db_existing_web logs" ;;
  # Files dropped: twenty pictures at once, each drawn once where and as it stays (cl-918), and Place's drop, chooser, rename and moves.
  web-dropped) export DB_WEB="dropped place"; parts="tools_read db_existing_web logs" ;;
  web-layout-1280) export DB_WEB=layout SIZES=1280x720; parts="tools_read db_existing_web logs" ;;
  web-layout-1600) export DB_WEB=layout SIZES=1600x1000,1600x1000@1.25; parts="tools_read db_existing_web logs" ;;
  web-layout-2560) export DB_WEB=layout SIZES=2560x1440@2x; parts="tools_read db_existing_web logs" ;;
  follow) parts="script_package one_name_two_paths image_ratio run_follow time_left broken_answers queued_download machines completions calcifer_faults calcifer_restarts logs" ;;
  # project.db (cl-803) and state.db (cl-818) alone, and project.db's latency against the cozy before it. A project's machines (cl-873), an idle
  # computer placed on (cl-894), a new take never changing what's approved (cl-908), and one not looked at (cl-928).
  projectdb) parts="projectdb statedb local_idle machine_set fall_back approvals newtakes logs" ;;
  projectvault) parts="projectvault vault_leftovers vault_web logs" ;;
  projectdb-latency) parts="db_latency logs" ;;
  projectdb-existing) parts="db_existing logs" ;;
  projectdb-web) parts="tools_read db_existing_web logs" ;;
  projectdb-palette) parts="tools_read db_existing_web logs"; DB_WEB="nodes tools" ;;
  projectdb-focus) parts="tools_read db_existing_web logs"; DB_WEB=focused ;;
  projectdb-select) parts="tools_read db_existing_web logs"; DB_WEB=select ;;
  projectdb-cancel) parts="db_cancel_undo logs" ;;
  projectdb-live) parts="tools_read db_existing_web logs"; DB_WEB=live ;;
  projectdb-instant) parts="tools_read db_existing_web logs"; DB_WEB=instant ;;
  projectdb-versions) parts="tools_read db_existing_web logs"; DB_WEB=versions ;;
  projectdb-menu) parts="tools_read db_existing_web logs"; DB_WEB=menu ;;
  projectdb-selected) parts="tools_read db_existing_web logs"; DB_WEB=selected ;;
  projectdb-offline) parts="tools_read db_existing_web logs"; DB_WEB=offline ;;
  projectdb-scale) parts="tools_read db_existing_web logs"; DB_WEB=scale ;;
  # Local code and every input by content (cl-817, cl-820): the CLI in a folder Calcifer can't see,
  # edits timed, a published cozy package called from local code (cl-871) and from an account's package (cl-872), a child
  # result's Latents in another package's caller (cr-276), a host path refused; then a docker rental's end, signed out,
  # with a revoked key and signed in (cl-875).
  local-code) parts="unseen local_edits hub_dependency hub_published child_latents path_manifest rental_end logs" ;;
  # By hand: the control-plane audit's cells alone (control.sh).
  control) parts="machine_show_runs writes_once page_resend hub_away broken_answers logs" ;;
  *) echo "no cli shard $SHARD" >&2; exit 2 ;;
esac
# macOS and Windows run what's theirs: the CLI, Calcifer, paths, the service and the page, on a Hub with only packages
# published (e2e/isolated.sh). The catalog's reads of published weights are the same on every OS: Linux's commands shard.
if [ -z "$LOCAL" ]; then
  case $SHARD in
    entrypoints | projectvault) ;;
    web-layout-* | web-scale | web-canvas) parts="db_existing_web logs" ;;
    web-core) parts="entrypoints refusals web_native completions homebrew calcifer_faults update remove" ;;
    *) parts="entrypoints refusals web_serves completions homebrew calcifer_faults update remove" ;;
  esac
fi

start
# Package fixtures must still resolve after entering the project directory.
if [ -n "$PACKAGES" ]; then
  PACKAGES=$(cd "$PACKAGES" && pwd) || fail "package checkout is unavailable"
fi
cd "$RUN_PROJECT" || fail "project directory is unavailable"
for part in $parts; do $part; done
[ -n "$SHARD" ] || [ -z "$LOCAL" ] || { step coverage; coverage "$OUT/cozy-under-test" pr; }
finish "cli${SHARD:+ $SHARD}"
