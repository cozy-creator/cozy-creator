#!/usr/bin/env bash
# The `cli` suite: cozy as a person uses it on a computer with no GPU, Linux, macOS or Windows. Install,
# status, the local machine, the candidates through `cozy machine update local` (cozy doctor restarts the
# installer's Calcifer on them and must find everything healthy), a weightless request through them, a real
# BiRefNet removal on the CPU from a YAML request, run list/show, a second executor (the machine restarted)
# with an --asset input, a stuck Calcifer that cozy doctor restarts, and removal as the README says. Where the OS
# runs no models itself (macOS, Windows: lib.sh's LOCAL), what needs no machine, every local verb refused, and the
# rest of that list but the machine.
# A broken step is recorded and the suite goes on, so one run reports all of them.
# --shard runs one of `shards` below after the same start, each on its own runner: e2e.yaml checks coverage over
# all their calls. Whole, the suite checks it itself.
#   e2e/cli/run.sh --out DIR [--shard runs|executors|commands|follow] [--tensord F|V] [--tensorfs F|V]
#                  [--runtime F|V] [--packages DIR]
. "$(dirname "$0")/../lib.sh"
parse "$@"
echo "cl-677 queue proof: red on purpose; this change must never merge" >&2; exit 1
. "$(dirname "$0")/commands.sh"
. "$(dirname "$0")/lifetime.sh"

start() {
step install; install_cozy
user_services
step status; cozy --json status | tee "$OUT/status.json"
[ -z "$LOCAL" ] || { step "the local machine"; machine_ready; }
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
(cd "$here/weightless/e2e_weightless" && cozy --json run .. --describe --on local > "$OUT/parent.json" 2> "$OUT/parent.err") \
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
script=$(cd "$(dirname "$0")/script" && pwd)/words.py folder=$OUT/words-package
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

# Describe reads a checkout's editable install statically (direct_url.json and .pth paths, never their code).
describe() {
step "describe"
cozy --json run "$(birefnet)" --describe > "$OUT/describe.json" 2> "$OUT/describe.err" || check "cozy run $(birefnet) --describe failed: $(tail -c 300 "$OUT/describe.err")"
# (`--describe` prints its table even under --json.)
grep -Eq '(^|[[:space:]"])image("|:)' "$OUT/describe.json" || check "the description of $(birefnet) names no image input"
}

first_run() {
step "BiRefNet on the CPU, from a YAML request"
dog=$(picture dog1.jpg)
# The request file names its image relative to itself, as a person writes one beside their files.
cp "$dog" "$OUT/request-dog.jpg"
printf 'image: request-dog.jpg\n' > "$OUT/request.yaml"
if cell first "$(birefnet)" --input "$OUT/request.yaml"; then white_background "$(output first webp)" "$dog" || check "first: not a background removal"; fi
first=$(cat "$OUT/first.number")
if [ "$first" != none ]; then
  grep -q '"number": *'"$first" "$OUT/first.json" || check "cozy run show $first does not show it"
  cozy --json run list > "$OUT/runs.json" || check "cozy run list failed"
  grep -Eq "\"number\": *$first([^0-9]|\$)" "$OUT/runs.json" || check "cozy run list does not list run $first"
fi
}

# After describe, which installs BiRefNet's environment: the restarted machine runs it in a new executor.
second_executor() {
step "a second executor"
machine_stop || check "the local machine did not stop"
machine_ready
dog=$(picture dog2.jpg)
if cell second "$(birefnet)" --asset "image=$dog"; then white_background "$(output second webp)" "$dog" || check "second: not a background removal"; fi

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
    cozy --json status | jq -e '.calcifer.running' > /dev/null || check "Calcifer does not answer after cozy doctor restarted it"
  fi
fi
}

update() { step update; update_to_newest; }
# Every log cozy wrote (Calcifer, TensorD, executors), before removal erases ~/.cozy: what a failed step left behind.
logs() { step logs; (cd ~/.cozy && find . -name '*.log' -size -20M | tar -czf - -T - > "$OUT/cozy-logs.tgz") || true; }
remove() { logs; step "remove cozy"; remove_cozy; }

# Balanced by step times (runs 40128128311, 40132063815, 40132725041). A shard keeps its parts' order; whole, the suite runs every part.
case $SHARD in
  '') parts="weightless script_package describe first_run second_executor lifetime catalog basics yaml_requests run_flags run_follow run_filters run_play machines completions calcifer_faults update remove" ;;
  runs) parts="weightless describe first_run run_flags run_play logs" ;;
  executors) parts="describe second_executor lifetime update remove" ;;
  commands) parts="catalog basics yaml_requests run_filters logs" ;;
  follow) parts="script_package run_follow machines completions calcifer_faults logs" ;;
  *) echo "no cli shard $SHARD" >&2; exit 2 ;;
esac
[ -n "$LOCAL" ] || parts="catalog refusals completions homebrew calcifer_faults update remove"

start
for part in $parts; do $part; done
[ -n "$SHARD" ] || [ -z "$LOCAL" ] || { step coverage; coverage "$OUT/cozy-under-test" pr; }
finish "cli${SHARD:+ $SHARD}"
