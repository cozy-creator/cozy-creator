#!/usr/bin/env bash
# The `cli` suite: cozy as a person uses it on a computer with no GPU, Linux, macOS or Windows. Install,
# status, the local machine, the candidates through `cozy machine update local` (cozy doctor restarts the
# installer's Calcifer on them and must find everything healthy), a weightless request through them, a real
# BiRefNet removal on the CPU from a YAML request, run list/show, a second executor (the machine restarted)
# with an --asset input, a stuck Calcifer that cozy doctor restarts, and removal as the README says.
# A broken step is recorded and the suite goes on, so one run reports all of them.
#   e2e/cli/run.sh --out DIR [--cozy-bin F] [--tensorfs-wheel F] [--runtime-wheel F] [--packages DIR]
. "$(dirname "$0")/../lib.sh"
parse "$@"

step install; install_cozy
user_services
step status; cozy --json status | tee "$OUT/status.json"
step "the local machine"; machine_ready
step candidates; candidates
step doctor; cozy --json doctor > "$OUT/doctor.json" || check "cozy doctor is not healthy: $(grep -o '"state": *"PROBLEM[^"]*"' "$OUT/doctor.json" | head -3)"

# The candidate set end to end: a request through TensorD, the Runtime and TensorFS to the reply, no model.
step "a weightless request"
text=e2e-$RANDOM$RANDOM back=''
for ((i = ${#text} - 1; i >= 0; i--)); do back+=${text:i:1}; done
if (cd "$(dirname "$0")" && cell weightless ./weightless/reverse "text=$text"); then
  grep -q "$back" "$OUT/weightless.ndjson" || check "weightless: the reply is not $back"
fi

# A script is an ordinary one-file package (xs-099): it and the same file in a package folder describe alike but for
# their names, both run on the CPU, and its function's `accelerator=True` is taken (the GPU when there is one).
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

# An idle executor killed is reaped at once (gone from ps, no zombie), and the next request works.
step "a killed idle executor"
if executor_reaped; then
  if (cd "$(dirname "$0")" && cell reaped ./weightless/reverse "text=$text"); then
    grep -q "$back" "$OUT/reaped.ndjson" || check "reaped: the reply is not $back"
  fi
fi

# Describe reads a checkout's editable install statically (direct_url.json and .pth paths, never their code).
step "describe"
cozy --json run "$(birefnet)" --describe > "$OUT/describe.json" 2> "$OUT/describe.err" || check "cozy run $(birefnet) --describe failed: $(tail -c 300 "$OUT/describe.err")"
# (`--describe` prints its table even under --json.)
grep -Eq '(^|[[:space:]"])image("|:)' "$OUT/describe.json" || check "the description of $(birefnet) names no image input"

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

step "a second executor"
machine_stop || check "the local machine did not stop"
machine_ready
dog=$(picture dog2.jpg)
if cell second "$(birefnet)" --asset "image=$dog"; then white_background "$(output second webp)" "$dog" || check "second: not a background removal"; fi

step "Calcifer reaps the machine it started"; machine_reaped
step "a restarted Calcifer adopts the machine"; calcifer_restart_adopts

# Every other command and flag (e2e/coverage.toml), while Calcifer is healthy.
. "$(dirname "$0")/commands.sh"

step "Calcifer stopped under the machine it started"
if calcifer_orphaned; then
  (cd "$(dirname "$0")" && cell after-orphan ./weightless/reverse "text=orphan$RANDOM") || check "no run went after cozy doctor recovered Calcifer"
fi

step "a stuck Calcifer"
if calcifer_stuck; then
  (cd "$(dirname "$0")" && cell after-stuck ./weightless/reverse "text=after$RANDOM") || check "no run went after cozy doctor restarted Calcifer"
fi

step update; update_to_newest

# Every log cozy wrote (Calcifer, TensorD, executors), before removal erases ~/.cozy: what a failed step left behind.
step logs; (cd ~/.cozy && find . -name '*.log' -size -20M | tar -czf - -T - > "$OUT/cozy-logs.tgz") || true
step "remove cozy"; remove_cozy
step coverage; coverage "$OUT/cozy-under-test" pr
finish "cli: runs $first and $(cat "$OUT/second.number")"
