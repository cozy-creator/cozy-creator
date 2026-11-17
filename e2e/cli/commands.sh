# e2e/cli/commands.sh: the rest of cozy's PR-tier commands and flags (e2e/coverage.toml), parts cli/run.sh calls
# after its runs or in its own shards: signed out, CPU, the local machine up (where this OS has one). Each call's
# answer is checked, not its exit code alone. Signed-in and renting commands are the release suite's (e2e/release/run.sh).
here=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
J() { jq -e "$@" > /dev/null; }  # J <filter> <file>: the answer holds what the filter asks

# The public entrypoints work without the retired canvas-editing command group.
entrypoints() {
  step "public commands and explicit project creation"
  ARGV_LOG="$OUT/argv.log" python3 "$here/entrypoints.py" --cozy "$COZY" --out "$OUT/entrypoints" \
    || check "entrypoints: see $OUT/entrypoints/results.json"
}

# Every command a hint names is one this cozy has: each `cozy …` its code, page and docs tell someone to type, walked down its help.
hint_commands() {
  step "every command a hint, a next line or a doc names parses"
  python3 "$here/hints.py" --cozy "$COZY" --root "$(git -C "$here" rev-parse --show-toplevel)" --out "$OUT/hints" \
    || check "hint commands: $(jq -r '.stale | keys | join(", ")' "$OUT/hints/hints.json" 2> /dev/null)"
}

# What needs no machine: Calcifer, the Hub setting, signing out, the Hub's packages and models.
catalog() {
step status
fixture_calcifer > "$OUT/status.json" || check "cozy status failed"
J '.calcifer.running == true and (.calcifer.version | length > 0)' "$OUT/status.json" || check "status: Calcifer is not running"
if [ -n "$LOCAL" ]; then
  J '.machines | any(.name == "local")' "$OUT/status.json" || check "status: the local machine is not listed"
else
  J '.machines | all(.name != "local")' "$OUT/status.json" || check "status: lists a local machine this OS cannot run"
fi

step hub
# tensorhub.com unless COZY_HUB_URL (or config.toml's hub_url) names another: one there is none at fails.
! COZY_HUB_URL=http://127.0.0.1:9 cozy --json package ls birefnet --limit 1 > "$OUT/hub-env.json" 2>&1 \
  || check "COZY_HUB_URL=http://127.0.0.1:9: package ls did not go there"
# A run made at that other Hub (local code reads none), then the variable unset: Calcifer still lists and shows it.
if [ -n "$LOCAL" ] && (cd "$here" && COZY_HUB_URL=http://127.0.0.1:9 cell other-hub ./weightless/reverse "text=hub$RANDOM"); then
  n=$(cat "$OUT/other-hub.number")
  cozy --json --full run ls --limit 0 | J --argjson n "$n" 'any(.runs[]; .number == $n and .hub == "http://127.0.0.1:9")' \
    || check "run ls: run $n, made at another Hub, is not listed"
  cozy --json run watch "$n" --once | J '.hub == "http://127.0.0.1:9" and .status == "completed"' || check "run watch $n: not its record at the other Hub"
fi

step "signed out"
cozy --json whoami > "$OUT/whoami.json" && J '.logged_in == false' "$OUT/whoami.json" || check "whoami: not signed out"
cozy --json logout > "$OUT/logout.json" || check "logout while signed out failed"

step packages
cozy --json package ls birefnet --limit 1 > "$OUT/search.json" && J '(.packages | length) == 1 and .packages[0].package == "birefnet"' "$OUT/search.json" \
  || check "package ls --limit 1: not exactly birefnet"
cozy --json package ls --limit 1 > "$OUT/packages-first-page.json" || check "package first page failed"
cozy --json package ls --limit 1 --offset 1 > "$OUT/packages-second-page.json" || check "package second page failed"
if [ "$(jq '.total' "$OUT/packages-first-page.json")" -gt 1 ]; then
  [ "$(jq -r '.packages[0].package' "$OUT/packages-first-page.json")" != "$(jq -r '.packages[0].package' "$OUT/packages-second-page.json")" ] \
    || check "package offset repeated the first page"
fi
package_details birefnet > "$OUT/package-info.json" && J '(.latest_release | length > 0) and (.releases | length > 0)' "$OUT/package-info.json" \
  || check "package details birefnet: no releases"
cozy --json run birefnet/remove_background --describe > "$OUT/describe-published.json" 2> "$OUT/describe-published.err" \
  && grep -Eq '(^|[[:space:]"])image("|:)' "$OUT/describe-published.json" || check "run birefnet/remove_background --describe: no image input"
cozy run birefnet > "$OUT/functions.txt" 2>&1 && grep -q remove_background "$OUT/functions.txt" || check "run birefnet: remove_background is not listed"

step models
cozy --json model ls birefnet --limit 1 > "$OUT/models-search.json" && J '(.models | length) == 1' "$OUT/models-search.json" \
  || check "model ls --limit 1: not one model"
cozy --json model ls --limit 100 > "$OUT/models-all.json" || check "model ls --limit 100: $(head -c 300 "$OUT/models-all.json")"
family=$(jq -r '[.models[].family | select(. != null)][0] // empty' "$OUT/models-all.json") || true
if [ -n "$family" ]; then
  cozy --json model ls --family "$family" > "$OUT/family.json" && J --arg f "$family" '(.models | length > 0) and all(.models[]; .family == $f)' "$OUT/family.json" \
    || check "model ls --family $family: other families listed"
else
  cozy --json model ls --family e2e-no-such-family > "$OUT/family.json" && J '(.models | length) == 0' "$OUT/family.json" \
    || check "model ls --family: an unknown family listed models"
fi
# Tags: a search that leaves a tag out lists none that has it; one that asks for it lists only those.
cozy --json model ls --no-tag nsfw --limit 100 > "$OUT/no-nsfw.json" && J 'all(.models[]; (.tags // []) | index("nsfw") | not)' "$OUT/no-nsfw.json" \
  || check "model ls --no-tag nsfw: a model tagged nsfw listed"
cozy --json model ls --tag nsfw --limit 100 > "$OUT/nsfw.json" && J 'all(.models[]; (.tags // []) | index("nsfw"))' "$OUT/nsfw.json" \
  || check "model ls --tag nsfw: a model without the tag listed"
cozy --json model ls --limit 1 > "$OUT/models-first-page.json" || check "model first page failed"
cozy --json model ls --limit 1 --offset 1 > "$OUT/models-second-page.json" || check "model second page failed"
if [ "$(jq '.total' "$OUT/models-first-page.json")" -gt 1 ]; then
  [ "$(jq -r '.models[0].model' "$OUT/models-first-page.json")" != "$(jq -r '.models[0].model' "$OUT/models-second-page.json")" ] || check "model offset repeated the first page"
fi
model_details zhengpeng7/birefnet > "$OUT/model-info.json" && J '.releases[0].lanes | length > 0' "$OUT/model-info.json" || check "model metadata zhengpeng7/birefnet: no lanes"
lane=$(jq -r '.releases[0].lanes[0].lane' "$OUT/model-info.json") || true
# What each lane's checkpoint holds, from its header alone (xs-183): its components, one lane by --lane, and a LoRA's modules.
cozy --json model show zhengpeng7/birefnet > "$OUT/model-show.json" \
  && J '(.lanes | length > 0) and all(.lanes[]; (.header | startswith("sha256:")) and any(.components[]; .tensors > 0))' "$OUT/model-show.json" \
  || check "model show zhengpeng7/birefnet: no lane with its components: $(head -c 300 "$OUT/model-show.json")"
cozy --json model show zhengpeng7/birefnet --lane "$lane" > "$OUT/model-show-lane.json" \
  && J --arg lane "$lane" '(.lanes | length) == 1 and .lanes[0].lane == $lane' "$OUT/model-show-lane.json" || check "model show --lane $lane: not that one lane"
cozy model show zhengpeng7/birefnet > "$OUT/model-show.txt" 2>&1 && grep -Eq '^checkpoint +sha256:' "$OUT/model-show.txt" \
  || check "model show: no checkpoint line: $(head -c 300 "$OUT/model-show.txt")"
# The H3 LoRA isolated.sh seeds as musubi-tuner writes it: lora_down/lora_up and .alpha of one block.
lora=alissonerdx/minimax-h3-vfx-edit
cozy --json lora ls --limit 100 > "$OUT/lora-ls.json" && J --arg m "$lora" 'any(.models[]; .model == $m)' "$OUT/lora-ls.json" \
  || check "lora ls: $lora not listed: $(head -c 300 "$OUT/lora-ls.json")"
cozy --json lora show "$lora" > "$OUT/lora-show.json" && J 'any(.lanes[].components[]; .lora.modules > 0)' "$OUT/lora-show.json" \
  || check "lora show $lora: no component with LoRA modules: $(head -c 300 "$OUT/lora-show.json")"
lora_lane=$(jq -r '.lanes[0].lane // empty' "$OUT/lora-show.json") || true
cozy --json lora show "$lora" --lane "$lora_lane" > "$OUT/lora-show-lane.json" && J '(.lanes | length) == 1' "$OUT/lora-show-lane.json" \
  || check "lora show $lora --lane $lora_lane: not one lane"

step "renting, signed out"
# The catalog's shape, never its stock: an empty market or a sold-out SKU is not a code failure.
cozy machine rent > "$OUT/rent-catalog.txt" 2>&1 && grep -q -E '^NAME +GPU +GPUS +COMPUTE +VRAM +\$/HOUR' "$OUT/rent-catalog.txt" \
  || check "machine rent: no catalog table: $(head -c 300 "$OUT/rent-catalog.txt")"
cozy --json machine rent > "$OUT/rent-catalog.json" 2> "$OUT/rent-catalog.err" && J 'type == "array" and all(.[]; .name | length > 0)' "$OUT/rent-catalog.json" \
  || check "machine rent --json: no catalog array: $(head -c 300 "$OUT/rent-catalog.json") $(head -c 200 "$OUT/rent-catalog.err")"
# Pinned to the checkout's release, a run --on new is refused before the sign-in check.
bir=birefnet/remove_background; v=$(sed -n 's/^version = "\(.*\)"$/\1/p' "${PACKAGES:-/nonexistent}/birefnet/pyproject.toml" 2>/dev/null | head -1)
[ -z "$v" ] || bir="birefnet/remove_background@$v"
code=0; in_project "$RUN_PROJECT" --json run "$bir" --file "image=$(picture dog1.jpg)" --on new > "$OUT/rent-signed-out.json" 2>&1 || code=$?
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
cozy --json package ls --on local > "$OUT/packages.json" && grep -q 'e2e-weightless' "$OUT/packages.json" || check "package ls --on local: no local/e2e-weightless"
cozy --json package uninstall local/e2e-weightless --on local > "$OUT/remove.ndjson" || check "package uninstall local/e2e-weightless --on local failed"
cozy --json package ls --on local > "$OUT/packages-after.json" && ! grep -q 'e2e-weightless' "$OUT/packages-after.json" \
  || check "package uninstall: local/e2e-weightless is still installed"

step "models on local"
cozy --json model install zhengpeng7/birefnet --lane "$lane" --on local > "$OUT/download.ndjson" || check "model install --lane $lane failed"
cozy --json model install zhengpeng7/birefnet local/e2e-birefnet > "$OUT/download-alias.ndjson" || check "model install zhengpeng7/birefnet local/e2e-birefnet failed"
# A local .safetensors file, made into a model the machine keeps by name: the file is written to the machine
# and made from its object (object://sha256:…/<name>). No format knows its one key `w`, so it is kept as-is.
safetensors_file "$OUT/tiny.safetensors"
cozy --json model install "$OUT/tiny.safetensors" local/e2e-tiny --source-format as-is > "$OUT/download-file.ndjson" || check "model install ./tiny.safetensors local/e2e-tiny failed: $(grep -oE '"message": *"[^"]*"' "$OUT/download-file.ndjson" | tail -1)"
cozy --json model ls --on local > "$OUT/models.json" || check "model ls --on local failed"
grep -q e2e-tiny "$OUT/models.json" || check "model ls --on local: the local file's local/e2e-tiny is not listed"
J --arg lane "$lane" 'any(.models[]; .model == "zhengpeng7/birefnet" and .lane == $lane)' "$OUT/models.json" || check "model ls --on local: no zhengpeng7/birefnet in lane $lane after --lane $lane"
grep -q e2e-birefnet "$OUT/models.json" || check "model ls --on local: the local/e2e-birefnet alias is not listed"
}

# cl-1076 (Paul, 11-16, a rental's disk full with LTX 2.5's 71.5 GB unused): "The CLI can remove a model from a rental. `cozy model
# uninstall` only works on this computer; give it `--on <machine>`." On local through the route a rental takes: while ondemand.py's
# `hold` holds with e2e/uninstall bound, its uninstall is refused naming that run; once the run ends, it goes, its bytes freed.
uninstall_on() {
step "a model uninstalled from a named machine, refused while a run reads it (cl-1076)"
[ -n "$LOCAL" ] || return 0
local run
cozy --json model install e2e/uninstall --on local > "$OUT/uninstall-on-install.ndjson" 2>&1 \
  || { check "uninstall --on: model install e2e/uninstall failed: $(tail -c 300 "$OUT/uninstall-on-install.ndjson")"; return; }
[ -n "$(run_submit "$OUT" uninstall-on-hold run "$here/script/ondemand.py/hold" seconds:=20 --model extra=e2e/uninstall --on local)" ] \
  || { check "uninstall --on: the holding run was not made: $(tail -c 300 "$OUT/uninstall-on-hold.err")"; return; }
run=$(cat "$OUT/uninstall-on-hold.id")
for _ in $(seq 900); do grep -q '"holding"' "$OUT/uninstall-on-hold.ndjson" 2> /dev/null && break; sleep 0.2; done
grep -q '"holding"' "$OUT/uninstall-on-hold.ndjson" || { check "uninstall --on: run $run never held: $(tail -c 300 "$OUT/uninstall-on-hold.ndjson")"; return; }
cozy --json model uninstall e2e/uninstall --on local > "$OUT/uninstall-on-held.json" 2> "$OUT/uninstall-on-held.err" \
  && check "uninstall --on: e2e/uninstall went while run $run held it"
J --arg run "$run" '.error.cause == "model_in_use" and (.error.message | contains($run)) and .error.next == ["cozy run watch \($run)"]' "$OUT/uninstall-on-held.json" \
  || check "uninstall --on: not refused naming run $run: $(head -c 400 "$OUT/uninstall-on-held.json") $(tail -c 200 "$OUT/uninstall-on-held.err")"
run_wait "$OUT" uninstall-on-hold && [ "$(run_get "$OUT" uninstall-on-hold .status)" = completed ] \
  || { check "uninstall --on: run $run did not complete: $(tail -c 300 "$OUT/uninstall-on-hold.ndjson")"; return; }
cozy --json model uninstall e2e/uninstall --on local > "$OUT/uninstall-on.json" 2> "$OUT/uninstall-on.err" \
  && J '.model == "e2e/uninstall" and .removed and .cache_bytes_removed > 0' "$OUT/uninstall-on.json" \
  || check "uninstall --on local: e2e/uninstall not removed, or no bytes freed: $(head -c 400 "$OUT/uninstall-on.json") $(tail -c 200 "$OUT/uninstall-on.err")"
cozy --json model ls --on local > "$OUT/uninstall-on-models.json" && J 'all(.models[]; .model != "e2e/uninstall")' "$OUT/uninstall-on-models.json" \
  || check "uninstall --on local: model ls --on local still lists e2e/uninstall"
echo "uninstall --on: $(jq -c . "$OUT/uninstall-on.json")"
}

# xs-190: a model of one component (e2e/parts-vae, uploaded as `<file>:vae` by e2e/isolated.sh), composed at run time with another
# model's other components. Its card and install hold its `vae` alone; a run whose `vae` slot names it reads the default
# pipeline's transformer and its VAE, and its stored picture differs from the default's. xs-192: an unversioned pick of
# e2e/parts, whose 0.2.0 is 0.1.0's vae alone, is the whole 0.1.0 for an install and the part for a slot that reads it.
component_models() {
step "a model of one component, composed with another model's at run time"
local script=$here/script/parts.py default composed
cozy --json model show e2e/parts-vae > "$OUT/parts-show.json" 2>&1 \
  && J '[.lanes[].components[] | {name, bytes}] == [{name: "vae", bytes: 262144}]' "$OUT/parts-show.json" \
  || check "model show e2e/parts-vae: not its 256 KiB vae alone: $(jq -c '[.lanes[]?.components[]? | {name, bytes}]' "$OUT/parts-show.json" 2>/dev/null || head -c 300 "$OUT/parts-show.json")"
cozy --json model show e2e/parts@0.1.0 > "$OUT/parts-pipeline.json" 2>&1 && J '[.lanes[].components[].name] == ["transformer", "vae"]' "$OUT/parts-pipeline.json" \
  || check "model show e2e/parts@0.1.0: not a transformer and a VAE: $(jq -c '[.lanes[]?.components[]?.name]' "$OUT/parts-pipeline.json" 2>/dev/null || head -c 300 "$OUT/parts-pipeline.json")"
cozy --json model install e2e/parts-vae --on local > "$OUT/parts-install.ndjson" 2>&1 || check "model install e2e/parts-vae failed: $(tail -c 300 "$OUT/parts-install.ndjson")"
# Its 256 KiB VAE, never the pipeline's 4 MiB transformer.
cozy --json model ls --on local > "$OUT/parts-models.json" \
  && J 'any(.models[]; .model == "e2e/parts-vae" and .bytes >= 262144 and .bytes < 1048576)' "$OUT/parts-models.json" \
  || check "model install e2e/parts-vae: not its VAE alone on local: $(jq -c '[.models[]? | select(.model | startswith("e2e/parts")) | {model, bytes}]' "$OUT/parts-models.json")"
# An unversioned pick: an install reads the checkpoint whole, so it takes 0.1.0, never the part 0.2.0 (e2e/parts@0.1.0:vae).
cozy --json model install e2e/parts --on local > "$OUT/parts-whole.ndjson" 2>&1 || check "model install e2e/parts failed: $(tail -c 300 "$OUT/parts-whole.ndjson")"
cozy --json model ls --on local > "$OUT/parts-whole.json" \
  && J '[.models[] | select(.model == "e2e/parts") | .release] == ["0.1.0"]' "$OUT/parts-whole.json" \
  || check "model install e2e/parts (unversioned): not its whole 0.1.0: $(jq -c '[.models[]? | select(.model == "e2e/parts") | {release, bytes}]' "$OUT/parts-whole.json")"
cell parts-default "$script" || return 0
cell parts-composed "$script" --model vae=e2e/parts-vae@0.1.0 || return 0
default=$(jq -c .result "$OUT/parts-default.json") composed=$(jq -c .result "$OUT/parts-composed.json")
J '.result.transformer.part == "dit" and .result.vae.part == "vae-a"' "$OUT/parts-default.json" \
  || check "parts-default: not e2e/parts's transformer and VAE: $default"
J --argjson was "$default" '.result.transformer == $was.transformer and .result.vae.part == "vae-b" and .result.vae.sum != $was.vae.sum' "$OUT/parts-composed.json" \
  || check "parts-composed: not e2e/parts's transformer with e2e/parts-vae's VAE: $composed (default $default)"
# A slot that reads the part takes it, unversioned: `vae` names e2e/parts and resolves its newest holding a vae, 0.2.0.
if cell parts-newest "$script" --model vae=e2e/parts; then
  J '[.measurements.models[] | {slot, release}] | (any(.slot == "vae" and .release == "0.2.0") and any(.slot == "model" and .release == "0.1.0"))' "$OUT/parts-newest.json" \
    && J '.result.vae.part == "vae-a"' "$OUT/parts-newest.json" \
    || check "parts-newest: vae=e2e/parts is not its part 0.2.0: $(jq -c '[.measurements.models[]? | {slot, release}]' "$OUT/parts-newest.json")"
fi
local was is
was=$(output parts-default png) is=$(output parts-composed png)
{ [ -n "$was" ] && [ -n "$is" ] && ! cmp -s "$was" "$is"; } || check "parts: the composed run's stored picture is not another than the default's (${was:-none}, ${is:-none})"
}

# tfs-300, LAB2-47829 (Paul 11-16: "we don't need all components of a model to be on the critical path; that's just creating
# unnecessary latency for the end-user who is waiting on models to download"; "I think this is a failure in terms of our download
# planner"): a LoRA on qwen-image-2's `model`, which reads Noct Q's transformer alone, pulled all of Noct Q, its 9.9 GiB text
# encoder too, before the call could start. script/lora.py's `model` reads e2e/lora-base's 256 KiB transformer beside its 16 MiB
# text encoder: cold, with e2e/lora-proj on it, the call completes, the LoRA changes what the transformer makes, and the machine
# holds the transformer alone.
lora_reads() {
step "a LoRA'd slot downloads only the components it reads of its model (tfs-300)"
[ -n "$LOCAL" ] || return 0
local script=$here/script/lora.py held stage
cell lora-adapted "$script" --lora model=e2e/lora-proj || return 0
stage=$(grep '^{' "$OUT/lora-adapted.ndjson" | jq -s '[.[] | select(.type == "run.progress") | .progress | select(.stage | startswith("preparing model adapters")) | .bytes_total] | max // 0')
cozy --json model ls --on local > "$OUT/lora-held.json" || { check "lora reads: model ls --on local failed"; return; }
held=$(jq -c '[.models[]? | select(.model == "e2e/lora-base") | {complete, bytes}]' "$OUT/lora-held.json")
J '[.models[] | select(.model == "e2e/lora-base")] | length == 1 and .[0].complete == false and .[0].bytes >= 262144 and .[0].bytes < 4194304' "$OUT/lora-held.json" \
  || check "lora reads: the LoRA'd call left more of e2e/lora-base here than its 256 KiB transformer (its text encoder is 16 MiB): $held"
J '[.measurements.models[] | select(.slot == "model") | .adapters | length] == [1]' "$OUT/lora-adapted.json" \
  || check "lora reads: its record names no LoRA on model: $(jq -c '.measurements.models' "$OUT/lora-adapted.json")"
cell lora-plain "$script" || return 0
J --slurpfile plain "$OUT/lora-plain.json" '.result.sum != $plain[0].result.sum' "$OUT/lora-adapted.json" \
  || check "lora reads: the LoRA changed nothing: $(jq -c .result "$OUT/lora-adapted.json") as without it"
echo "lora reads: e2e/lora-base here after the LoRA'd call $held; its adapter stage moved ${stage} bytes; sums $(jq .result.sum "$OUT/lora-adapted.json") with, $(jq .result.sum "$OUT/lora-plain.json") without"
}

# ltx2.lora (the ltx-2.5 package's LoRAs, for the release coordinator: "`cozy model upload` / `cozy lora` / `cozy run --lora` can
# take an LTX-2.5 LoRA (including IC-LoRAs such as Lightricks' Ingredients) and store it as a LoRA whose component is
# `transformer`, with Diffusers' `LTX2VideoTransformer3DModel` module names and a PEFT adapter config"; Paul 11-16: "convert
# in-the-wild deviations ... to our canonical cozytensors format"). e2e/isolated.sh uploads Lightricks' LTX-2.3 Motion Track
# Control IC-LoRA from Hugging Face as published, LTX's native names under diffusion_model.: it is an ltx-2-5 LoRA of its
# transformer, its 480 modules under Diffusers' names (to_out.0), r and lora_alpha 32 (no .alpha: a scale of 1, as ltx-core applies it),
# and its reference_downscale_factor (2, its file's metadata) in the adapter config the package reads.
ltx_lora() {
step "a published LTX IC-LoRA is a LoRA of ltx-2-5's transformer in Diffusers' names"
local lora=lightricks/ltx-2.3-motion-track-control show=$OUT/ltx-lora-show.json
cozy --json --full lora show "$lora" > "$show" 2>&1 || { check "lora show $lora: $(tail -c 300 "$show")"; return; }
J '.family == "ltx-2-5" and .lanes[0].records.adapter.transformer == {family: "ltx-2-5", component: "transformer"}' "$show" \
  || check "$lora: not a LoRA of ltx-2-5's transformer: family $(jq -c '[.family, .lanes[0].records.adapter]' "$show")"
J '[.lanes[0].components[] | select(.tensors > 0) | .name] == ["transformer"]' "$show" \
  || check "$lora: not one component, transformer: $(jq -c '[.lanes[0].components[] | {name, tensors}]' "$show")"
J '.lanes[0].components[] | select(.name == "transformer") | .lora.modules == 480 and .lora.alphas == 0 and .config_file == "adapter_config.json"
   and .config.peft_type == "LORA" and .config.r == 32 and .config.lora_alpha == 32 and .config.reference_downscale_factor == 2
   and (.config.target_modules | length) == 480
   and (.config.target_modules | index("transformer_blocks.0.attn1.to_out.0") != null)
   and all(.config.target_modules[]; startswith("transformer_blocks."))' "$show" \
  || check "$lora: its transformer is not 480 modules in Diffusers' names at r = lora_alpha = 32, reference_downscale_factor 2: $(jq -c '.lanes[0].components[] | select(.name == "transformer") | {lora, config_file, r: .config.r, lora_alpha: .config.lora_alpha, downscale: .config.reference_downscale_factor, targets: (.config.target_modules // [] | .[:3])}' "$show")"
}

# se-141 (the ltx-2.5 package's agent, for picture and voice references: "TensorFS ingests LiconStudio's Licon MSR V2 LoRA for
# LTX-2.5 losslessly, so the ltx-2.5 package can run it"; Paul: canonical format, in-the-wild layouts converted at ingest, the
# package never maps keys). e2e/isolated.sh uploads it from Hugging Face as published: LTX's native names under diffusion_model.,
# 1152 rank-128 factor pairs (no .alpha) and two Fourier-MLP reference-slot embeddings the DiT has not. It is an ltx-2-5 LoRA of
# its transformer holding all 2314 tensors and every byte of them, the embeddings carried whole (PEFT's modules_to_save), and its
# reference_* metadata typed in the adapter config the package reads.
ltx_msr() {
step "LiconStudio's MSR is an ltx-2-5 LoRA that carries its reference-slot embeddings whole"
local lora=liconstudio/licon-msr-v2 show=$OUT/msr-show.json
cozy --json --full lora show "$lora" > "$show" 2>&1 || { check "lora show $lora: $(tail -c 300 "$show")"; return; }
J '.family == "ltx-2-5" and .lanes[0].records.adapter.transformer == {family: "ltx-2-5", component: "transformer"}' "$show" \
  || check "$lora: not a LoRA of ltx-2-5's transformer: $(jq -c '[.family, .lanes[0].records.adapter]' "$show")"
J '[.lanes[0].components[] | select(.tensors > 0) | {name, tensors, bytes}] == [{name: "transformer", tensors: 2314, bytes: 2214758976}]' "$show" \
  || check "$lora: not its 2314 tensors and 2214758976 bytes in transformer: $(jq -c '[.lanes[0].components[] | {name, tensors, bytes}]' "$show")"
J '.lanes[0].components[] | select(.name == "transformer") | .lora.modules == 1152 and .lora.alphas == 0
   and .config.r == 128 and .config.lora_alpha == 128 and (.config.target_modules | length) == 1152
   and (.config.target_modules | index("transformer_blocks.0.video_to_audio_attn.to_out.0") != null)
   and .config.modules_to_save == ["reference_audio_slot_embedding", "reference_slot_embedding"]
   and .config.reference_slot_embedding_enabled == true and .config.reference_audio_sparse_slots == true
   and .config.reference_slot_embedding_dim == 128 and .config.reference_audio_slot_embedding_num_frequencies == 16
   and .config.reference_audio_slot_duration_seconds == 5.0 and .config.reference_audio_end_margin_seconds == 0.04
   and .config.reference_slot_embedding_type == "fourier_mlp" and .config.reference_token_order == "prepend"' "$show" \
  || check "$lora: its adapter config is not 1152 modules at r = lora_alpha = 128, the two embeddings carried and the reference_* layout typed: $(jq -c '.lanes[0].components[] | select(.name == "transformer") | {lora, config: (.config // {} | del(.target_modules))}' "$show")"
}

# Paul's rule (11-13, cl-984): "1. project data is private 2. models / packages installed are machine-specific (not project
# specific)". After component_models ran parts.py (a script package on e2e/parts, ~4 MiB) in the suite's project, a second
# project's first run of it downloads nothing, builds no environment and adds no weights anywhere, and the first project's
# runs, inputs and outputs are not the second's.
second_project() {
step "a second project's first run of a package and model this machine holds (cl-984)"
[ -n "$LOCAL" ] || return 0
local script=$here/script/parts.py other=$OUT/second-project before after
envs_in() { find "$@" -maxdepth 10 \( -name 'env-*' -type d -o -path '*/generations/*/env' \) 2> /dev/null | sort; }
cozy --json new --unencrypted "$other" > "$OUT/second-new.json" 2>&1 || { check "second project: cozy new --unencrypted: $(tail -c 300 "$OUT/second-new.json")"; return; }
in_project "$RUN_PROJECT" --json run ls --limit 0 > "$OUT/second-first-runs.json" || check "second project: run ls in the first project failed"
before=$(envs_in "$HOME/.cozy")
RUN_PROJECT=$other cell second-parts "$script" || return 0
after=$(envs_in "$HOME/.cozy")
# Nothing downloaded or installed: no `downloading` stage moved a byte, none installed dependencies.
grep '^{' "$OUT/second-parts.ndjson" | jq -e -s '[.[] | select(.type == "run.progress") | .progress
    | select(((.stage | test("downloading")) and ((.stage | test("the result")) | not) and ((.bytes_done // 0) > 0))
      or (.stage | test("installing dependencies|installing this machine")))] | length == 0' > /dev/null \
  || check "second project: its first run downloaded or installed: $(grep '^{' "$OUT/second-parts.ndjson" | jq -r 'select(.type == "run.progress") | .progress.stage' | sort -u | grep -E 'download|install' | head -3 | paste -sd';' -)"
[ "$before" = "$after" ] || check "second project: the machine gained environments: $(comm -13 <(echo "$before") <(echo "$after") | sed "s#$HOME/##" | paste -sd' ' - | cut -c1-400)"
# Its own area holds no environment, PyTorch, uv cache, Python or weights: those are the machine's.
local held
held=$(find "$other/.vault-machine" -maxdepth 4 \( -name 'env-*' -type d -o -name 'torch-*' -o -name uv-cache -o -name python -type d \) 2> /dev/null | head -3)
[ -z "$held" ] || check "second project: its .vault-machine holds installs: $held"
held=$(du -sk "$other/.vault-machine/objects" 2> /dev/null | cut -f1)
[ "${held:-0}" -lt 2048 ] || check "second project: its store holds ${held} KiB, the model's weights among them"
# Each project's history is its own: the second lists only its parts run, none of the first's other work.
in_project "$other" --json run ls --limit 0 > "$OUT/second-runs.json" \
  && J '([.runs[]?] | length) >= 1 and ([.runs[]? | select(.target | test("parts") | not)] | length) == 0' "$OUT/second-runs.json" \
  || check "second project: run ls holds more than its own run: $(jq -c '[.runs[]? | {number, target}]' "$OUT/second-runs.json" 2> /dev/null | head -c 300)"
J '[.runs[]? | select(.target | test("parts") | not)] | length >= 1' "$OUT/second-first-runs.json" \
  || check "second project: the first project lists none of its other runs: $(jq -c '[.runs[]? | .target]' "$OUT/second-first-runs.json" 2> /dev/null | head -c 300)"
echo "second project: machine environments $(echo "$before" | grep -c .) then $(echo "$after" | grep -c .); its store $(du -sh "$other/.vault-machine/objects" 2> /dev/null | cut -f1)"
}

# cl-1011: a lock takes no release the machines can't install. A local index holds a dependency at 1.0 (a wheel for any platform)
# and 2.0 (a macOS wheel alone, as a release still uploading to PyPI is): local code needing it locks 1.0 and runs here.
lock_platforms() {
step "a lock passes over a release no rental can install (cl-1011)"
[ -n "$LOCAL" ] || return 0
local L=$OUT/lock-platforms
rm -rf "$L" && mkdir -p "$L/index" && cp -r "$here/weightless" "$L/code" || { check "lock platforms: no copy of weightless"; return; }
python3 - "$L/index" <<'PY' || { check "lock platforms: no index"; return; }
import base64, hashlib, sys, zipfile
from pathlib import Path
index = Path(sys.argv[1])
def wheel(version: str, tag: str) -> None:
    name, info = "e2e_cl1011_dep", f"e2e_cl1011_dep-{version}.dist-info"
    files = {f"{name}/__init__.py": f'VERSION = "{version}"\n',
             f"{info}/METADATA": f"Metadata-Version: 2.1\nName: e2e-cl1011-dep\nVersion: {version}\n",
             f"{info}/WHEEL": f"Wheel-Version: 1.0\nGenerator: e2e\nRoot-Is-Purelib: {'true' if tag == 'py3-none-any' else 'false'}\nTag: {tag}\n"}
    record = [f"{path},sha256={base64.urlsafe_b64encode(hashlib.sha256(body.encode()).digest()).rstrip(b'=').decode()},{len(body.encode())}" for path, body in files.items()]
    files[f"{info}/RECORD"] = "\n".join(record + [f"{info}/RECORD,,"]) + "\n"
    with zipfile.ZipFile(index / f"e2e_cl1011_dep-{version}-{tag}.whl", "w") as out:
        for path, body in files.items():
            out.writestr(path, body)
wheel("1.0", "py3-none-any")
wheel("2.0", "py3-none-macosx_11_0_arm64")
PY
sed -i 's/^name = "e2e-weightless"/name = "e2e-lock-platforms"/; s/"msgspec>=0.19"\]/"msgspec>=0.19", "e2e-cl1011-dep"]/' "$L/code/pyproject.toml"
printf '\n[[tool.uv.index]]\nname = "e2e-cl1011"\nurl = "file://%s"\nformat = "flat"\n\n[tool.uv.sources]\ne2e-cl1011-dep = { index = "e2e-cl1011" }\n' "$L/index" >> "$L/code/pyproject.toml"
cat >> "$L/code/e2e_weightless/__init__.py" <<'PY'


class Dependency(msgspec.Struct):
    version: str


@app.entrypoint(accelerator=False)
def dependency() -> Dependency:
    import e2e_cl1011_dep
    return Dependency(version=e2e_cl1011_dep.VERSION)
PY
in_project "$RUN_PROJECT" --json run "$L/code/dependency" --on local > "$L/run.ndjson" 2> "$L/run.err" \
  || { check "lock platforms: the run failed, a lock that took 2.0 (a macOS wheel alone)?: $(tail -c 300 "$L/run.err")"; return; }
grep -q '"version": *"1.0"' "$L/run.ndjson" || check "lock platforms: the dependency is not 1.0: $(grep -o '"version": *"[^"]*"' "$L/run.ndjson" | tail -1)"
echo "lock platforms: $(grep -o '"version": *"[^"]*"' "$L/run.ndjson" | tail -1) of 1.0 (any) and 2.0 (macOS alone)"
}

# safetensors_file <file>: a 4×4 F16 .safetensors file.
safetensors_file() {
uv run -q --no-project python -I - "$1" <<'PY'
import json, struct, sys
header = json.dumps({"w": {"dtype": "F16", "shape": [4, 4], "data_offsets": [0, 32]}}).encode()
header += b" " * (-len(header) % 8)
open(sys.argv[1], "wb").write(struct.pack("<Q", len(header)) + header + bytes(32))
PY
}

# Where this OS runs no models itself (macOS, Windows): every verb that would use this computer's machine refuses
# `local_unavailable` (exit 1) with the rental to use instead, before anything is staged or started; nothing ever
# starts a machine here or makes its root; `machine ls` and `status` say so in one line; doctor is healthy.
refusals() {
step "local runs refused"
dog=$(picture dog2.jpg) key=AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA
printf 'image: %s\n' "$dog" > "$OUT/refused.yaml"
safetensors_file "$OUT/refused.safetensors"
refused() {  # refused <name> <cozy args...>
  local name=$1 code=0; shift
  cozy --json "$@" > "$OUT/refused-$name.json" 2> "$OUT/refused-$name.err" || code=$?
  [ "$code" = 1 ] && J '.error.code == "local_unavailable" and (.error.next | any(contains("--on new")))' "$OUT/refused-$name.json" \
    || check "cozy $*: exit $code, not refused local_unavailable: $(head -c 300 "$OUT/refused-$name.json") $(head -c 200 "$OUT/refused-$name.err")"
}
refused run run birefnet/remove_background --file "image=$dog"
refused run-on-local run birefnet/remove_background --input "$OUT/refused.yaml" --on local
(refused run-code run "$here/weightless/reverse" text=refused)
# Describing local code reads its source here and uses no machine: it works where none runs.
(in_project "$RUN_PROJECT" --json run "$here/weightless/reverse" --describe > "$OUT/describe-code.json" 2> "$OUT/describe-code.err") \
  && J '.fields | any(.name == "text")' "$OUT/describe-code.json" \
  || check "cozy run ./weightless/reverse --describe: $(tail -c 300 "$OUT/describe-code.json") $(tail -c 200 "$OUT/describe-code.err")"
# Outside any project, local code is the machine's, on every OS (cl-990).
mkdir -p "$OUT/outside-projects"
(cd "$OUT/outside-projects" && cozy --json run "$here/weightless/reverse" --describe > "$OUT/describe-outside.json" 2> "$OUT/describe-outside.err") \
  && J '.fields | any(.name == "text")' "$OUT/describe-outside.json" \
  || check "outside a project: cozy run ./weightless/reverse --describe: $(tail -c 300 "$OUT/describe-outside.json") $(tail -c 200 "$OUT/describe-outside.err")"
(cd "$OUT/outside-projects" && refused install-code-outside package install "$here/weightless")
refused install package install birefnet
refused install-code package install "$here/weightless"
refused remove package uninstall birefnet
refused download model install zhengpeng7/birefnet
refused upload model upload "$OUT/refused.safetensors" fidika/e2e-refused --lane f16
refused quantize model quantize circlestone-labs/anima --lane int8 --package anima
refused update machine update local --runtime 0.30.0 -y
refused show machine show local
refused logs machine logs local --lines 5
refused authorize machine authorize "$key"
refused doctor doctor --on local
cozy run birefnet/remove_background --file "image=$dog" > "$OUT/refused.txt" 2>&1 && check "a local run without --on was not refused"
grep -q "This computer can't run models on" "$OUT/refused.txt" && grep -q -- '--on new' "$OUT/refused.txt" \
  || check "the refusal does not say this computer can't run models and --on new: $(head -c 300 "$OUT/refused.txt")"

step "no local machine"
cozy --json machine ls > "$OUT/machines.json" && J '.machines | all(.name != "local")' "$OUT/machines.json" || check "machine ls: a local machine is listed"
cozy machine ls > "$OUT/machines.txt" 2>&1 && grep -q "This computer can't run models on" "$OUT/machines.txt" || check "machine ls does not say this computer can't run models"
fixture_calcifer > "$OUT/status.txt" && J '.calcifer.local_usable == false' "$OUT/status.txt" || check "status does not say local runs aren't available"
cozy --json doctor > "$OUT/doctor-gated.json" && J '.healthy and (.checks | any(.check == "machine" and (.state | contains("can'"'"'t run models"))))' "$OUT/doctor-gated.json" \
  || check "doctor: not healthy, or no line that local runs aren't available: $(jq -c .checks "$OUT/doctor-gated.json")"
cozy machine logs local calcifer --lines 5 > "$OUT/log-calcifer.txt" && [ -s "$OUT/log-calcifer.txt" ] || check "machine logs local calcifer: no Calcifer log"
cozy --json package ls --on all > "$OUT/packages.json" || check "package ls --on all failed"
cozy --json model ls --on all > "$OUT/models.json" || check "model ls --on all failed"
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

# cl-716 B: a run asked unbound, which Calcifer binds: a video's frames cut by the box mention that names them
# (`<box:48-144>`, cl-819: `path#t=` went when ranges became mentions), a picture and a folder; a file uploaded by its
# digest and named by it; a digest Calcifer does not hold refused; and the release's CLI, which builds its run itself,
# refused as outdated. Fixtures by ffmpeg (the job's, on this shell's PATH).
bound_runs() {
step "a run Calcifer binds: a clip cut, a picture, a folder"
local dir=$OUT/bound lock=$HOME/.cozy/calcifer/calcifer.lock.holder addr token hex size code before
mkdir -p "$dir/folder/sub"; echo one > "$dir/folder/a.txt"; echo two > "$dir/folder/sub/b.txt"
ffmpeg -v error -y -f lavfi -i testsrc2=size=320x240:rate=24:duration=8 -c:v libx264 -pix_fmt yuv420p -g 24 "$dir/clip.mp4"
ffmpeg -v error -y -f lavfi -i "color=c=teal:size=64x64" -frames:v 1 "$dir/still.png"
size=$(stat -c %s "$dir/still.png")
[ -n "$(row "$RUN_PROJECT" bound_clip .name)" ] || project_import "$RUN_PROJECT" "$dir/clip.mp4" bound_clip > /dev/null || check "bound: import the clip"
for _ in $(seq 60); do [ "$(row "$RUN_PROJECT" bound_clip .duration_ms)" != null ] && break; sleep 0.5; done
# Frames 48 up to 144 at 24 fps: 2 to 6 s.
if (cd "$dir" && cell bound "$here/inspect/inspect" 'clip=<bound_clip:48-144>' --file "still=$dir/still.png" "folder=$dir/folder" label=bound); then
  grep -qE '"clip_seconds": *(3\.9|4\.0|4\.1)' "$OUT/bound.ndjson" && grep -qE "\"still_bytes\": *$size" "$OUT/bound.ndjson" \
    && grep -qE '"folder_files": *\[ *"a.txt", *"sub/b.txt" *\]' "$OUT/bound.ndjson" \
    || check "bound: not a 4 s clip, the picture and the folder's two files: $(grep -oE '"(clip_seconds|still_bytes|folder_files)": *[^,}]*' "$OUT/bound.ndjson" | head -3 | paste -sd' ')"
fi
cozy --json run "$here/inspect/inspect" --describe --on local > "$OUT/bound-describe.json" 2> "$OUT/bound-describe.err" \
  && grep -q '"clip"' "$OUT/bound-describe.json" && grep -q '"folder"' "$OUT/bound-describe.json" \
  || check "run inspect --describe: no clip or folder input: $(tail -c 300 "$OUT/bound-describe.err")"

step "a file uploaded by its digest, and one Calcifer does not hold"
addr=$(jq -r .address "$lock") token=$(jq -r .token "$lock") hex=$(sha256sum "$dir/still.png" | cut -c1-64)
code=$(curl -sS -o "$OUT/put.json" -w '%{http_code}' -X PUT -H "Authorization: Bearer $token" --data-binary "@$dir/still.png" "http://$addr/v1/files/sha256:$hex")
# 200: the CLI sent it already, for the run above.
{ [ "$code" = 201 ] || [ "$code" = 200 ]; } || check "PUT /v1/files/sha256:$hex is not 201 or 200: $(head -c 300 "$OUT/put.json")"
[ "$(curl -sS -o /dev/null -w '%{http_code}' -I -H "Authorization: Bearer $token" "http://$addr/v1/files/sha256:$hex")" = 200 ] || check "HEAD of an uploaded file is not 200"
if (cd "$dir" && cell bound-digest "$here/inspect/inspect" --file "clip=$dir/clip.mp4" --file "still=sha256:$hex" "folder=$dir/folder" label=digest); then
  grep -qE "\"still_bytes\": *$size" "$OUT/bound-digest.ndjson" || check "bound-digest: the run was not given the uploaded picture"
fi
before=$(cozy --json run ls --limit 0 | jq '.runs | length')
(cd "$dir" && ALLOW=file_not_held cell bound-unheld "$here/inspect/inspect" --file "clip=$dir/clip.mp4" --file "still=sha256:$(head -c 64 /dev/urandom | sha256sum | cut -c1-64)" "folder=$dir/folder") || true
{ [ -f "$OUT/bound-unheld.refused" ] && [ "$(cozy --json run ls --limit 0 | jq '.runs | length')" = "$before" ]; } \
  || check "a digest Calcifer does not hold was not refused file_not_held before any run"

step "the release's CLI against this Calcifer"
# Only a cozy before 0.16.3 builds its run itself. A release dispatch installs the candidate straight away and keeps no
# previous cozy, and from 0.16.3 on the previous one asks as this one does: then there is nothing to refuse.
local previous=$HOME/.cozy/bin/.cozy-previous old=''
[ -x "$previous" ] && old=$("$previous" --version 2>/dev/null | cut -d' ' -f2) || true
if [ -z "$old" ] || [ "$(printf '%s\n0.16.3\n' "${old%%+*}" | sort -V | head -1)" = 0.16.3 ]; then
  echo "skipped: no cozy here that builds its own run (previous: ${old:-none})"
else
  code=0; (cd "$here" && "$previous" run ./weightless/reverse text=old --on local) > "$OUT/old-cli.txt" 2>&1 || code=$?
  cat "$OUT/old-cli.txt"
  { [ "$code" != 0 ] && grep -q client_outdated "$OUT/old-cli.txt" && [ "$(cozy --json run ls --limit 0 | jq '.runs | length')" = "$before" ]; } \
    || check "the release's CLI ($old) was not refused client_outdated, or its run was recorded (exit $code)"
fi
}

# A run's own references: `<Name>` in a direct run's prompt that names one of its `references:=[{name, image}]` sends that
# picture (H3's form); a name that names nothing is still refused, with the command that makes it a box.
own_references() {
step "a run's own references: the <name> its request gives"
local dir=$OUT/own-refs code=0
rm -rf "$dir"; mkdir -p "$dir"
ffmpeg -v error -y -f lavfi -i "color=c=teal:size=48x32" -frames:v 1 "$dir/amy.png"
if (cd "$dir" && cell own-refs "$here/project/mentions.py/mentions" 'instructions=<Amy> waves at <Amy>' "references:=$(jq -cn --arg image "$dir/amy.png" '[{name: "Amy", image: $image}]')"); then
  grep -qE '"keys": *\[ *"Amy" *\]' "$OUT/own-refs.ndjson" && grep -qE '"widths": *\[ *48 *\]' "$OUT/own-refs.ndjson" \
    || check "own references: <Amy> did not send its picture: $(grep -oE '"(text|keys|widths)": *("[^"]*"|\[[^]]*\])' "$OUT/own-refs.ndjson" | head -3 | paste -sd' ')"
fi
(cd "$dir" && in_project "$RUN_PROJECT" --json run "$here/project/mentions.py/mentions" 'instructions=<Nobody> waves' --on local) > "$OUT/own-refs-nothing.json" 2>&1 || code=$?
[ "$code" != 0 ] && grep -q input_missing "$OUT/own-refs-nothing.json" && grep -q 'cozy open' "$OUT/own-refs-nothing.json" \
  || check "own references: <Nobody> was not refused with the command that makes it a box (exit $code): $(tail -c 300 "$OUT/own-refs-nothing.json")"
# xs-191: a relative path in an --input file starts at that file's folder, the run's working directory being the project:
# in/run.yaml names amy.png beside it. One written from elsewhere (se-131's inputs/edit.json named inputs/meadow-ref.jpg)
# is refused saying where a relative path starts.
mkdir -p "$dir/in"; cp "$dir/amy.png" "$dir/in/amy.png"
printf 'instructions: "<Amy> waves"\nreferences:\n  - name: Amy\n    image: amy.png\n' > "$dir/in/run.yaml"
if cell own-refs-input "$here/project/mentions.py/mentions" --input "$dir/in/run.yaml"; then
  grep -qE '"keys": *\[ *"Amy" *\]' "$OUT/own-refs-input.ndjson" && grep -qE '"widths": *\[ *48 *\]' "$OUT/own-refs-input.ndjson" \
    || check "own references: in/run.yaml's amy.png, beside it, did not send its picture: $(grep -oE '"(text|keys|widths)": *("[^"]*"|\[[^]]*\])' "$OUT/own-refs-input.ndjson" | head -3 | paste -sd' ')"
fi
printf 'instructions: "<Amy> waves"\nreferences:\n  - name: Amy\n    image: in/amy.png\n' > "$dir/in/elsewhere.yaml"
code=0; in_project "$RUN_PROJECT" --json run "$here/project/mentions.py/mentions" --input "$dir/in/elsewhere.yaml" --on local > "$OUT/own-refs-elsewhere.json" 2>&1 || code=$?
[ "$code" != 0 ] && grep -q reference_file_missing "$OUT/own-refs-elsewhere.json" && grep -q "starts at that file's folder" "$OUT/own-refs-elsewhere.json" \
  || check "own references: in/elsewhere.yaml's in/amy.png (in/in/amy.png) wasn't refused saying where it starts (exit $code): $(tail -c 300 "$OUT/own-refs-elsewhere.json")"
}

# xs-191: a lane fits on the components its slot reads, not its whole checkpoint (madebyollin/texture-fix-vae@0.2.0 is
# Qwen-Image-2.1 with its VAE replaced; its `vae` slot warned that 30.8 GiB doesn't fit for 1.35 GB). TensorD asks the Hub
# with its GPUs, their free bytes and the slot's components; a CPU runner has no GPUs to send, so the cell asks it the same
# way. isolated.sh's fitting/vae is an 8 MiB `model` and a 1 MiB `vae`: with 4 MiB free the whole checkpoint doesn't fit,
# its `vae` does, and its `model` still doesn't.
lane_fit() {
step "a lane fits on the components its slot reads, not its whole checkpoint (xs-191)"
[ -n "${E2E_FIT:-}" ] || { check "lane fit: isolated.sh published no fitting/vae"; return; }
local ask="${COZY_HUB_URL:?}/v1/models/$E2E_FIT/resolve?release=0.1.0&gpus=8.6&free_vram=$((4 << 20))" part
for part in whole vae model; do
  curl -fsS "$ask$([ "$part" = whole ] || echo "&components=$part")" > "$OUT/fit-$part.json" 2>&1 \
    || { check "lane fit: resolving $E2E_FIT for its $part failed: $(head -c 300 "$OUT/fit-$part.json")"; return; }
done
J '.reasons | any(startswith("no lane of fitting/vae@0.1.0 fits"))' "$OUT/fit-whole.json" \
  || check "lane fit: the whole 9 MiB checkpoint wasn't told it doesn't fit 4 MiB: $(jq -c .reasons "$OUT/fit-whole.json")"
J '.reasons == []' "$OUT/fit-vae.json" \
  || check "lane fit: its 1 MiB vae was told it doesn't fit 4 MiB: $(jq -c .reasons "$OUT/fit-vae.json")"
J '.reasons | any(startswith("no lane of fitting/vae@0.1.0'"'"'s model fits"))' "$OUT/fit-model.json" \
  || check "lane fit: its 8 MiB model wasn't told it doesn't fit 4 MiB: $(jq -c .reasons "$OUT/fit-model.json")"
}

# cl-900: descriptions are off for now (tools §9). A described picture's mention reaches its function with an empty description, and the
# description stays in the project.
no_descriptions() {
step "a mention reaches its function with an empty description, and the stored one stays"
local dir=$OUT/no-desc
mkdir -p "$dir"
ffmpeg -v error -y -f lavfi -i "color=c=orange:size=40x24" -frames:v 1 "$dir/described.png"
[ -n "$(row "$RUN_PROJECT" described .name)" ] || project_import "$RUN_PROJECT" "$dir/described.png" described > /dev/null || { check "descriptions: import"; return; }
project_api "$RUN_PROJECT" /assets/described -X PATCH -H 'Content-Type: application/json' -d '{"fields": {"description": "An orange card."}}' > "$OUT/no-desc-set.json" \
  || { check "descriptions: describe: $(head -c 300 "$OUT/no-desc-set.json")"; return; }
if cell no-desc "$here/project/mentions.py/mentions" 'instructions=<described> waves'; then
  cozy --json --full run watch "$(cat "$OUT/no-desc.number")" --once | jq -c .request.input.instructions.references > "$OUT/no-desc-refs.json"
  J 'length == 1 and .[0].key == "described" and .[0].description == "" and (.[0].file | startswith("sha256:"))' "$OUT/no-desc-refs.json" \
    || check "descriptions: <described> reached its function as $(cat "$OUT/no-desc-refs.json")"
fi
[ "$(row "$RUN_PROJECT" described .fields.description)" = '"An orange card."' ] || check "descriptions: the stored one is now $(row "$RUN_PROJECT" described .fields)"
}

# se-138 (Paul's clamp-and-warn rule): a size or count that isn't one of a choice's values runs at the nearest one and says
# so, never at a default larger than asked (cr-299's `to: 768p` ran at 1440p); a word that names none goes as its default.
choice_clamp() {
step "a choice given a value it doesn't offer: the nearest, said"
# `to=768p` is typed by `cozy run` as a resolution (runs/bind.rs `tiers`); a word it doesn't offer is refused there, so `mode` goes as JSON.
if cell choice "$here/project/choice.py/size" to=768p megapixels:=3 'mode:="slow"'; then
  grep -q 'to=1080p megapixels=2 mode=full' "$OUT/choice.ndjson" \
    || check "choice: 768p and 3 didn't run as 1080p and 2, or slow not as full: $(grep -oE '"given": *"[^"]*"' "$OUT/choice.ndjson" | head -1)"
  cozy --json --full run watch "$(cat "$OUT/choice.number")" --once > "$OUT/choice.json" 2>&1
  local said
  for said in "takes 1080p, 1440p or 2160p, not 768p, so Cozy used 1080p." "takes 2 or 4, not 3, so Cozy used 2." "takes fast or full, not slow, so Cozy used the default."; do
    grep -qF "$(jq -Rr '@json' <<< "$said" | sed 's/^"//; s/"$//')" "$OUT/choice.json" "$OUT/choice.ndjson" "$OUT/choice.err" 2> /dev/null \
      || grep -qF "$said" "$OUT/choice.err" 2> /dev/null || check "choice: the run didn't say: $said"
  done
fi
}

# cl-883: an orbit mentioned by angle. A dropped video, turned to an orbit as its box's Orbit switch does (PATCH its version's
# metadata), reaches a function as the frame at an angle, a picture, and as the frames turned between two, a clip:
# `floor(angle / 360 × 48)`, wrapping past 360° and across 0°. Its frames are greys, frame n at 5n, so the function says which
# it got. An angle of a video that isn't an orbit is refused before any run, naming Orbit.
orbit_angles() {
step "an orbit mentioned by angle: a frame as a picture, a turn as a clip"
local dir=$OUT/angles before code=0 turned='[range(12; 24)], [range(40; 48), range(0; 8)]'
mkdir -p "$dir"
ffmpeg -v error -y -f lavfi -i "nullsrc=s=64x64:r=24:d=2,format=gray,geq=lum='N*5'" -c:v libx264 -crf 10 -g 1 -pix_fmt yuv420p "$dir/spin.mp4"
ffmpeg -v error -y -f lavfi -i testsrc2=size=64x64:rate=24:duration=1 -c:v libx264 -pix_fmt yuv420p "$dir/plain.mp4"
for name in spin plain; do
  [ -n "$(row "$RUN_PROJECT" "angles_$name" .name)" ] || project_import "$RUN_PROJECT" "$dir/$name.mp4" "angles_$name" > /dev/null || check "angles: import $name"
done
for _ in $(seq 60); do [[ $(row "$RUN_PROJECT" angles_spin .duration_ms) =~ ^[0-9]+$ ]] && break; sleep 0.5; done
project_api "$RUN_PROJECT" /assets/angles_spin/versions/0 -X PATCH -H 'Content-Type: application/json' \
  -d '{"metadata": {"orbit": {"degrees": 360, "direction": "cw", "loops": true}}}' > "$OUT/angles-orbit.json" \
  || check "angles: Orbit not turned on: $(head -c 300 "$OUT/angles-orbit.json")"
if cell angles "$here/project/angles.py/angles" 'pictures:=["<angles_spin:120deg>", "<angles_spin:370deg>"]' \
  'clips:=["<angles_spin:90deg-180deg>", "<angles_spin:300deg-60deg>"]' 'prompt=<angles_spin:120deg> turns through <angles_spin:90deg-180deg>'; then
  J ".result | .pictures == [16, 1] and .clips == [$turned] and .keys == [\"angles_spin:120deg\", \"angles_spin:90deg-180deg\"] and .kinds == [\"image\", \"video\"]" "$OUT/angles.json" \
    || check "angles: not frames 16 and 1, frames 12-23 and 40-47 + 0-7, keyed by angle: $(jq -c .result "$OUT/angles.json" | head -c 600)"
fi
before=$(cozy --json run ls --limit 0 | jq '.runs | length')
in_project "$RUN_PROJECT" --json run "$here/project/angles.py/angles" 'pictures:=["<angles_plain:120deg>"]' --on local > "$OUT/angles-plain.json" 2>&1 || code=$?
{ [ "$code" != 0 ] && grep -q mention_not_orbit "$OUT/angles-plain.json" && grep -q 'Turn on Orbit' "$OUT/angles-plain.json" \
  && [ "$(cozy --json run ls --limit 0 | jq '.runs | length')" = "$before" ]; } \
  || check "angles: an angle of a video that isn't an orbit was not refused mention_not_orbit before any run (exit $code): $(tail -c 300 "$OUT/angles-plain.json")"
}

# cl-854: a lock on Runtime 0.43 can't speak a 0.23 machine's executor protocol. One whose range takes the machine's
# Runtime runs on it, its install saying to re-lock; one whose range refuses it installs nothing, refused in one line
# naming the re-lock. Both locks are PyPI's 0.43.1 and TensorFS 0.22.1. On the open RC's Runtime (a dev release) an RC build
# meets any cap (cl-952): both run on the machine's pair, and nothing is said of their locks.
old_runtime() {
step "a lock on an older executor protocol: the machine's pair where its range takes it, else one line naming the re-lock"
local code=0 runtime unspoken="can't speak this machine's executor protocol"
runtime=$(cozy --json machine show local | jq -r '.status.runtime // empty')
if [[ $runtime =~ ^[0-9]+(\.[0-9]+)*\.dev[0-9]+(\+.*)?$ ]]; then
  if cell old-runtime "$here/old-runtime/takes/echo" text=old; then
    J --arg r "$runtime" '.result.text == "old" and .measurements.software.runtime == $r' "$OUT/old-runtime.json" && ! grep -q "$unspoken" "$OUT/old-runtime.err" \
      || check "old runtime: on the RC's Runtime $runtime its lock didn't run on the machine's pair, said nothing: $(jq -c '{result, runtime: .measurements.software.runtime}' "$OUT/old-runtime.json") $(grep -m1 "$unspoken" "$OUT/old-runtime.err" | head -c 300)"
  fi
  in_project "$RUN_PROJECT" --json run "$here/old-runtime/refuses/echo" text=old --on local > "$OUT/old-runtime-refused.json" 2>&1 || code=$?
  [ "$code" = 0 ] && grep '"type":"run.completed"' "$OUT/old-runtime-refused.json" | J '.run.result.text == "old"' && ! grep -q "$unspoken" "$OUT/old-runtime-refused.json" \
    || check "old runtime: on the RC's Runtime $runtime a range capped below it didn't run, said nothing (exit $code): $(tail -c 400 "$OUT/old-runtime-refused.json")"
  return
fi
if cell old-runtime "$here/old-runtime/takes/echo" text=old; then
  grep -q "$unspoken.*uv lock --upgrade-package cozy-runtime" "$OUT/old-runtime.err" \
    || check "old runtime: its install doesn't say it runs on the machine's pair, or how to re-lock: $(grep -m1 warning "$OUT/old-runtime.err" | head -c 400)"
fi
in_project "$RUN_PROJECT" --json run "$here/old-runtime/refuses/echo" text=old --on local > "$OUT/old-runtime-refused.json" 2>&1 || code=$?
[ "$code" != 0 ] && grep -q runtime_protocol_unspoken "$OUT/old-runtime-refused.json" && grep -q 'uv lock --upgrade-package cozy-runtime' "$OUT/old-runtime-refused.json" \
  || check "old runtime: a range refusing the machine's Runtime wasn't refused naming the re-lock (exit $code): $(tail -c 400 "$OUT/old-runtime-refused.json")"
}

# cl-779: Calcifer's ffmpeg and ffprobe read only the file they are given. A playlist and a concat list, each named as a
# clip, name a FIFO beside them; a reader opening it lets a waiting writer finish. Imported (a probe, a thumbnail) before
# a real clip whose probe is awaited, neither may open it. Today the import's own sniff refuses both ("no picture, video
# or sound") before ffmpeg sees them; the whitelist stands behind that.
hostile_media() {
step "media naming another file: nothing else is read"
local dir=$OUT/hostile name
rm -rf "$dir"; mkdir -p "$dir"; mkfifo "$dir/marker"
printf '#EXTM3U\n#EXT-X-TARGETDURATION:1\n#EXTINF:1,\n%s\n#EXT-X-ENDLIST\n' "$dir/marker" > "$dir/playlist.mp4"
printf "ffconcat version 1.0\nfile '%s'\n" "$dir/marker" > "$dir/list.mp4"
ffmpeg -v error -y -f lavfi -i testsrc2=size=160x120:rate=24:duration=2 -c:v libx264 -pix_fmt yuv420p "$dir/control.mp4"
(timeout 120 sh -c 'echo read > "$1"' _ "$dir/marker" && touch "$dir/opened") &
for name in playlist list control; do
  project_import "$RUN_PROJECT" "$dir/$name.mp4" "hostile_$name" > "$OUT/hostile-$name.json" 2>&1 || true
done
for _ in $(seq 60); do [[ $(row "$RUN_PROJECT" hostile_control .duration_ms) =~ ^[0-9]+$ ]] && break; sleep 0.5; done
sleep 5
[[ $(row "$RUN_PROJECT" hostile_control .duration_ms) =~ ^[0-9]+$ ]] || check "hostile media: the control clip was never probed: $(tail -c 200 "$OUT/hostile-control.json")"
if [[ $(uname -s) == Linux ]]; then
  # A literal questionmark is a valid POSIX project path, not an image sequence pattern.
  local literal; literal=$(mktemp -d "$RUNNER_TEMP/hostile?image.XXXXXX")
  ffmpeg -v error -y -f lavfi -i color=c=blue:size=8x8 -frames:v 1 "$dir/image.png"
  cozy --json new --unencrypted "$literal" > "$OUT/hostile-questionmark-new.json" 2>&1 || check "literal image: project creation failed"
  project_import "$literal" "$dir/image.png" literal_image > "$OUT/hostile-questionmark-import.json" 2>&1 || check "literal image: import failed"
  for _ in $(seq 60); do
    in_project "$literal" --json --full status > "$OUT/hostile-questionmark.json" 2> "$OUT/hostile-questionmark.err" || true
    J 'any(.project.assets[]; .name == "literal_image" and .width == 8 and .height == 8)' "$OUT/hostile-questionmark.json" && break
    sleep 0.5
  done
  J 'any(.project.assets[]; .name == "literal_image" and .width == 8 and .height == 8)' "$OUT/hostile-questionmark.json" \
    || check "literal image: the CLI never measured its PNG in the questionmark project: $(tail -c 300 "$OUT/hostile-questionmark.json")"
fi
[ ! -e "$dir/opened" ] || check "hostile media: ffmpeg or ffprobe opened a file an imported playlist or concat list names"
echo "hostile media: playlist $(tail -c 160 "$OUT/hostile-playlist.json"); list $(tail -c 160 "$OUT/hostile-list.json")"
}

# cl-817, cl-819: the CLI in a container whose /work only it has, Calcifer out here, sharing no disk: it sees only Calcifer's lock
# record (its address and bearer) and config.toml, copied into a HOME of its own. Each verb goes by content, never a path:
# a clip, a picture and a folder (a Tree), --input naming files beside it, a positional file; local code from /work (a package
# and a script) run, described and listed, installed, locked (its uv.lock written back into /work) and published (refused only
# for want of a sign-in), and quantize's recipe package; a project made and named by its name; import and --into; a run's
# outputs downloaded by their digest; a model from a .safetensors file; and a Runtime wheel for the machine.
unseen() {
step "the CLI in a folder Calcifer can't see"
[ -n "$LOCAL" ] && command -v docker > /dev/null || { echo "skipped: no local machine or no docker"; return 0; }
# The runner's own OS: a candidate cozy is built on it, for its glibc (debian:12's 2.36 runs only a release's).
local src=$OUT/unseen seen=$OUT/unseen-seen image=ubuntu:24.04 size name n hex
rm -rf "$src" "$seen"; mkdir -p "$src/folder/sub" "$src/sub" "$seen/project"
(cd "$seen/project" && cozy --json new --unencrypted) > "$OUT/unseen-new.out" || { check "unseen: initialize project"; return; }
echo one > "$src/folder/a.txt"; echo two > "$src/folder/sub/b.txt"
ffmpeg -v error -y -f lavfi -i testsrc2=size=320x240:rate=24:duration=3 -c:v libx264 -pix_fmt yuv420p "$src/clip.mp4"
for name in still other; do
  ffmpeg -v error -y -f lavfi -i "color=c=0x$(printf %06x $((RANDOM * RANDOM % 16777216))):size=64x64" -frames:v 1 "$src/$name.png"
done
cp "$src/still.png" "$src/sub/beside.png"
printf 'clip: ../clip.mp4\nstill: beside.png\nfolder: ../folder\nlabel: from-input\n' > "$src/sub/req.yaml"
safetensors_file "$src/tiny.safetensors"
cp -r "$here/inspect" "$here/weightless" "$src/" && cp "$here/project/draw.py" "$src/draw.py"
rm -rf "$src"/*/.venv "$src"/*/uv.lock
# Local code holding an empty file goes whole (0.22.1's CLI sent it without a length: file_length_required).
: > "$src/weightless/e2e_weightless/empty.py"; : > "$src/inspect/e2e_inspect/empty.py"
size=$(stat -c %s "$src/still.png")
[ ! -e /work ] || { check "unseen: /work is on this computer too"; return; }
docker pull -q "$image" > /dev/null || { check "unseen: docker pull $image"; return; }
# inside <name> <cozy words…>: the CLI at the host project path with client-only source files mounted over it. $AFTER, a shell
# line, then runs there, its words on stderr. Its stdout is unseen-<name>.out, its stderr unseen-<name>.err. $REFUSED: its
# failure is the caller's to judge.
inside() {
  local name=$1 container=$image; shift
  local extra=()
  if [ -n "${LOCK_FIXTURE:-}" ]; then
    [ -x "${RUN_CODE_STAGE:-}" ] || { check "unseen lock: RUN_CODE_STAGE must name the API stage-code executable"; return 1; }
    container=python:3.12-slim-trixie
    extra=(-v "$here/..:/e2e:ro" -v "$RUN_CODE_STAGE:/usr/local/bin/stage-code:ro")
  else
    printf '%s\x1f' "$@" >> "$OUT/argv.log"; printf '\x1e' >> "$OUT/argv.log"  # docker runs the binary itself
  fi
  cp "$HOME/.cozy/calcifer/calcifer.lock.holder" "$HOME/.cozy/config.toml" "$seen/"
  docker run --rm --network host --pid host -u "$(id -u):$(id -g)" -e HOME=/home/cli -e COZY_HUB_URL="${COZY_HUB_URL:-}" \
    -e PROJECT_DIRECTORY="$seen/project" -e AFTER="${AFTER:-}" -e DIR="${DIR:-.}" -e LOCK_FIXTURE="${LOCK_FIXTURE:-}" \
    -e CI="${CI:-}" -e GITHUB_ACTIONS="${GITHUB_ACTIONS:-}" -e COZY_TEST_HUB_ORIGIN="${COZY_TEST_HUB_ORIGIN:-}" \
    --tmpfs "/home/cli:rw,exec,uid=$(id -u),gid=$(id -g),mode=0700" --tmpfs "$seen/project:rw,exec,uid=$(id -u),gid=$(id -g),mode=0700" \
    -v "$seen:/seen:ro" -v "$COZY:/usr/local/bin/cozy:ro" -v "$src:/src:ro" -v /etc/ssl/certs:/etc/ssl/certs:ro \
    "${extra[@]}" ${CAND_RT:+-v "$(dirname "$CAND_RT"):/wheels:ro"} -w "$seen/project" "$container" \
    sh -c 'mkdir -p ~/.cozy/calcifer && cp /seen/calcifer.lock.holder ~/.cozy/calcifer/ && cp /seen/config.toml ~/.cozy/ \
      && cp -r /src/. "$PROJECT_DIRECTORY" && cd "$PROJECT_DIRECTORY/$DIR" \
      && { if [ -n "$LOCK_FIXTURE" ]; then python3 /e2e/package-lock.py --stage-code /usr/local/bin/stage-code . "$@"; else cozy "$@"; fi; } \
      && { [ -z "$AFTER" ] || sh -c "$AFTER" >&2; }' sh "$@" \
    > "$OUT/unseen-$name.out" 2> "$OUT/unseen-$name.err" && return 0
  [ -n "${REFUSED:-}" ] || check "unseen $name: cozy $*: $(tail -c 400 "$OUT/unseen-$name.err") $(tail -c 300 "$OUT/unseen-$name.out")"
  return 1
}
# refused_inside <name> <code> <cozy words…>: inside, refused with that code, as Calcifer refuses what it read: so it read it.
refused_inside() {
  local name=$1 code=$2; shift 2
  REFUSED=1 inside "$name" --json "$@" && { check "unseen $name: cozy $* was not refused"; return; }
  grep -q "\"code\": *\"$code\"" "$OUT/unseen-$name.out" || check "unseen $name: cozy $*: not $code: $(tail -c 400 "$OUT/unseen-$name.out")"
}
said() { grep -qE "$2" "$OUT/unseen-$1.out" || check "unseen $1: not $2: $(grep -oE '"(clip_seconds|still_bytes|folder_files|label)": *[^,}]*' "$OUT/unseen-$1.out" | paste -sd' ')"; }
# Client code is sent by content; its working directory names the separate host project. Union fields go as text, as a
# smoke request names them (0.22.1's binder refused `aspect_ratio=1:1` on `null|aspect-ratio`).
if inside files --json run ./inspect/inspect --file clip=./clip.mp4 --file still=./still.png folder=./folder label=files \
    aspect_ratio=1:1 seed=7 --on local; then
  said files '"clip_seconds": *(2\.9|3\.0|3\.1)' && said files "\"still_bytes\": *$size" && said files '"folder_files": *\[ *"a.txt", *"sub/b.txt" *\]'
  said files '"target": *"local/e2e-inspect/inspect"' && said files '"aspect_ratio": *"1:1"' && said files '"seed": *7'
fi
inside input --json run ./inspect/inspect --input ./sub/req.yaml --on local && said input '"label": *"from-input"' && said input "\"still_bytes\": *$size"
inside positional --json run ./inspect/inspect ./still.png --file clip=./clip.mp4 folder=./folder label=positional --on local && said positional "\"still_bytes\": *$size"
inside describe --json run ./inspect/inspect --describe && J '.fields | any(.name == "clip") and any(.name == "folder")' "$OUT/unseen-describe.out" \
  || check "unseen describe: $(head -c 300 "$OUT/unseen-describe.out")"
inside functions --json run ./weightless && J '.package == "local/e2e-weightless" and (.functions | any(.function == "reverse"))' "$OUT/unseen-functions.out" \
  || check "unseen functions: $(head -c 300 "$OUT/unseen-functions.out")"
# A script's still, and its file downloaded into /work by its digest: what it holds is what the run says it made.
if AFTER='sha256sum out/*' inside script --json run ./draw.py/draw instructions=unseen -o out --on local; then
  hex=$(tail -1 "$OUT/unseen-script.out" | jq -r '.run.output[0].sha256 // empty' | sed 's/^sha256://')
  { [ -n "$hex" ] && grep -q "^$hex  out/" "$OUT/unseen-script.err"; } || check "unseen script: out/ holds $(grep '  out/' "$OUT/unseen-script.err"), not sha256:$hex"
  n=$(tail -1 "$OUT/unseen-script.out" | jq -r '.run.number // empty')
  AFTER='sha256sum again/*' inside show run watch "$n" -o again && grep -q "^$hex  again/" "$OUT/unseen-show.err" \
    || check "unseen show: run watch $n -o again: $(tail -c 300 "$OUT/unseen-show.err")"
fi
inside install --json package install ./weightless --on local && J '.run.status == "completed"' <(tail -1 "$OUT/unseen-install.out") \
  || check "unseen install: $(tail -c 300 "$OUT/unseen-install.out")"
# Its recipes read from Calcifer's copy, then refused only at the model's header, which a sign-in reads: from a path it couldn't
# see, the package was refused first (`target_not_found`).
refused_inside quantize not_logged_in model quantize zhengpeng7/birefnet --lane int8 --package ./weightless --on local
AFTER='grep -c "name = \"e2e-weightless\"" uv.lock' DIR=weightless LOCK_FIXTURE=1 inside lock && grep -qx 1 "$OUT/unseen-lock.err" \
  || check "unseen lock: no uv.lock written in /work: $(tail -c 300 "$OUT/unseen-lock.err")"
# Locked in Calcifer's copy of the tree, which it holds by the tree's digest.
# Local code is project data: Calcifer's copy is the project's own (.vault-calcifer).
ls -d "$seen"/project/.vault-calcifer/code/*/tree/e2e_weightless > /dev/null 2>&1 || check "unseen lock: Calcifer holds no copy of the package in the project: $(ls "$seen/project/.vault-calcifer/code" 2> /dev/null | head -3)"
# Publishing a project's package is an explicit export: it gets as far as the sign-in it needs.
DIR=weightless refused_inside publish not_logged_in package publish
# Its benchmarks come from this project's warm runs of it on a GPU (cl-1024): this CPU runner made none, so there's nothing to write.
DIR=weightless refused_inside benchmarks no_benchmarks package benchmarks
# A model installed from a project's file is an explicit export: it is the machine's.
inside model --json model install ./tiny.safetensors local/e2e-unseen --source-format as-is --on local \
  && cozy --json model ls --on local | grep -q e2e-unseen || check "unseen model: local/e2e-unseen is not on local"
if [ -f "${CAND_RT:-}" ]; then
  inside runtime --json machine update local --runtime "/wheels/$(basename "$CAND_RT")" -y || true
fi
# Local code is a project's or goes by content: a path to your tool list is refused, as no client's folder is Calcifer's.
local lock=$HOME/.cozy/calcifer/calcifer.lock.holder
[ "$(curl -sS -o "$OUT/unseen-list.json" -w '%{http_code}' -H "Authorization: Bearer $(jq -r .token "$lock")" -H 'Content-Type: application/json' \
  -d '{"add": "/work/weightless"}' "http://$(jq -r .address "$lock")/v1/tools/list")" = 400 ] && J '.error.code == "tool_list_local"' "$OUT/unseen-list.json" \
  || check "unseen: a path added to the tool list: $(cat "$OUT/unseen-list.json")"
}

# cl-990 (Paul 11-13: installed packages are the machine's): a checkout installed from outside any project is a machine-wide
# package, its install one of Calcifer's own runs, its copy and environment in the machine's stores (read as the imports suite
# reads them); it uninstalls there, and its recipe gets as far as the sign-in a model's header needs. A call needs a project,
# as a Hub package's does. In a project, local code is still the project's own.
machine_checkout() {
step "a checkout outside any project is a machine-wide package (cl-990)"
[ -n "$LOCAL" ] || return 0
local out=$OUT/machine-checkout py code=0
mkdir -p "$out"
(cd "$out" && cozy --json package install "$here/weightless" --on local -y) > "$out/install.ndjson" 2> "$out/install.err" \
  || { check "machine checkout: package install outside a project: $(tail -c 300 "$out/install.ndjson") $(tail -c 200 "$out/install.err")"; return; }
py=$(python3 -I "$here/../imports/generation.py" "$HOME/.cozy" "$out/install.ndjson" e2e-weightless 0.1.0 2> "$out/generation.err") \
  && [ -x "$py" ] && echo "machine checkout: its environment is the machine's: $py" \
  || check "machine checkout: no installation in the machine's store: $(cat "$out/generation.err")"
case $py in "$RUN_PROJECT"/*) check "machine checkout: its environment is the project's: $py" ;; esac
ls -d "$HOME"/.cozy/calcifer/code/*/tree/e2e_weightless > /dev/null 2>&1 \
  || check "machine checkout: Calcifer holds no machine copy: $(ls "$HOME/.cozy/calcifer/code" 2> /dev/null | head -3)"
(cd "$out" && cozy --json model quantize zhengpeng7/birefnet --lane int8 --package "$here/weightless" --on local) > "$out/quantize.json" 2>&1 || code=$?
[ "$code" != 0 ] && grep -q '"code": *"not_logged_in"' "$out/quantize.json" \
  || check "machine checkout: model quantize --package outside a project: exit $code, not not_logged_in: $(tail -c 300 "$out/quantize.json")"
(cd "$out" && cozy --json package uninstall local/e2e-weightless --on local) > "$out/uninstall.json" 2> "$out/uninstall.err" \
  && J '(.removed | any(.package == "local/e2e-weightless")) and (.failed // []) == []' "$out/uninstall.json" \
  || check "machine checkout: package uninstall local/e2e-weightless outside a project: $(tail -c 300 "$out/uninstall.json") $(tail -c 200 "$out/uninstall.err")"
code=0; (cd "$out" && cozy --json run "$here/weightless/reverse" text=outside) > "$out/run.json" 2> "$out/run.err" || code=$?
[ "$code" = 2 ] && J '.error.code == "not_in_a_project"' "$out/run.json" \
  || check "machine checkout: a call outside a project: exit $code, not not_in_a_project as a Hub package's: $(tail -c 300 "$out/run.json")"
in_project "$RUN_PROJECT" --json run "$here/weightless" --describe > "$out/describe-project.json" 2> "$out/describe-project.err" \
  && ls -d "$RUN_PROJECT"/.vault-calcifer/code/*/tree/e2e_weightless > /dev/null 2>&1 \
  || check "machine checkout: in a project, its copy is not the project's: $(tail -c 300 "$out/describe-project.err")"
}

# cl-820: local code edited between runs on `local`, as cl-817's edit.sh timed it (an edit took 7.9 s read in place, 10.2 s once it
# went by content with an environment per tree). The environment is keyed on what it is built from (lock, pyproject, path
# dependencies), never the code: an edit to the code alone runs on the same one, an edit back runs the first code again, and a new
# dependency builds another. Each step's `cozy run` wall time, the reply and the environment TensorD ran it in.
# Local codes made ready at once on a computer with no uv yet (cl-911): Calcifer fetches uv once, and the others wait for that
# fetch, where the second was refused `uv_unavailable` ("run this again", cli gate 43133580881). The local machine fetched uv
# into the same tools folder when its candidates went on, so that is removed first, as on a computer that never had it.
uv_at_once() {
step "local codes at once on a fresh computer: one uv fetch, all of them run"
[ -n "$LOCAL" ] || return 0
local fetched=$HOME/.cozy/tools/uv pid dir found='' at=$OUT/at-once i pids=() code
pid=$(jq -r .pid "$HOME/.cozy/calcifer/calcifer.lock.holder")
while IFS= read -r dir; do [ ! -x "$dir/uv" ] || found=$dir/uv; done < <(tr '\0' '\n' < "/proc/$pid/environ" | sed -n 's/^PATH=//p' | tr ':' '\n')
[ -z "$found" ] || { check "uv at once: Calcifer finds uv on its PATH ($found), so it never fetches one"; return; }
rm -rf "$fetched"
rm -rf "$at" && mkdir -p "$at"
for i in a b; do
  cp -r "$here/weightless" "$at/$i" && sed -i "s/^name = \"e2e-weightless\"/name = \"e2e-at-once-$i\"/" "$at/$i/pyproject.toml"
done
cp "$here/script/words.py" "$at/words.py"
for i in a/reverse b/reverse words.py; do
  (in_project "$RUN_PROJECT" --json run "$at/$i" "text=at once" --on local > "$at/${i%%/*}.ndjson" 2> "$at/${i%%/*}.err") & pids+=($!)
done
for i in 0 1 2; do
  code=0; wait "${pids[$i]}" || code=$?
  [ "$code" = 0 ] || check "uv at once: run $((i + 1)) of 3 exited $code: $(tail -c 300 "$at/$(echo a b words.py | cut -d' ' -f$((i + 1))).err")"
done
! grep -l uv_unavailable "$at"/*.err "$at"/*.ndjson 2> /dev/null || check "uv at once: refused uv_unavailable: $(grep -h -o '[^"]*uv_unavailable[^"]*' "$at"/*.err "$at"/*.ndjson | head -1)"
compgen -G "$fetched/*/uv" > /dev/null || check "uv at once: Calcifer never fetched uv into $fetched"
}

local_edits() {
step "local code edited between runs"
[ -n "$LOCAL" ] || return 0
local code=$OUT/edits/e2e-edits envs=$HOME/.cozy/machine/var/lib/cozy/tensord/editable  # environments are the machine's (cl-984)
local module=$code/e2e_weightless/__init__.py first now
rm -rf "$code" && mkdir -p "$OUT/edits" && cp -r "$here/weightless" "$code"
sed -i 's/^name = "e2e-weightless"/name = "e2e-edits"/' "$code/pyproject.toml"
: > "$code/e2e_weightless/empty.py"  # 0.22.1 (f82d8de26): an empty file's PUT states its Content-Length, or Calcifer refuses it
: > "$OUT/edits.txt"
timed() {  # timed <step> <reply>: the environment its run had, on stdout
  local t0 t1 env
  t0=$(date +%s.%N)
  in_project "$RUN_PROJECT" --json run "$code/reverse" text=abc --on local > "$OUT/edit-$1.ndjson" 2> "$OUT/edit-$1.err" \
    || { check "edit $1: cozy run did not complete: $(tail -c 300 "$OUT/edit-$1.err")" >&2; return 1; }
  t1=$(date +%s.%N)
  grep -q "\"text\": *\"$2\"" "$OUT/edit-$1.ndjson" || check "edit $1: the reply is not $2: $(grep -o '"text": *"[^"]*"' "$OUT/edit-$1.ndjson" | tail -1)" >&2
  env=$(cd "$envs" && ls -d -- */e2e-edits/env-* 2> /dev/null | sed 's#.*/##' | paste -sd, -)
  printf '%s: %s in %.1f s, environment %s\n' "$1" "$2" "$(echo "$t1 - $t0" | bc)" "${env:-none}" | tee -a "$OUT/edits.txt" >&2
  echo "$env"
}
first=$(timed first cba) || return 0
[ -n "$first" ] || check "edit first: TensorD holds no environment for e2e-edits under $envs"
sed -i 's/return Reversed(text=payload.text\[::-1\])/return Reversed(text=payload.text[::-1] + "!edited")/' "$module"
if now=$(timed edited 'cba!edited'); then [ "$now" = "$first" ] || check "edit: the code alone built another environment ($first, then $now)"; fi
sed -i 's/ + "!edited"//' "$module"
if now=$(timed back cba); then [ "$now" = "$first" ] || check "edit back: another environment than the first's ($first, then $now)"; fi
sed -i 's/"msgspec>=0.19"\]/"msgspec>=0.19", "six>=1.16"]/' "$code/pyproject.toml"
if now=$(timed dependency cba); then [ -n "$now" ] && [ "$now" != "$first" ] || check "edit: a new dependency ran on the environment before it ($now)"; fi
}

# cl-871: local code that calls another cozy package, written as an author writes it: its dependency's source names the
# `tensorhub` index, and it has no uv.lock. Signed out, the root's index serves it (the minimax-h3-tools stand-in isolated.sh
# publishes at the root, its `echo`). cozy locks it at the run, as `cargo run` locks: the run calls the dependency's function, and
# the uv.lock it took, naming the root's index, is written into the package's folder. cozy 0.24.0 refused it unlocked
# (`account_index_unreached`), and signed in it looked only in the account's own index.
hub_dependency() {
step "local code calling a published cozy package, locked at its run (cl-871)"
[ -n "$LOCAL" ] || return 0
local dir=$OUT/hub-dependency
rm -rf "$dir"
package_at "$dir" e2e_local_relay '"cozy-runtime>=0.47.0.dev0,<0.48", "msgspec>=0.19", "minimax-h3-tools>=0.1.0"' \
  "$(printf '\n[tool.uv.sources]\nminimax-h3-tools = { index = "tensorhub" }')" <<'PY'
import msgspec
from e2e_local_relay_calls import Text, echo

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
cp "$here/relay/e2e_relay/echo_calls.py" "$dir/e2e_local_relay_calls.py"
sed -i 's/only-include = \["e2e_local_relay.py"\]/only-include = ["e2e_local_relay.py", "e2e_local_relay_calls.py"]/' "$dir/pyproject.toml"
cell hub-dependency "$dir/relay" "text=from the root index" || return 0
grep -q '"text": *"from the root index"' "$OUT/hub-dependency.ndjson" && grep -q '"release": *"0.1.0"' "$OUT/hub-dependency.ndjson" \
  || check "hub dependency: the reply is not minimax-h3-tools 0.1.0's echo: $(tail -c 300 "$OUT/hub-dependency.ndjson")"
grep -q '/v1/index/_/simple/' "$dir/uv.lock" 2> /dev/null \
  || check "hub dependency: the package's folder holds no uv.lock naming the root's index: $(ls "$dir" | paste -sd' ')"
}

# cl-872: an account's published package that calls a root package. isolated.sh publishes e2e/cli/relay as its admin's own
# `<account>/e2e-relay`, its lock taking minimax-h3-tools from the root's index. Run signed out by its name, it answers with the
# stand-in's echo; the environment the machine built names that callee by its own package, `_/minimax-h3-tools` (TensorD read
# every Hub row of a release as the release's org: `<account>/minimax-h3-tools`, the callee's slots, memo and records then
# another package's); and the Hub's publication warned of no other Hub's index. cozy 0.24.0 could not publish it at all: its
# account's index serves no root package.
hub_published() {
step "an account's published package calling a root package (cl-872)"
[ -n "$LOCAL" ] || return 0
[ -n "${E2E_RELAY:-}" ] || { check "hub published: isolated.sh published no e2e-relay: ${E2E_RELAY_REFUSED:-not seeded}"; return; }
! grep -q "another Hub's index" "$E2E_RELAY_PUBLISHED" \
  || check "hub published: its publication warned of another Hub's index: $(grep -o "uv.lock package [^\"]*" "$E2E_RELAY_PUBLISHED" | head -1)"
cell hub-published "$E2E_RELAY/relay" "text=from a published package" || return 0
grep -q '"text": *"from a published package"' "$OUT/hub-published.ndjson" && grep -q '"release": *"0.1.0"' "$OUT/hub-published.ndjson" \
  || check "hub published: the reply is not minimax-h3-tools 0.1.0's echo: $(tail -c 300 "$OUT/hub-published.ndjson")"
local callee
# A generation's record names its package by distribution, and each callee by the package it is.
callee=$(find "$RUN_PROJECT/.vault-machine" "$HOME/.cozy/machine" -name generation.json -path '*generations*' -exec jq -r \
  'select(.package == "e2e-relay") | .callees[]? | select(.distribution | ascii_downcase | gsub("[-_.]+"; "-") == "minimax-h3-tools") | .package' \
  {} + 2> /dev/null | sort -u | paste -sd, -)
echo "hub published: $E2E_RELAY's environment names its callee ${callee:-nothing}"
[ "$callee" = _/minimax-h3-tools ] || check "hub published: $E2E_RELAY's environment names its callee ${callee:-nothing}, not _/minimax-h3-tools"
}

# xs-194: a package whose floor only the open RC meets (cozy-runtime>=0.47.0.dev0,<0.48) names no Runtime wheel, and runs from
# its source and from the Hub as it was published: each lock takes the RC's newest build, so a Runtime merge between two
# runs of this cell (each Runtime merge replaces the RC's wheels) needs no relock. isolated.sh published e2e-rcfloor 0.1.0.
rc_floor() {
step "a package with only an RC floor runs from source and from the Hub, its lock on the RC's newest Runtime (xs-194)"
[ -n "$LOCAL" ] || return 0
local src=$OUT/rcfloor runtime rc minor newest="the open RC's newest"
if cozy --version | awk '{exit !($2 ~ /-/)}'; then
  # A dev cozy (X.Y.0-dev) is its open RC's: the lock takes that RC's newest build.
  rc=$(cozy --version | awk '{n = split($2, v, /[.-]/); print v[1] "." v[2] ".0"}')
  runtime=$(curl -fsSL "https://github.com/cozy-creator/cozy-runtime/releases/download/rc-$rc/SHA256SUMS" 2> /dev/null \
    | awk '$2 ~ /-py3-none-any[.]whl$/ {split($2, p, "-"); print p[2]; exit}')
else
  # A release's (a run on release/X.Y.Z, its tag, or a PR into it) is its own line's, never the next RC's (cl-1186): the lock takes
  # the newest released Runtime of the minor its machine runs, as cozy-runtime's release/X.Y.Z is.
  newest="the release line's newest"
  minor=$(cozy --json machine show local 2> /dev/null | jq -r '.status.runtime // empty' | awk -F. '{print $1 "." $2}')
  [ -z "$minor" ] || runtime=$(curl -fsSL https://pypi.org/pypi/cozy-runtime/json 2> /dev/null | jq -r '.releases | keys[]' \
    | grep -xE "${minor//./\\.}\.[0-9]+" | sort -V | tail -1)
fi
echo "rc floor: $newest Runtime is ${runtime:-unread}"
grep -q '"status": *"published' "${E2E_RCFLOOR_PUBLISHED:-/dev/null}" 2> /dev/null \
  || check "rc floor: publishing e2e-rcfloor failed: $(tail -c 300 "${E2E_RCFLOOR_PUBLISHED:-/dev/null}" 2> /dev/null | tr '\n' ' ')"
rm -rf "$src" && cp -r "$here/relay" "$src"
sed -i 's/^name = "e2e-relay"$/name = "e2e-rcsource"/' "$src/pyproject.toml"
cell rc-floor-source "$src/relay" "text=from source" || check "rc floor: its source run failed: $(run_get "$OUT" rc-floor-source .reason.message | head -c 300)"
grep -q 'cozy-runtime = ' "$src/pyproject.toml" && check "rc floor: its pyproject.toml took a Runtime source"
if [ -f "$src/uv.lock" ] && [ -n "$runtime" ]; then
  grep -A1 -x 'name = "cozy-runtime"' "$src/uv.lock" | grep -qF "version = \"$runtime\"" \
    || check "rc floor: its lock holds $(grep -A1 -x 'name = "cozy-runtime"' "$src/uv.lock" | tail -1), not $newest $runtime"
fi
[ -z "${E2E_RCFLOOR:-}" ] || cell rc-floor-hub "$E2E_RCFLOOR/relay" "text=from the Hub" \
  || check "rc floor: the published e2e-rcfloor's run failed: $(run_get "$OUT" rc-floor-hub .reason.message | head -c 300)"
}

# cl-952, se-128: an unversioned run takes the Hub's newest release its machine runs, installing it when the machine holds
# an older one, and says so. isolated.sh publishes e2e-held 0.1.0 and 0.2.0rc1, and e2e-newer 0.1.0 and 0.1.1. Put on a released
# Runtime by hand, local refuses a project's run, naming the update (cl-1055). Holding e2e-newer 0.1.0 (asked by name), an
# unversioned run takes 0.1.1 and says it's newer (47617 stayed on minimax-h3 0.11.1 after 0.12.0rc2).
#
# cl-1109 (coordinator, 11-16; xs-203's pod-session 43961722220 installed the Hub's minimax-h3 0.13.0rc2 on RC machines while the
# palette said 0.12.0, and every H3 run failed): "an unpinned or range request never resolves to a prerelease; only an exact pin
# (or an explicit opt-in) selects one" (Paul, 11-13: no test prereleases; finals only at promotion). isolated.sh publishes the tool
# e2e-final 0.1.0 and 0.2.0rc1. On local, an RC build: the palette lists 0.1.0; an unpinned install installs 0.1.0, and the
# palette still lists it; an unpinned run takes 0.1.0; a run pinned to 0.2.0rc1 takes it, and the palette, holding both, still
# lists 0.1.0.
held_prerelease() {
step "an unversioned run or install takes the newest final, a prerelease only when pinned; held below this cozy's Runtime minor, a project's run is refused (cl-952, se-128, cl-1055, cl-1109)"
[ -n "$LOCAL" ] || return 0
[ -n "${E2E_HELD:-}" ] || { check "held prerelease: isolated.sh published no e2e-held: ${E2E_HELD_REFUSED:-not seeded}"; return; }
local released=${HELD_RUNTIME:-0.45.1} was code=0
machine_ready
if [ -z "${E2E_FINAL:-}" ]; then check "unpinned final: isolated.sh published no e2e-final: ${E2E_FINAL_REFUSED:-not seeded}"
else unpinned_final; fi
case $(jq -r '.status.runtime // empty' "$OUT/machine.json") in *+dev*) ;; *) echo "held prerelease: local isn't on an RC build"; return 0 ;; esac
was=${CAND_RT:-$(jq -r '.status.runtime // empty' "$OUT/machine.json")}
# Unversioned, in words as a person reads it: the run says what it moved to. On the RC's Runtime: a project's run needs this cozy's
# Runtime minor (cl-1055).
if cell held-newer-old "$E2E_NEWER/relay@0.1.0" "text=asked 0.1.0"; then
  in_project "$RUN_PROJECT" run "$E2E_NEWER/relay" text=newer --on local > "$OUT/held-newer.out" 2> "$OUT/held-newer.err" \
    || check "held prerelease: the unversioned e2e-newer run failed: $(tail -c 300 "$OUT/held-newer.err")"
  grep -q 'e2e-newer 0.1.1, newer than the 0.1.0 on local' "$OUT/held-newer.err" \
    || check "held prerelease: holding e2e-newer 0.1.0, the run didn't say it takes 0.1.1: $(tail -c 300 "$OUT/held-newer.err")"
fi
cozy --json machine update local --runtime "$released" -y > "$OUT/held-update.json" 2>&1 \
  || { check "held prerelease: local refused Runtime $released: $(tail -c 300 "$OUT/held-update.json")"; return; }
machine_ready
# Held on a release below this cozy's Runtime minor, off the RC: a project's run is refused naming the update, and runs nothing.
in_project "$RUN_PROJECT" --json run "$E2E_HELD/relay" "text=off the RC" --on local > "$OUT/held-prerelease.ndjson" 2> "$OUT/held-prerelease.err" || code=$?
[ "$code" != 0 ] && grep -q '"runtime_held"' "$OUT/held-prerelease.ndjson" "$OUT/held-prerelease.err" \
  && grep -q 'cozy machine update local' "$OUT/held-prerelease.ndjson" "$OUT/held-prerelease.err" \
  || check "held prerelease: held on Runtime $released, a project's e2e-held run wasn't refused runtime_held naming cozy machine update local: exit $code $(tail -c 300 "$OUT/held-prerelease.ndjson") $(tail -c 300 "$OUT/held-prerelease.err")"
cozy --json machine update local --runtime "$was" -y > "$OUT/held-back.json" 2>&1 \
  || check "held prerelease: local refused Runtime $was again: $(tail -c 300 "$OUT/held-back.json")"
machine_ready
}

# unpinned_final: cl-1109's checks (held_prerelease's header), on e2e-final.
unpinned_final() {
local holder=$HOME/.cozy/calcifer/calcifer.lock.holder tool=$E2E_FINAL/echo said
# listed <name>: the releases the palette (`GET /v1/tools`) lists e2e-final at on local, its answer in $OUT/<name>.json; up to 30 s for it to list it.
listed() {
  local _
  for _ in 1 2 3 4 5 6; do
    curl --fail-with-body -sS -H "Authorization: Bearer $(jq -r .token "$holder")" "http://$(jq -r .address "$holder")/v1/tools?on=local" > "$OUT/$1.json" 2>&1 \
      && J --arg id "$tool" '[.tools[] | select(.id == $id)] != []' "$OUT/$1.json" && break
    sleep 5
  done
  jq -c --arg id "$tool" '[.tools[]? | select(.id == $id) | .release] | unique' "$OUT/$1.json" 2> /dev/null || head -c 300 "$OUT/$1.json"
}
said=$(listed final-palette)
[ "$said" = '["0.1.0"]' ] || check "unpinned final: before an install, the palette lists $tool at $said, not 0.1.0"
cozy --json package install "$E2E_FINAL" --on local > "$OUT/final-install.ndjson" 2> "$OUT/final-install.err" \
  || check "unpinned final: package install $E2E_FINAL --on local failed: $(tail -c 300 "$OUT/final-install.err")"
cozy --json package ls --on local > "$OUT/final-installed.json" 2>&1
said=$(jq -c --arg p "$E2E_FINAL" '[.. | objects | select(.package? == $p) | .release] | unique' "$OUT/final-installed.json" 2> /dev/null)
[ "$said" = '["0.1.0"]' ] || check "unpinned final: an unpinned install put $E2E_FINAL $said on local, not 0.1.0: $(tail -c 300 "$OUT/final-install.err")"
said=$(listed final-palette-installed)
[ "$said" = '["0.1.0"]' ] || check "unpinned final: after an unpinned install, the palette lists $tool at $said, not 0.1.0"
# ran <name>: the release run <name>'s reply says ran (the package echoes its own).
ran() { grep -o '"release": *"[^"]*"' "$OUT/$1.ndjson" | tail -1 | cut -d'"' -f4; }
if cell final-unpinned "$tool" "text=unpinned"; then
  [ "$(ran final-unpinned)" = 0.1.0 ] || check "unpinned final: an unpinned run ran $(ran final-unpinned) ($(run_get "$OUT" final-unpinned .target)), not 0.1.0"
fi
if cell final-pinned "$tool@0.2.0rc1" "text=pinned"; then
  [ "$(ran final-pinned)" = 0.2.0rc1 ] || check "unpinned final: a run pinned to 0.2.0rc1 ran $(ran final-pinned) ($(run_get "$OUT" final-pinned .target))"
fi
said=$(listed final-palette-both)
[ "$said" = '["0.1.0"]' ] || check "unpinned final: holding 0.1.0 and 0.2.0rc1, the palette lists $tool at $said, not 0.1.0"
}

# cl-952: a machine runs only a package release its Runtime can run. Publishing refused isolated.sh's e2e-lies, whose floor
# cozy-runtime>=0.44 lacks the `call` it imports (minimax-h3 0.12.0rc3 declared 0.45 and imported the RC's `preview`: run
# 47791). A project's run on local also needs this cozy's Runtime minor (cl-1055). Held on 0.45.0 by hand, local refuses
# e2e-floor 0.2.0 (cozy-runtime>=0.45.1) and 0.1.0 alike, naming `cozy machine update local`, and still does after Calcifer
# restarts (the hold is kept in its store); with nothing named, `cozy machine update local` moves it to its Hub's pin (0.45.0, this
# shard's isolated Hub: e2e.yaml), which holds nothing, and a project's run then moves it to this cozy's own Runtime, saying so.
# Held on a release of the minor, a cap binds: e2e-capped 0.2.0 (<0.47.0.dev0) runs as 0.1.0, as a machine moves only up.
runtime_floor() {
step "a package release runs on a Runtime that meets its floor: publishing refuses a dishonest one; a held machine refuses, another upgrades (cl-952)"
[ -n "$LOCAL" ] || return 0
runtime_floor_cells
# Whatever stopped the cells above, local gets its candidates back (and this computer's cozy, if the RC pin stopped
# halfway), so a red there never runs the cells after it on the held 0.45.0 (child-latents did: 43402947916).
local was=$OUT/cozy-before-rc-pin code=0
if [ -f "$was" ] && [ "$(cozy --version)" != "$("$was" --version)" ]; then
  cozy --json machine update local --tensord "$was" -y > "$OUT/floor-back-cozy.json" 2>&1 \
    || check "runtime floor: local refused this computer's cozy back: $(tail -c 300 "$OUT/floor-back-cozy.json")"
fi
cozy --json machine update local ${CAND_RT:+--runtime "$CAND_RT"} ${CAND_TFS:+--tensorfs "$CAND_TFS"} -y > "$OUT/floor-back.json" 2>&1 \
  || check "runtime floor: local refused its candidates again: $(tail -c 300 "$OUT/floor-back.json")"
machine_ready
# The open RC's Runtime (a dev release, as an RC candidate's 0.47.0.dev0+dev.g… is) runs only under the RC's cozy: a released cozy
# named by hand under it is refused in one line, and nothing moves.
case $(jq -r '.status.runtime' "$OUT/machine.json") in
  *.dev[0-9]*)
    code=0
    cozy --json machine update local --tensord 0.24.0 -y > "$OUT/floor-rc-cozy.json" 2>&1 || code=$?
    [ "$code" != 0 ] && grep -q '"rc_runtime_needs_rc_cozy"' "$OUT/floor-rc-cozy.json" \
      || check "runtime floor: a released cozy under the RC's Runtime wasn't refused: exit $code $(tail -c 300 "$OUT/floor-rc-cozy.json")" ;;
  *) echo "runtime floor: local's candidate Runtime $(jq -r .status.runtime "$OUT/machine.json") is no RC dev release: the RC cozy pairing isn't asked here" ;;
esac
}

runtime_floor_cells() {
[ -n "${E2E_FLOOR:-}" ] || { check "runtime floor: isolated.sh published no e2e-floor: ${E2E_HELD_REFUSED:-not seeded}"; return; }
grep -q '"runtime_floor_lacks"' "$E2E_LIES" && grep -q 'call from cozy_runtime.author' "$E2E_LIES" && grep -q 'raise its floor: cozy-runtime>=0\.' "$E2E_LIES" \
  || check "runtime floor: publishing e2e-lies (cozy-runtime>=0.44, imports call) wasn't refused naming call and the floor its lock has: $(tail -c 400 "$E2E_LIES")"
local code=0
cozy --json machine update local --runtime 0.45.0 -y > "$OUT/floor-hold.json" 2>&1 \
  || { check "runtime floor: local refused Runtime 0.45.0: $(tail -c 300 "$OUT/floor-hold.json")"; return; }
machine_ready
in_project "$RUN_PROJECT" --json run "$E2E_FLOOR/relay" text=held --on local > "$OUT/floor-held.ndjson" 2> "$OUT/floor-held.err" || code=$?
[ "$code" != 0 ] && grep -q '"runtime_held"' "$OUT/floor-held.ndjson" "$OUT/floor-held.err" && grep -q 'cozy machine update local' "$OUT/floor-held.ndjson" "$OUT/floor-held.err" \
  || check "runtime floor: held on 0.45.0, e2e-floor (0.2.0 needs >=0.45.1) wasn't refused runtime_held naming cozy machine update local: exit $code $(tail -c 400 "$OUT/floor-held.ndjson") $(tail -c 300 "$OUT/floor-held.err")"
# The hold outlives Calcifer: restarted, it still refuses and moves nothing (cl-922's asirpa moved after a restart).
systemctl --user restart cozy-calcifer || check "runtime floor: systemctl --user restart cozy-calcifer failed"
machine_ready
code=0
in_project "$RUN_PROJECT" --json run "$E2E_FLOOR/relay" text=held-again --on local > "$OUT/floor-held-again.ndjson" 2> "$OUT/floor-held-again.err" || code=$?
[ "$code" != 0 ] && grep -q '"runtime_held"' "$OUT/floor-held-again.ndjson" "$OUT/floor-held-again.err" \
  || check "runtime floor: after a Calcifer restart, local held on 0.45.0 wasn't refused runtime_held: exit $code $(tail -c 300 "$OUT/floor-held-again.ndjson")"
jq -e '.status.runtime == "0.45.0"' "$OUT/machine.json" > /dev/null \
  || check "runtime floor: after a Calcifer restart, held local runs $(jq -r .status.runtime "$OUT/machine.json"), not 0.45.0"
# A project's run on this computer needs this cozy's Runtime minor (cl-1055): held below it, even a release its own floor admits
# (e2e-floor 0.1.0) is refused naming the update, and e2e-scratch's probe runs nowhere: no file of it outside its project.
code=0
in_project "$RUN_PROJECT" --json run "$E2E_FLOOR/relay@0.1.0" "text=asked 0.1.0" --on local > "$OUT/floor-held-older.ndjson" 2> "$OUT/floor-held-older.err" || code=$?
[ "$code" != 0 ] && grep -q '"runtime_held"' "$OUT/floor-held-older.ndjson" "$OUT/floor-held-older.err" && grep -q 'cozy machine update local' "$OUT/floor-held-older.ndjson" "$OUT/floor-held-older.err" \
  || check "runtime floor: held on 0.45.0, e2e-floor 0.1.0 in a project wasn't refused runtime_held naming cozy machine update local: exit $code $(tail -c 300 "$OUT/floor-held-older.ndjson") $(tail -c 300 "$OUT/floor-held-older.err")"
local marker=CL1055_HELD_$RANDOM$RANDOM
code=0
in_project "$RUN_PROJECT" --json run e2e-scratch/probe "marker=$marker" --on local > "$OUT/floor-held-probe.ndjson" 2> "$OUT/floor-held-probe.err" || code=$?
[ "$code" != 0 ] && grep -q '"runtime_held"' "$OUT/floor-held-probe.ndjson" "$OUT/floor-held-probe.err" \
  || check "runtime floor: held on 0.45.0, e2e-scratch's probe in a project wasn't refused runtime_held: exit $code $(tail -c 300 "$OUT/floor-held-probe.ndjson")"
! grep -rqs "$marker" "$HOME/.cozy/machine" "$RUN_PROJECT/.vault-machine" \
  || check "runtime floor: the refused probe wrote its marker: $(grep -rls "$marker" "$HOME/.cozy/machine" "$RUN_PROJECT/.vault-machine" | head -3)"
cozy --json machine update local -y > "$OUT/floor-unhold.json" 2>&1 \
  || { check "runtime floor: cozy machine update local (its Hub's pin) failed: $(tail -c 300 "$OUT/floor-unhold.json")"; return; }
machine_ready
jq -e '.status.runtime == "0.45.0"' "$OUT/machine.json" > /dev/null || check "runtime floor: off its hold, local runs $(jq -r .status.runtime "$OUT/machine.json"), not its Hub's pinned 0.45.0"
# xs-191: an RC build named by hand is a pin too. se-131's aishela, given the RC's TensorFS and cozy by version (each said
# `(RC)`), was moved off them as the RC moved; held, local refuses e2e-floor 0.2.0 as above, and nothing named lets it go.
# The RC's cozy is named: its TensorFS is 0.25's since 0.25.0's merge-back and doesn't install beside the held Runtime 0.45.0.
# The RC is the one this cozy's version names: a dev build's (0.26.0-dev) its own minor, a release's the next (as rc.rs reads).
local rc td was=$OUT/cozy-before-rc-pin
rc=$(cozy --version | awk '{n = split($2, v, /[.-]/); print v[1] "." v[2] + ($2 ~ /-/ ? 0 : 1) ".0"}')
td=$(curl -fsSL "https://github.com/cozy-creator/cozy-creator/releases/download/rc-$rc/SHA256SUMS" 2> /dev/null \
  | sed -n 's/.*  cozy-\(.*\)-linux-x86_64$/\1/p' | head -1) || td=
if [ -n "$td" ]; then
  cp "$COZY" "$was"
  cozy --json machine update local --tensord "$td" -y > "$OUT/floor-rc-pin.json" 2>&1 \
    || { check "runtime floor: local refused the RC's cozy $td: $(tail -c 300 "$OUT/floor-rc-pin.json")"; return; }
  machine_ready
  # This computer's cozy is the RC's build now, Calcifer with it, and the RC's prerelease cozy is rebuilt only for TensorD's
  # crates: a project the candidate opened may be a newer schema than it reads, which it refuses (rightly). The pin's run
  # is in a project the RC's build makes.
  run_project "$OUT/floor-rc-project" > "$OUT/floor-rc-project.log" 2>&1 \
    || { check "runtime floor: the RC's cozy $td made no project: $(tail -c 300 "$OUT/floor-rc-project.log")"; return; }
  code=0
  in_project "$OUT/floor-rc-project" --json run "$E2E_FLOOR/relay" text=rc-pin --on local > "$OUT/floor-rc-pin.ndjson" 2> "$OUT/floor-rc-pin.err" || code=$?
  [ "$code" != 0 ] && grep -q '"runtime_held"' "$OUT/floor-rc-pin.ndjson" "$OUT/floor-rc-pin.err" \
    || check "runtime floor: given the RC's cozy $td by hand, local wasn't held: exit $code $(tail -c 300 "$OUT/floor-rc-pin.ndjson") $(tail -c 300 "$OUT/floor-rc-pin.err")"
  # This computer's cozy back as it was, then let go (an update naming nothing; the Hub's target names no cozy).
  cozy --json machine update local --tensord "$was" -y > "$OUT/floor-rc-back.json" 2>&1 \
    || { check "runtime floor: local refused its own cozy back after the RC pin: $(tail -c 300 "$OUT/floor-rc-back.json")"; return; }
  cozy --json machine update local -y > "$OUT/floor-rc-unpin.json" 2>&1 \
    || { check "runtime floor: cozy machine update local failed after the RC pin: $(tail -c 300 "$OUT/floor-rc-unpin.json")"; return; }
  machine_ready
  [ "$(cozy --version)" = "$("$was" --version)" ] || check "runtime floor: after the RC pin this computer's cozy is $(cozy --version), not $("$was" --version)"
  jq -e '.status.runtime == "0.45.0"' "$OUT/machine.json" > /dev/null || check "runtime floor: after the RC pin, local runs $(jq -r .status.runtime "$OUT/machine.json"), not 0.45.0"
else
  echo "runtime floor: rc-$rc holds no cozy for linux x86_64: an RC pin isn't asked here"
fi
# Not held, on the Hub's 0.45.0: a project's run moves local up to this cozy's Runtime minor first, saying so (cl-1055), and its
# call's temporary file is in its project's scratch.
in_project "$RUN_PROJECT" run e2e-scratch/probe "marker=CL1055_MOVED_$RANDOM" --on local > "$OUT/floor-upgrade.out" 2> "$OUT/floor-upgrade.err" \
  || check "runtime floor: a project's probe run on local (0.45.0, not held) failed: $(tail -c 400 "$OUT/floor-upgrade.err")"
grep -q "Updating local.s Runtime from 0.45.0 to 0.4[7-9][^ ]* for this project" "$OUT/floor-upgrade.err" \
  || check "runtime floor: the run didn't say it upgrades local's Runtime for this project: $(tail -c 400 "$OUT/floor-upgrade.err")"
local temporary
temporary=$(awk '$1 == "temporary" { print $2 }' "$OUT/floor-upgrade.out" | tail -1)
case $temporary in
  "$(realpath "$RUN_PROJECT")"/.vault-machine/scratch/*) ;;
  *) check "runtime floor: the moved run's temporary file was ${temporary:-nowhere said}, not in its project's scratch" ;;
esac
machine_ready
jq -e '.status.runtime | test("^0\\.4[7-9]\\.")' "$OUT/machine.json" > /dev/null \
  || check "runtime floor: after its upgrade local runs $(jq -r .status.runtime "$OUT/machine.json"), not this cozy's Runtime minor"
# On the minor, e2e-floor 0.2.0 (cozy-runtime>=0.45.1) runs as it is.
cell floor-on-minor "$E2E_FLOOR/relay" "text=on the minor" || true
# A cap binds only on a release: a dev build meets every cap (api::release::runs; the RC's builds run packages capped below its
# minor, cl-922), and until a release of this minor is on PyPI every Runtime a project's run can take is a dev build. So local is held
# by hand on the candidate's Runtime as a release of the minor (minor_release). There a named older release its Runtime admits runs
# (e2e-floor 0.1.0); e2e-capped 0.2.0 (cozy-runtime<0.47.0.dev0) can't run, and local never moves down, so the run takes 0.1.0 and
# says why; and off the RC, an unversioned run never takes a prerelease (e2e-held 0.1.0, never 0.2.0rc1: se-128).
local release
release=$(minor_release) || { check "runtime floor: no release of the minor to hold local on: $release"; return; }
cozy --json machine update local --runtime "$release" -y > "$OUT/floor-release.json" 2>&1 \
  || { check "runtime floor: local refused the candidate's Runtime as a release ($release): $(tail -c 300 "$OUT/floor-release.json")"; return; }
machine_ready
jq -e '.status.runtime | test("^[0-9]+\\.[0-9]+\\.[0-9]+$")' "$OUT/machine.json" > /dev/null \
  || check "runtime floor: held on $release, local runs $(jq -r .status.runtime "$OUT/machine.json"), not a release"
if cell floor-release-older "$E2E_FLOOR/relay@0.1.0" "text=held, asked 0.1.0"; then
  grep -q 'relay@0\.1\.0"' "$OUT/floor-release-older.ndjson" \
    || check "runtime floor: held on a release of the minor, e2e-floor 0.1.0 named didn't run as 0.1.0: $(grep -o '"target": *"[^"]*"' "$OUT/floor-release-older.ndjson" | head -1)"
fi
in_project "$RUN_PROJECT" run "$E2E_CAPPED/relay" text=capped --on local > "$OUT/floor-capped.out" 2> "$OUT/floor-capped.err" \
  || check "runtime floor: the unversioned e2e-capped run failed: $(tail -c 400 "$OUT/floor-capped.err")"
grep -q "e2e-capped 0.2.0 needs cozy-runtime.*no Runtime it can move to does, so this run uses 0.1.0" "$OUT/floor-capped.err" \
  || check "runtime floor: on a release of the minor, e2e-capped didn't say it takes 0.1.0 for 0.2.0's cap: $(tail -c 400 "$OUT/floor-capped.err")"
if cell floor-release-held "$E2E_HELD/relay" "text=off the RC"; then
  grep -q 'relay@0\.1\.0"' "$OUT/floor-release-held.ndjson" && ! grep -q '0\.2\.0rc' "$OUT/floor-release-held.ndjson" \
    || check "runtime floor: off the RC on a release of the minor, the unversioned e2e-held run didn't take 0.1.0: $(grep -o '"target": *"[^"]*"' "$OUT/floor-release-held.ndjson" | head -1)"
fi
}

# cl-1160 (0.26.0 blocker; Paul: version skew both ways): Runtime 0.46 sends a managed call's files inside the callee's request, where
# 0.47 passes them by `assets=`; the RC's TensorD refused them ("its files come by assets="), so on a rented machine still on 0.46, every
# minimax-h3 0.12.0 shot with references failed (gate 44053323560). stand-old is a machine of this candidate cozy on cozy 0.25.0's
# Runtime and TensorFS, as such a pod is; isolated.sh published e2e-oldcall 0.1.0, locked on Runtime 0.46.0, whose `relay` passes its
# files on to `seen` by a managed call. A project's run there completes on 0.46.0, and the callee read the bytes it was given.
old_call_files() {
step "a package of Runtime 0.46 passes files to a function it calls, on a machine still on 0.46 (cl-1160)"
[ "${RUNNER_OS:-}" = Linux ] && [ "${GITHUB_ACTIONS:-}" = true ] || { echo "old call files: its machine is another user of a fresh GitHub runner"; return; }
[ -n "${E2E_OLDCALL:-}" ] || { check "old call files: isolated.sh published no e2e-oldcall: ${E2E_OLDCALL_REFUSED:-not seeded}"; return; }
local file=$OUT/oldcall.txt want got
cozy --json machine show local > "$OUT/set-local.json" || { check "old call files: machine show local: $(head -c 300 "$OUT/set-local.json")"; return; }
CAND_RT=0.46.0 CAND_TFS=0.25.1 stand_in stand-old || return
printf 'old call %s\n' "$RANDOM$RANDOM" > "$file"
want="notes=sha256:$(sha256sum "$file" | cut -d' ' -f1)"
if run_cell "$OUT" old-call "$E2E_OLDCALL/relay" --file "notes=$file" --on stand-old; then
  got=$(run_get "$OUT" old-call '[.result.files[]?, .result.runtime, .measurements.software.runtime] | join(" ")')
  echo "old call files: run $(cat "$OUT/old-call.number") on stand-old: $got"
  [ "$got" = "$want 0.46.0 0.46.0" ] || check "old call files: its callee read '$got', not '$want' on Runtime 0.46.0"
else
  check "old call files: run $(cat "$OUT/old-call.number" 2> /dev/null) on stand-old $(run_get "$OUT" old-call .status): $(run_get "$OUT" old-call .reason.message | head -c 400) $(tail -c 300 "$OUT/old-call.err")"
fi
timeout 60 "$COZY" --json machine end stand-old --force -y > "$OUT/stand-old-end.json" 2>&1 || true
sudo pkill -KILL -u cozy-stand-old || true
}

# cl-1109 (coordinator, 11-16): a model asked with no release takes "finals first, else the newest", the same rule as packages, so
# one rule everywhere. xs-203 met it on packages: RC machines took minimax-h3 0.13.0rc2 over the 0.12.0 final. isolated.sh publishes
# e2e/final-model 0.1.0 and then 0.2.0-rc.1. Calcifer's `model show` and `model ls`, and the Hub's resolution (`model install` on
# local; the TensorFS SDK online) take 0.1.0; the SDK takes 0.2.0-rc.1 when it names it; offline, holding both, TensorFS's own
# resolution takes 0.1.0.
final_model() {
step "a model asked with no release takes its newest final, a prerelease label only when named (cl-1109)"
[ -n "$LOCAL" ] || return 0
local model=e2e/final-model dir=$OUT/final-model with said
mkdir -p "$dir"
cozy --json model show "$model" > "$dir/show.json" 2>&1
said=$(jq -r '.release // empty' "$dir/show.json" 2> /dev/null)
[ "$said" = 0.1.0 ] || check "final model: model show $model says ${said:-no release}, not 0.1.0: $(head -c 300 "$dir/show.json")"
cozy --json model ls final-model > "$dir/ls.json" 2>&1
said=$(jq -c --arg m "$model" '[.models[]? | select(.model == $m) | .release]' "$dir/ls.json" 2> /dev/null)
[ "$said" = '["0.1.0"]' ] || check "final model: model ls lists $model at ${said:-nothing}, not 0.1.0: $(head -c 300 "$dir/ls.json")"
cozy --json model install "$model" --on local > "$dir/install.ndjson" 2>&1 \
  || check "final model: model install $model --on local failed: $(tail -c 300 "$dir/install.ndjson")"
cozy --json model ls --on local > "$dir/installed.json" 2>&1
said=$(jq -c --arg m "$model" '[.models[]? | select(.model == $m) | .release] | unique' "$dir/installed.json" 2> /dev/null)
[ "$said" = '["0.1.0"]' ] || check "final model: an unpinned install put $model ${said:-nothing} on local, not 0.1.0"
# The TensorFS SDK, against this Hub, in a store of its own.
[ -n "${CAND_TFS:-}" ] || { echo "final model: the SDK's part skipped, this run names no TensorFS"; return 0; }
if [ -f "$CAND_TFS" ]; then with=$CAND_TFS; else with=tensorfs==${CAND_TFS#v}; fi
# This Hub and its store are on loopback: declared local for the SDK as for cozy (`local_origins`).
TENSORFS_LOCAL_ORIGINS=${COZY_LOCAL_ORIGINS:-} uv run -q --no-project --with "$with" python -I - "$model" "$dir/store" > "$dir/sdk.json" 2> "$dir/sdk.err" <<'PY' \
  || { check "final model: the TensorFS SDK failed: $(tail -n 1 "$dir/sdk.err" | cut -c1-300)"; return; }
import json, os, ssl, sys
import tensorfs
ca = os.environ.get("COZY_TEST_HUB_CA")
if ca and os.path.isfile(ca):
    with open(ca) as pem:
        tensorfs.configure(ca_der=ssl.PEM_cert_to_DER_cert(pem.read()))
model, store = sys.argv[1], sys.argv[2]
def release(**asked):
    with tensorfs.open(model, cache_dir=store, **asked) as checkpoint:
        return checkpoint.release
said = {"online": release(), "pinned": release(release="0.2.0-rc.1")}
release(release="0.1.0")
said["offline"] = release(local_files_only=True)
print(json.dumps(said))
PY
echo "final model: the SDK: $(cat "$dir/sdk.json")"
J '.online == "0.1.0"' "$dir/sdk.json" || check "final model: the SDK online took $(jq -r .online "$dir/sdk.json"), not 0.1.0"
J '.pinned == "0.2.0-rc.1"' "$dir/sdk.json" || check "final model: the SDK naming 0.2.0-rc.1 took $(jq -r .pinned "$dir/sdk.json")"
J '.offline == "0.1.0"' "$dir/sdk.json" || check "final model: offline, holding 0.1.0 and 0.2.0-rc.1, TensorFS took $(jq -r .offline "$dir/sdk.json"), not 0.1.0"
}

# minor_release: the candidate's Runtime wheel as a release of its minor (0.47.0.dev0+dev.g… as 0.47.0), for a cell that needs a cap
# to bind; made once, its path said. Fails, saying why, without a candidate wheel.
minor_release() {
  local dir=$OUT/minor-release made
  [ -f "${CAND_RT:-}" ] || { echo "no candidate Runtime wheel (${CAND_RT:-none})"; return 1; }
  made=$(find "$dir" -name '*.whl' -print -quit 2> /dev/null) || made=
  [ -n "$made" ] || made=$(mkdir -p "$dir" && python3 -I - "$CAND_RT" "$dir" <<'PY'
import base64, hashlib, re, sys, zipfile
from pathlib import Path
source, out = Path(sys.argv[1]), Path(sys.argv[2])
name, version, rest = source.name.split("-", 2)
release = re.match(r"\d+(?:\.\d+)*", version).group(0)
target = out / f"{name}-{release}-{rest}"
with zipfile.ZipFile(source) as wheel:
    info = next(n for n in wheel.namelist() if n.endswith(".dist-info/METADATA")).rsplit("/", 1)[0]
    renamed = f"{name}-{release}.dist-info"
    record = []
    with zipfile.ZipFile(target, "w", zipfile.ZIP_DEFLATED) as made:
        for item in wheel.infolist():
            if item.filename == f"{info}/RECORD":
                continue
            data = wheel.read(item)
            path = renamed + item.filename[len(info):] if item.filename.startswith(info + "/") else item.filename
            if path == f"{renamed}/METADATA":
                data = re.sub(rb"(?m)^Version: .*$", b"Version: " + release.encode(), data, count=1)
            entry = zipfile.ZipInfo(path, item.date_time)
            entry.external_attr, entry.compress_type = item.external_attr, zipfile.ZIP_DEFLATED
            made.writestr(entry, data)
            digest = base64.urlsafe_b64encode(hashlib.sha256(data).digest()).rstrip(b"=").decode()
            record.append(f"{path},sha256={digest},{len(data)}")
        record.append(f"{renamed}/RECORD,,")
        made.writestr(f"{renamed}/RECORD", "\n".join(record) + "\n")
print(target)
PY
) || { echo "the candidate's wheel $CAND_RT didn't repack as a release"; return 1; }
  echo "$made"
}

# cr-276: a package calling another package's function whose result holds a `Latents` field, as orbit calls minimax-h3's
# `generate` for its `ShotResult`: the callee, a path dependency, fills the field with a file it saved. The caller gets that
# field as a `Latents` holding the callee's bytes, and the `File` beside it as a `File`. Runtime 0.45.0 failed the call in the
# caller: `child_result_invalid`, "Expected `Latents`, got `File` - at `$.latents`". The caller has no uv.lock: cozy locks
# it at the run, path dependency and all (cl-878); before, it was refused `unlocked_dependency`.
child_latents() {
step "a child result holding Latents, in another package's caller (cr-276)"
[ -n "$LOCAL" ] || return 0
local dir=$OUT/child-latents
rm -rf "$dir"
package_at "$dir/maker" e2e_latents_maker '"cozy-runtime>=0.47.0.dev0,<0.48", "msgspec>=0.19"' <<'PY'
import hashlib
from typing import cast

import msgspec

from cozy_runtime.author import App, Outputs
from cozy_runtime.media import File, Latents

app = App()


class Ask(msgspec.Struct, forbid_unknown_fields=True):
    text: str


class Shot(msgspec.Struct):
    clip: File
    latents: Latents | None = None
    sha256: str = ""


@app.entrypoint(accelerator=False)
def shot(payload: Ask, out: Outputs) -> Shot:
    data = (payload.text * 64).encode()
    clip = out.save_bytes(b"clip " + data, media_type="application/octet-stream")
    kept = out.save_bytes(data, media_type="application/octet-stream")  # a saved file fills a Latents field
    return Shot(clip=clip, latents=cast(Latents, kept), sha256=f"sha256:{hashlib.sha256(data).hexdigest()}")
PY
package_at "$dir/caller" e2e_latents_caller '"cozy-runtime>=0.47.0.dev0,<0.48", "msgspec>=0.19", "e2e-latents-maker"' \
  "$(printf '\n[tool.uv.sources]\ne2e-latents-maker = { path = "../maker" }')" <<'PY'
import hashlib
from importlib.metadata import version

import msgspec
from e2e_latents_calls import Ask as MakerAsk, shot

from cozy_runtime.author import App, call

app = App()


class Ask(msgspec.Struct, forbid_unknown_fields=True):
    text: str


class Got(msgspec.Struct):
    latents: str
    clip: str
    same_bytes: bool
    runtime: str


async def relay(payload: Ask) -> Got:
    made = await call(shot, MakerAsk(text=payload.text))
    data = b"" if made.latents is None else made.latents.read_bytes()
    return Got(
        latents=type(made.latents).__name__,
        clip=type(made.clip).__name__,
        same_bytes=f"sha256:{hashlib.sha256(data).hexdigest()}" == made.sha256,
        runtime=version("cozy-runtime"),
    )


app.entrypoint(relay, accelerator=False)
PY
cp "$here/latents_maker_calls.py" "$dir/caller/e2e_latents_calls.py"
sed -i 's/only-include = \["e2e_latents_caller.py"\]/only-include = ["e2e_latents_caller.py", "e2e_latents_calls.py"]/' "$dir/caller/pyproject.toml"
# No uv.lock: cozy locks it at the run, its path dependency included, and writes the lock beside it (cl-878).
cell child-latents "$dir/caller/relay" "text=latents" || return 0
grep -q 'directory = "../maker"' "$dir/caller/uv.lock" 2> /dev/null \
  || check "child latents: the caller's folder holds no uv.lock naming its path dependency: $(ls "$dir/caller" | paste -sd' ')"
# What still has no lock, the caller's tree described without its uv.lock as TensorD describes in the environment its run built
# (`ask_runtime`), is refused saying so (its exit is the refusal's), not "add it to [project].dependencies", which names it.
local py stale=$OUT/child-latents-stale
py=$(ls -d "$HOME"/.cozy/machine/var/lib/cozy/tensord/editable/*/e2e-latents-caller/env-*/bin/python 2> /dev/null | head -1) || true
rm -rf "$stale" && cp -r "$dir" "$stale" && rm -f "$stale/caller/uv.lock"
jq -n --arg project "$stale/caller" --arg python "$py" '{kind: "describe", project: $project, environment_python: $python}' \
  | "$py" -I -S -c "import pathlib,runpy,sys;root=pathlib.Path(sys.executable).absolute().parent.parent;\
sys.path[:0]=[str(p) for p in (root/'lib').glob('python*/site-packages')];sys.argv=['cozy-runtime','machine-describe'];\
runpy.run_module('cozy_runtime.cli.main',run_name='__main__')" > "$OUT/child-latents-stale.json" 2> "$OUT/child-latents-stale.err" || true
echo "child latents, unlocked: $(jq -r '"\(.code // .kind): \(.detail // "")"' "$OUT/child-latents-stale.json" 2> /dev/null | head -c 300)"
jq -e '.code == "unlocked_dependency" and (.detail | test("no uv.lock"))' "$OUT/child-latents-stale.json" > /dev/null 2>&1 \
  || check "child latents: its unlocked tree was not refused for its missing lock: $(tail -c 300 "$OUT/child-latents-stale.json") $(tail -c 200 "$OUT/child-latents-stale.err")"
echo "child latents: $(grep -oE '"(latents|clip|same_bytes|runtime)": *("[^"]*"|true|false)' "$OUT/child-latents.ndjson" | sort -u | paste -sd' ')"
grep -q '"latents": *"Latents"' "$OUT/child-latents.ndjson" && grep -q '"clip": *"File"' "$OUT/child-latents.ndjson" \
  && grep -q '"same_bytes": *true' "$OUT/child-latents.ndjson" \
  || check "child latents: the caller did not get the callee's Latents and File: $(tail -c 300 "$OUT/child-latents.ndjson")"
}

# cl-820: a signer this machine admits sends TensorD a manifest that names a host path, as only a hand-made one can (no cozy sends
# one): it is refused, typed, before the run is accepted, so nothing installs from that folder.
path_manifest() {
step "a manifest naming a host path is refused"
[ -n "$LOCAL" ] || return 0
local pem=$OUT/signer.pem key='' code=0
for _ in $(seq 100); do
  openssl genpkey -algorithm ed25519 -out "$pem" 2> /dev/null || { check "path manifest: openssl could not make an Ed25519 key"; return; }
  key=$(openssl pkey -in "$pem" -pubout -outform DER | tail -c 32 | base64 | tr '+/' '-_' | tr -d '=\n')
  [[ $key == -* ]] || break
done
cozy --json machine authorize "$key" > "$OUT/path-authorize.json" || { check "path manifest: machine authorize failed"; return; }
cozy --json machine show local > "$OUT/path-local.json" || check "path manifest: machine show local failed"
uv run -q --no-project --with cryptography python -I - "$pem" "$(jq -r .address "$OUT/path-local.json")" \
  "$(jq -r .fingerprint "$OUT/path-local.json")" "$here/weightless" "$OUT/path-refused.json" <<'PY' || code=$?
import base64, hashlib, http.client, json, os, ssl, sys, time
from cryptography.hazmat.primitives.serialization import load_pem_private_key

pem, address, fingerprint, project, out = sys.argv[1:]
key = load_pem_private_key(open(pem, "rb").read(), None)
b64 = lambda raw: base64.urlsafe_b64encode(raw).rstrip(b"=").decode()
host, port = address.split("://")[-1].rsplit(":", 1)
tls = ssl.create_default_context()
tls.check_hostname, tls.verify_mode = False, ssl.CERT_NONE

def call(method, path, body=b"", token=""):
    connection = http.client.HTTPSConnection(host.strip("[]"), int(port), context=tls, timeout=60)
    connection.connect()
    leaf = hashlib.sha256(connection.sock.getpeercert(binary_form=True)).hexdigest()
    assert leaf == fingerprint.removeprefix("sha256:"), f"{address} presents the leaf {leaf}, not {fingerprint}"
    headers = {"content-length": str(len(body))} | ({"authorization": f"Cozy-Cap {token}"} if token else {})
    connection.request(method, path, body=body, headers=headers)
    answer = connection.getresponse()
    return answer.status, answer.read()

worker = json.loads(call("GET", "/v1/status")[1])["worker_id"]
public = key.public_key().public_bytes_raw()
grant = json.dumps({"m": worker, "a": "machine", "e": int(time.time()) + 600, "k": b64(hashlib.sha256(public).digest()[:16])}).encode()
signed = key.sign(b"cozy-capability/1\0" + grant)  # api::machine::capability's DOMAIN, then the grant
token = f"{b64(grant)}.{b64(signed)}"
# The fixture's folder on this computer, as a manifest named it before cl-817 (`in_place`).
manifest = json.dumps({"package": "local/e2e-weightless", "release": "0.1.0", "path": os.path.dirname(project),
                       "project": os.path.basename(project)}).encode()
digest = "sha256:" + hashlib.sha256(manifest).hexdigest()
status, body = call("PUT", f"/v1/objects/{digest}", manifest, token)
assert status < 300, f"PUT /v1/objects/{digest}: {status} {body[:300]!r}"
run = {"id": f"e2e-path-{time.time_ns()}", "spec": {"kind": "call", "source": {"local": {"manifest": digest}},
                                                   "entrypoint": "reverse", "payload": {"text": "abc"}}}
status, body = call("POST", "/v1/runs/batch", json.dumps({"runs": [run]}).encode(), token)
print(f"POST /v1/runs/batch, a manifest naming {os.path.dirname(project)}: {status} {body[:300].decode()}")
refusal = json.loads(body)["runs"][0].get("refusal") if status == 200 else None  # a batch answers each run's refusal
open(out, "w").write(json.dumps({"error": refusal}) if refusal else body.decode())
sys.exit(0 if refusal else 1)
PY
cozy --json machine authorize "$key" --remove > /dev/null || check "path manifest: machine authorize --remove failed"
[ "$code" = 0 ] && J '.error.code == "local_source_path_refused"' "$OUT/path-refused.json" \
  || check "path manifest: not refused local_source_path_refused (exit $code): $(head -c 300 "$OUT/path-refused.json" 2> /dev/null)"
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

# Run options and the retained warming API.
run_flags() {
step "run flags"
before=$(cozy --json run ls --limit 0 | jq '.runs | length') || true
(in_project "$RUN_PROJECT" --json run "$here/weightless/reverse" --describe) > "$OUT/describe.json" || check "run --describe failed"
J '.fields | any(.name == "text")' "$OUT/describe.json" || check "run --describe: no text field"
[ "$(cozy --json run ls --limit 0 | jq '.runs | length')" = "$before" ] || check "run --describe started a run"
if in_project "$RUN_PROJECT" --json run "$here/weightless/reverse" text=test --model missing=owner/example > "$OUT/model-slot.json" 2> "$OUT/model-slot.err"; then
  check "--model accepted a slot the function does not declare"
fi
J '.error.code == "model_slot_unknown"' "$OUT/model-slot.json" || check "--model did not reach slot validation: $(cat "$OUT/model-slot.json")"
note=$OUT/inputs/note.txt; mkdir -p "$OUT/inputs"; echo "an asset" > "$note"
if (cd "$here" && cell fidelity ./weightless/fidelities --file "notes=$note" --file-fidelity notes=high); then
  grep -q 'notes=high' "$OUT/fidelity.ndjson" || check "--file-fidelity notes=high: the executor saw $(grep -o '"assets":\[[^]]*\]' "$OUT/fidelity.ndjson")"
fi
for level in installed downloaded imported host; do
  run_warm_cell "$OUT" "warm-$level" "$(birefnet)" "$level" --on local || check "warming API at $level failed"
  cozy --json machine show local > "$OUT/warm-$level.json" || true
  # The warm set holds the level asked, or says why it holds less (a CPU binds no model on a GPU).
  J --arg l "$level" '.status.warm | any(.entrypoint == "remove_background" and .level == $l and (.holds == $l or (.held_back | length > 0)))' "$OUT/warm-$level.json" \
    || check "warming API at $level: the warm set does not hold birefnet at $level: $(jq -c '.status.warm' "$OUT/warm-$level.json")"
done
run_warm_cell "$OUT" warm-off "$(birefnet)" off --on local || check "warming API at off failed"
cozy --json machine show local | J '.status.warm | all(.entrypoint != "remove_background")' || check "warming API at off: birefnet is still in the warm set"
}

# Calcifer's time left, on a weightless run whose stages and steps time it as a model's do (tool.py's `stepped`, its first step slow as a
# warm-up is). Its first run here says `first_run`, with no number, until it has a step of its own: then that step's pace (`steps`,
# cl-921). With history, each report has a
# number from the first; at step 2 it is within 35% of what was left, and once three steps are in a step moves it at most 25% past its
# countdown (and a second of rounding). Both are from replaying Paul's Qwen Turbo runs 171-208 through the estimator: 10 of 14 were within
# 35% at step 2 (of the four past it, 181 and 208 spent 70 s encoding their prompt, and 195 and 196 ran on a Runtime half again as fast as
# their history), and no step after the third moved one more than 23%. A weightless fixture is steadier than any of them.
time_left() {
step "time left: a first run, then from history"
local turn had end out
had=$(cozy --json run ls --package local/tool/stepped --state completed --limit 1 | jq '.runs | length')
for turn in 1 2 3; do
  out=$OUT/time-left-$turn
  cell "time-left-$turn" "$here/project/tool.py/stepped" prompt=time "seed:=$RANDOM$turn" steps:=8 step:=0.8 first:=2.4 || continue
  end=$(jq '.started_at_ms + .execution_ms' "$out.json")
  grep '^{' "$out.ndjson" | jq -c 'select(.type == "run.progress") | .progress' > "$out.progress"
  if [ "$turn" = 1 ] && [ "$had" = 0 ]; then
    jq -se 'map(select((.completed // 0) < 1)) | length > 0 and all(.eta_s == null and .eta_basis == "first_run")' "$out.progress" > /dev/null \
      || check "time left: a first run had a number before a step of its own: $(head -c 600 "$out.progress")"
    jq -se 'map(select(.stage == "generating image" and (.completed // 0) >= 1)) | length > 0 and all(.eta_s != null and .eta_basis == "steps")' "$out.progress" > /dev/null \
      || check "time left: a first run with a step of its own had no number: $(jq -sc 'map(select(.stage == "generating image")) | .[:3]' "$out.progress")"
    continue
  fi
  jq -se 'length > 0 and all(.eta_s != null and .eta_basis == "machine")' "$out.progress" > /dev/null \
    || check "time left: run $turn had history and a report without its number: $(jq -sc 'map(select(.eta_s == null)) | .[:3]' "$out.progress")"
  jq -se --argjson end "$end" 'map(select(.stage == "generating image" and .completed >= 2)) | .[0] | (($end - .at_ms) / 1000) as $truth
      | (.eta_s - $truth | fabs) <= 0.35 * $truth + 1' "$out.progress" > /dev/null \
    || check "time left: at step 2, more than 35% from what was left: $(jq -sc --argjson end "$end" 'map(select(.completed >= 2)) | .[0] | {eta_s, truth: (($end - .at_ms) / 1000)}' "$out.progress")"
  jq -se 'map(select(.stage == "generating image" and .completed >= 3)) | group_by(.completed) | map(.[0])
      | [range(1; length) as $k | .[$k - 1] as $a | .[$k] as $b | ($a.eta_s - ($b.at_ms - $a.at_ms) / 1000) as $counted
         | ((($b.eta_s - $counted) | fabs) - 1) / ([$a.eta_s, 1] | max)] | all(. <= 0.25)' "$out.progress" > /dev/null \
    || check "time left: a step moved it more than 25% past its countdown: $(jq -sc 'map(select(.stage == "generating image")) | group_by(.completed) | map(.[0] | {completed, eta_s, at_ms})' "$out.progress")"
done
}

run_follow() {
step "a run followed, a snapshot, a cancel"
(cd "$here" && in_project "$RUN_PROJECT" --json run "$here/weightless/wait" seconds:=120 --on local > "$OUT/wait.ndjson" 2>&1) &
waiting=$!
# Its own events say its number, then that it runs (e2e/run.sh); its follower exiting first ends the wait.
until n=$(run_number "$OUT/wait.ndjson"); [ -n "$n" ] || ! kill -0 "$waiting" 2> /dev/null; do sleep 0.2; done
if [ -z "$n" ]; then check "the waiting run never got a number"; else
  until run_reached "$OUT/wait.ndjson" in_progress || ! kill -0 "$waiting" 2> /dev/null; do sleep 0.2; done
  cozy --json run watch "$n" --once > "$OUT/snapshot.json" || true
  J '.status == "in_progress"' "$OUT/snapshot.json" || check "run watch --once: run $n is $(jq -r .status "$OUT/snapshot.json"), not in_progress"
  cozy --json run watch "$n" > "$OUT/followed.ndjson" &
  following=$!
  sleep 3
  kill -0 "$following" 2> /dev/null || check "run watch $n stopped before the run settled (it should follow it)"
  cozy --json run cancel "$n" > "$OUT/cancel.json" || check "run cancel $n failed"
  wait "$following" || true
  wait "$waiting" || true
  grep -qE '"status": *"canceled"' "$OUT/followed.ndjson" || check "run watch $n did not follow it to canceled"
  cozy --json run watch "$n" --once > "$OUT/canceled.json" || true
  J '.status == "canceled"' "$OUT/canceled.json" || check "run cancel: run $n is $(jq -r '.status, .reason.code? // empty' "$OUT/canceled.json" | paste -sd' ' -), not canceled"
fi

step "run ls filters"
cozy --json run ls --state canceled > "$OUT/canceled.json" && J --argjson n "${n:-0}" '(.runs | length > 0) and all(.runs[]; .status == "canceled") and any(.runs[]; .number == $n)' "$OUT/canceled.json" \
  || check "run ls --state canceled: wrong runs"
}

# After `basics`: its runs' targets filter (its model installs are jobs, no project's runs: cl-1028).
run_filters() {
step "run ls filters"
local prefix
prefix=$(cozy --json run ls --all --limit 1 | jq -r '.runs[0].target // empty | split("@")[0]') || prefix=
[ -n "$prefix" ] && cozy --json run ls --all --package "$prefix" > "$OUT/by-package.json" \
  && J --arg p "$prefix" '(.runs | length > 0) and all(.runs[]; .target | startswith($p))' "$OUT/by-package.json" \
  || check "run ls --package ${prefix:-?}: none listed, or others listed"
cozy --json run ls --on local > "$OUT/by-machine.json" && J '(.runs | length > 0) and all(.runs[]; .machine == "local")' "$OUT/by-machine.json" \
  || check "run ls --on local: other machines listed"
cozy --json run ls --limit 1 > "$OUT/limit.json" && J '(.runs | length) == 1' "$OUT/limit.json" || check "run ls --limit 1: not one run"
}

# cl-998: a command a run carries out ends a failed run as a typed refusal: after the run's events (`--json`), the envelope with
# the run's reason code and its next step. Each run here fails on the machine: an install whose build has no module, a model file
# whose tensor runs past its end, the same file uploaded.
# run_refused <dir> <name> <exit code>: a failed run among its events, its exit non-zero, and its last line the envelope with that run's code.
run_refused() {
[ "$3" != 0 ] || check "run failures: $2 exited 0"
python3 - "$1/$2.ndjson" <<'PY' || check "run failures: $2 isn't a typed refusal: $(tail -c 500 "$1/$2.ndjson") $(tail -c 300 "$1/$2.err")"
import json, sys
lines = []
for line in open(sys.argv[1]):
  try:
      lines.append(json.loads(line))
  except ValueError:
      pass
failed = [line["run"] for line in lines if isinstance(line, dict) and line.get("type") in ("run.failed", "run.canceled")]
assert failed, "no failed run among its events"
code = (failed[-1].get("reason") or {}).get("code") or "run_failed"
error = (lines[-1] if isinstance(lines[-1], dict) else {}).get("error") or {}
assert error.get("code") == code, f"its envelope says {error.get('code')!r}, its run {code!r}"
assert error.get("next"), "its envelope names no next step"
PY
}

run_failures() {
[ -n "$LOCAL" ] || return 0
step "a failed run's command is a typed refusal (cl-998)"
local root=$OUT/run-failures project bad account code
project=$root/project bad=$root/bad.safetensors
mkdir -p "$root"
cozy --json new --unencrypted "$project" > "$root/new.json" 2>&1 || { check "run failures: cozy new: $(tail -c 300 "$root/new.json")"; return 0; }
# package install: its code builds no module (`uv pip install --editable` fails on the machine; uv lock, here, doesn't build it).
cp -r "$here/weightless" "$root/nomodule"
sed -i 's/^module-name = "e2e_weightless"/module-name = "e2e_no_such_module"/' "$root/nomodule/pyproject.toml"
sed -i 's/^name = "e2e-weightless"/name = "e2e-no-module"/' "$root/nomodule/pyproject.toml"
code=0; (cd "$project" && cozy --json package install "$root/nomodule" --on local) > "$root/install.ndjson" 2> "$root/install.err" || code=$?
run_refused "$root" install "$code"
# model install: a .safetensors file whose one tensor runs past its end.
python3 - "$bad" <<'PY'
import json, struct, sys
header = json.dumps({"w": {"dtype": "F32", "shape": [1024], "data_offsets": [0, 4096]}}).encode()
open(sys.argv[1], "wb").write(struct.pack("<Q", len(header)) + header)
PY
code=0; cozy --json model install "$bad" local/e2e-cl998-bad --on local > "$root/download.ndjson" 2> "$root/download.err" || code=$?
run_refused "$root" download "$code"
# model upload: the same file, signed in to the suite's Hub, then signed out again.
if [ -z "${E2E_HUB_LOG:-}" ] || [ -z "${E2E_HUB_EMAIL:-}" ]; then
  check "run failures: e2e/isolated.sh named no Hub log or email to sign in with"
elif signin "$E2E_HUB_EMAIL" > "$root/login.json"; then
  account=$(cozy --json whoami | jq -r '.account // empty')
  code=0; cozy --json model upload "$bad" "${account:-e2e}/cl998-bad-$RANDOM" --lane bf16 --on local > "$root/upload.ndjson" 2> "$root/upload.err" || code=$?
  run_refused "$root" upload "$code"
  cozy --json logout > "$root/logout.json" 2>&1 || check "run failures: logout: $(tail -c 300 "$root/logout.json")"
else
  check "run failures: cozy login failed: $(tail -c 300 "$root/login.json")"
fi
}

# cl-1083, from #3423's review: "`cozy model quantize` outside a project is now allowed but no cell runs it to completion". Signed in to
# the suite's Hub: the tensorfs gate's two-stack model (release-gate/suites/tensorfs/multi-conversion, CPU only) made by its `source`
# in a project and uploaded as bf16, then quantized to int8 from outside any project, as one of Calcifer's jobs on `local` (job type
# `quantize`, no project's run). The lane's commit is read back from the Hub by `inspect`, in the project: every weight encoded
# int8.convrot and near the source, every bias the source's bytes.
quantize_outside() {
[ -n "$LOCAL" ] || return 0
step "cozy model quantize outside a project, carried out to the end (cl-1083)"
local root=$OUT/quantize-outside seed=$((RANDOM % 9000 + 1000)) code=0 account model made bf16 int8 job
local pkg=$root/ci-multi-conversion project=$root/project outside=$root/outside
rm -rf "$root" && mkdir -p "$outside" && cp -r "$here/../../release-gate/suites/tensorfs/multi-conversion" "$pkg" \
  || { check "quantize outside: no copy of the multi-conversion package"; return 0; }
[ -n "${E2E_HUB_LOG:-}" ] && [ -n "${E2E_HUB_EMAIL:-}" ] || { check "quantize outside: e2e/isolated.sh named no Hub log or email to sign in with"; return 0; }
signin "$E2E_HUB_EMAIL" > "$root/login.json" || { check "quantize outside: cozy login failed: $(tail -c 300 "$root/login.json")"; return 0; }
account=$(cozy --json whoami | jq -r '.account // empty' || true)
model=${account:-e2e}/cl1083-two-stacks-$seed
cozy --json new --unencrypted "$project" > "$root/new.json" 2>&1 || { check "quantize outside: cozy new: $(tail -c 300 "$root/new.json")"; cozy --json logout > /dev/null 2>&1; return 0; }
# The bf16 source, made in the project and uploaded from its run.
in_project "$project" --json run "$pkg/source" "seed:=$seed" --on local > "$root/source.ndjson" 2> "$root/source.err" \
  || { check "quantize outside: the source run failed: $(tail -c 300 "$root/source.ndjson") $(tail -c 200 "$root/source.err")"; cozy --json logout > /dev/null 2>&1; return 0; }
made=$(grep '^{' "$root/source.ndjson" | jq -rs '[.. | objects | select(.name? and (.name | test("^[A-Z0-9]+-[0-9]+$")) and .status? == "completed") | .name] | last // empty' || true)
in_project "$project" --json model upload "run:$made#model" "$model" --lane bf16 --on local -y > "$root/upload.ndjson" 2> "$root/upload.err" \
  || { check "quantize outside: uploading run $made's model as $model failed: $(tail -c 300 "$root/upload.ndjson") $(tail -c 200 "$root/upload.err")"; cozy --json logout > /dev/null 2>&1; return 0; }
bf16=$(grep '^{' "$root/upload.ndjson" | jq -rs --arg m "$model" '[.. | objects | select(.published?.destination? == $m) | .published.commit] | last // empty' || true)
[ -n "$bf16" ] || { check "quantize outside: the upload named no commit of $model: $(tail -c 400 "$root/upload.ndjson")"; cozy --json logout > /dev/null 2>&1; return 0; }
# The quantize, from a folder in no project.
(cd "$outside" && cozy --json model quantize "$model@$bf16" --lane int8 --package "$pkg" --on local -y) > "$root/quantize.ndjson" 2> "$root/quantize.err" || code=$?
int8_of() { { grep '^{' "$1" 2> /dev/null | jq -rs '[.. | objects | select(.name? == "int8" and (.checkpoint?.revision? | type == "string")) | .checkpoint.revision] | last // empty'; } || true; }
int8=$(int8_of "$root/quantize.ndjson")
if [ -z "$int8" ]; then  # its whole record, by the job's id
  job=$(grep '^{' "$root/quantize.ndjson" | jq -rs '[.. | objects | select(.id? and .kind? == "call") | .id] | last // empty' || true)
  [ -z "$job" ] || (cd "$outside" && cozy --json --full run watch "$job" --once) > "$root/quantize-record.json" 2> /dev/null || true
  int8=$(int8_of "$root/quantize-record.json")
fi
[ "$code" = 0 ] && [ -n "$int8" ] \
  || { check "quantize outside: cozy model quantize $model@${bf16:-?} --lane int8 outside a project: exit $code, no int8 checkpoint: $(tail -c 400 "$root/quantize.ndjson") $(tail -c 200 "$root/quantize.err")"; cozy --json logout > /dev/null 2>&1; return 0; }
echo "quantize outside: $model@$bf16 (bf16) → int8 $int8"
# One of Calcifer's jobs on `local`, of type quantize, that completed; in state.db, which holds no project's run.
job=$(python3 - "$HOME/.cozy/calcifer/state.db" <<'PY'
import json, sqlite3, sys
c = sqlite3.connect(f"file:{sys.argv[1]}?mode=ro", uri=True, timeout=10)
print(json.dumps([list(r) for r in c.execute("SELECT machine, kind, status FROM runs WHERE task = 'quantize' AND state = 'settled' ORDER BY number DESC LIMIT 1")]))
PY
) || job=
[ "$job" = '[["local", "call", "completed"]]' ] || check "quantize outside: no completed quantize job of Calcifer's on local (machine, kind, status): ${job:-none}"
# The lane, read back from the Hub in the project.
in_project "$project" --json run "$pkg/inspect" "seed:=$seed" "model.model=$model@$int8" --on local > "$root/inspect.ndjson" 2> "$root/inspect.err" \
  && grep -q '"scheme": *"int8.convrot"' "$root/inspect.ndjson" \
  || check "quantize outside: the int8 lane $model@$int8 didn't read back as int8.convrot near its source: $(tail -c 400 "$root/inspect.ndjson") $(tail -c 200 "$root/inspect.err")"
cozy --json logout > "$root/logout.json" 2>&1 || check "quantize outside: logout: $(tail -c 300 "$root/logout.json")"
}

# cl-987: a project's key names its runs, `KEY-N`, from any folder: proposed from its name, kept in its project.db (so it goes
# with its folder), changed with `cozy key` while the key it had names them still. Calcifer's jobs are no project's runs (cl-1028): none
# is listed with them, and none has a name.
project_keys() {
[ -n "$LOCAL" ] || return 0
step "project keys: KEY-N from any folder (cl-987)"
local root=$OUT/keys ka kb na nb ida idb n
local a=$root/kilo-alpha-bravo b=$root/zulu outside=$root/outside
mkdir -p "$outside"
cozy --json new --unencrypted "$a" > "$root-new-a.json" 2>&1 && cozy --json new --unencrypted "$b" > "$root-new-b.json" 2>&1 \
  || { check "keys: cozy new: $(tail -c 300 "$root-new-a.json") $(tail -c 300 "$root-new-b.json")"; return 0; }
ka=$(jq -r .key "$root-new-a.json") kb=$(jq -r .key "$root-new-b.json")
[[ $ka == KAB* ]] || check "keys: kilo-alpha-bravo's key is '$ka', not its words' initials KAB"
[[ $kb == ZUL* ]] || check "keys: zulu's key is '$kb', not its first letters ZUL"
(cd "$here" && RUN_PROJECT=$a cell keys-a ./weightless/reverse "text=keys$RANDOM") || return 0
(cd "$here" && RUN_PROJECT=$b cell keys-b ./weightless/reverse "text=keys$RANDOM") || return 0
na=$(jq -r .name "$OUT/keys-a.json") nb=$(jq -r .name "$OUT/keys-b.json") ida=$(jq -r .id "$OUT/keys-a.json") idb=$(jq -r .id "$OUT/keys-b.json")
[ "$na" = "$ka-$(cat "$OUT/keys-a.number")" ] && [ "$nb" = "$kb-$(cat "$OUT/keys-b.number")" ] \
  || check "keys: the runs are named '$na' and '$nb', not $ka-N and $kb-N"
[ "$ida" != "$idb" ] || check "keys: two projects' runs are one run, $ida"
# From outside both, in any case, and from the other project's folder: each name is its own project's run.
(cd "$outside" && cozy --json run watch "$na" --once) > "$root-watch-a.json" 2>&1 || true
J --arg id "$ida" '.id == $id' "$root-watch-a.json" || check "keys: run watch $na outside both projects: $(tail -c 300 "$root-watch-a.json")"
(cd "$outside" && cozy --json run watch "${nb,,}" --once) > "$root-watch-b.json" 2>&1 || true
J --arg id "$idb" '.id == $id' "$root-watch-b.json" || check "keys: run watch ${nb,,} outside both projects: $(tail -c 300 "$root-watch-b.json")"
(cd "$b" && cozy --json run watch "$na" --once) > "$root-watch-ab.json" 2>&1 || true
J --arg id "$ida" '.id == $id' "$root-watch-ab.json" || check "keys: run watch $na in $kb's folder isn't $ka's run: $(tail -c 300 "$root-watch-ab.json")"
(cd "$outside" && cozy --json run ls --all --limit 0) > "$root-ls.json" 2>&1 || true
J --arg a "$na" --arg b "$nb" '[.runs[].name] | index($a) and index($b)' "$root-ls.json" \
  || check "keys: run ls --all doesn't name $na and $nb: $(jq -c '[.runs[]?.name]' "$root-ls.json")"
(cd "$outside" && cozy run ls --all --limit 0) > "$root-ls.txt" 2>&1 || true
grep -qE "(^| )$na( |$)" "$root-ls.txt" && grep -qE "(^| )$nb( |$)" "$root-ls.txt" || check "keys: run ls's table doesn't name $na and $nb: $(head -c 600 "$root-ls.txt")"
# Calcifer's jobs (the downloads before) are no project's runs (cl-1028): run ls --all lists none.
J '[.runs[] | select(.project_id == null)] | length == 0' "$root-ls.json" \
  || check "keys: run ls --all lists runs of no project: $(jq -c '[.runs[]? | select(.project_id == null) | {id, target}]' "$root-ls.json")"
# A new key names its runs; the key it had names them still, and its project.db keeps both, so they go with the folder.
(cd "$a" && cozy --json key kx987) > "$root-key.json" 2>&1 || true
J --arg was "$ka" '.key == "KX987" and (.former | index($was))' "$root-key.json" || check "keys: cozy key kx987: $(tail -c 300 "$root-key.json")"
n=${na##*-}
for name in "$na" "KX987-$n"; do
  (cd "$outside" && cozy --json run watch "$name" --once) > "$root-renamed.json" 2>&1 || true
  J --arg id "$ida" --arg now "KX987-$n" '.id == $id and .name == $now' "$root-renamed.json" \
    || check "keys: $name doesn't name KX987-$n: $(tail -c 300 "$root-renamed.json")"
done
(cd "$a" && cozy --json key) > "$root-shown.json" 2>&1 || true
J --arg was "$ka" '.key == "KX987" and .former == [$was]' "$root-shown.json" || check "keys: cozy key: not KX987, formerly $ka: $(tail -c 300 "$root-shown.json")"
[ "$(python3 -c 'import sqlite3, sys; print(",".join(r[0] for r in sqlite3.connect(sys.argv[1]).execute("SELECT key FROM project_keys ORDER BY ord")))' "$a/project.db" 2>&1)" = "$ka,KX987" ] \
  || check "keys: its project.db doesn't keep $ka then KX987"
# Another's key, a reserved one, a misspelled one and one nobody has: refused, typed.
(cd "$b" && cozy --json key "$ka") > "$root-taken.json" 2>&1 || true
J '.error.code == "project_key_taken"' "$root-taken.json" || check "keys: $kb took $ka, KX987's former key: $(tail -c 300 "$root-taken.json")"
(cd "$b" && cozy --json key run) > "$root-reserved.json" 2>&1 || true
J '.error.code == "project_key_taken"' "$root-reserved.json" || check "keys: $kb took RUN: $(tail -c 300 "$root-reserved.json")"
(cd "$b" && cozy --json key 9z) > "$root-invalid.json" 2>&1 || true
J '.error.code == "project_key_invalid"' "$root-invalid.json" || check "keys: 9z wasn't refused project_key_invalid: $(tail -c 300 "$root-invalid.json")"
(cd "$outside" && cozy --json run watch QQ987-1 --once) > "$root-none.json" 2>&1 || true
J '.error.code == "run_not_found" and (.error.message | test("QQ987"))' "$root-none.json" || check "keys: QQ987-1, a key nobody has: $(tail -c 300 "$root-none.json")"
# A bare number outside every project's folder names no run (cl-1028): the refusal says whose numbers they are and how to name
# the run (cl-1122; it said "There's no such run.", and the 0.26.0 gate's loop-cancel canceled nothing).
(cd "$outside" && cozy --json run cancel "$n") > "$root-bare.json" 2>&1 || true
J --arg n "$n" '.error.code == "run_not_found" and (.error.message | test("project.*KEY-" + $n))' "$root-bare.json" \
  || check "keys: run cancel $n outside a project: $(tail -c 300 "$root-bare.json")"
}

run_playback() {
step "run playback API"
# The player link hands a browser the output straight from its machine: a plaintext project's too (cl-1066), its link naming the run
# and the output; web/play-link plays it. Before, a project's was refused project_play_unavailable. The output is a weightless still.
local holder=$HOME/.cozy/calcifer/calcifer.lock.holder base id project
base="http://$(jq -r .address "$holder")"
(cd "$here" && cell play-still ./project/draw.py/draw "instructions=play $RANDOM") || true
first=$(cat "$OUT/play-still.number")
name=$(jq -r '.output[0].name // empty' "$OUT/play-still.json") || true
if [ "$first" = none ] || [ -z "$name" ]; then check "run playback API: no still to play"; else
  read -r id project < <(cozy --json run watch "$first" --once | jq -r '"\(.id) \(.project_id)"')
  curl -sS -G -H "Authorization: Bearer $(jq -r .token "$holder")" \
    --data-urlencode "output=$name" --data-urlencode "expires=30s" "$base/v1/projects/$project/runs/$id/play" > "$OUT/play.json" || true
  J --arg id "$id" --arg name "$name" '(.url | startswith("https://cozy-creator.github.io/cozy-creator/play/#")) and (.url | contains("r=" + $id))
    and (.url | contains("o=" + $name)) and (.token | length > 0) and .expires_s == 30' "$OUT/play.json" \
    || check "run playback API: project run $first's output has no link: $(head -c 300 "$OUT/play.json")"
fi
}

machines() {
step machines
# The same wheels again, an update to what the machine runs: it reports their versions. They're named from inside a plaintext project,
# which a command of no project sends as any file (cl-1066, fix don't refuse); before, `project_source_scope` refused them.
if [ -n "$CAND_RT" ] && [ -n "$CAND_TFS" ]; then
  wheels=$RUN_PROJECT/cl-1066-wheels
  mkdir -p "$wheels" && cp "$CAND_RT" "$CAND_TFS" "$wheels/"
  cozy --json machine update local --runtime "$wheels/$(basename "$CAND_RT")" --tensorfs "$wheels/$(basename "$CAND_TFS")" -y \
    > "$OUT/update-again.json" 2> "$OUT/update-again.err" || check "machine update local --runtime --tensorfs, wheels in a plaintext project, failed: $(tail -c 300 "$OUT/update-again.err") $(tail -c 300 "$OUT/update-again.json")"
  rm -rf "$wheels"
  rt=$(jq -r 'select(.machine) | .runtime' "$OUT/update-again.json" | tail -1 || true)
  tfs=$(jq -r 'select(.machine) | .tensorfs' "$OUT/update-again.json" | tail -1 || true)
  cozy --json machine show local | J --arg rt "$rt" --arg tfs "$tfs" '$rt != "" and .status.runtime == $rt and .status.tensorfs == $tfs' \
    || check "machine update --runtime --tensorfs reported Runtime '$rt', TensorFS '$tfs': the machine reports other versions"
fi
cozy --json machine ls > "$OUT/machines.json" && J '.machines | any(.name == "local")' "$OUT/machines.json" || check "machine ls: no local"
cozy --json machine show local > "$OUT/local.json" && J '.state == "ready" and (.address | length > 0)' "$OUT/local.json" || check "machine show local: not ready"

step "this computer's player listens on loopback alone (xs-196)"
# Until another computer's controller is authorized, only a browser on this computer plays from its machine: the WebRTC listener
# binds loopback, as the API does. Its port is the one the machine keeps across restarts.
port=$(cat "$HOME/.cozy/machine/var/lib/cozy/machine/webrtc-port" 2> /dev/null) || true
[ -n "$port" ] || check "this computer's machine keeps no player port"
if [ "$RUNNER_OS" = Linux ] && [ -n "$port" ]; then
  ss -Hltn "sport = :$port" | awk '{print $4}' > "$OUT/webrtc-listen.txt"
  [ -s "$OUT/webrtc-listen.txt" ] && ! grep -qv '^127\.0\.0\.1:' "$OUT/webrtc-listen.txt" \
    || check "the player's port $port listens on $(paste -sd' ' "$OUT/webrtc-listen.txt"), not loopback alone"
fi

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
! cozy --json machine ls | J '.machines | any(.name == "e2e-self")' || check "machine add: this computer's own machine is listed again as e2e-self"
J '.controller_key | test("^[A-Za-z0-9_-]{43}$")' "$OUT/local.json" || check "machine show local: no controller key: $(head -c 300 "$OUT/local.json")"
code=0; cozy --json machine ssh local > "$OUT/ssh-local.json" 2> "$OUT/ssh-local.err" || code=$?
[ "$code" = 2 ] && J '.error.code == "machine_ssh_none"' "$OUT/ssh-local.json" || check "machine ssh local: exit $code, not refused as no rental: $(head -c 300 "$OUT/ssh-local.json")"
# One base64url key in 64 begins with `-`, which cozy and grep once read as an option (runs 41245693908, 41266489312):
# this cell always takes such a key.
for _ in $(seq 1000); do
  openssl genpkey -algorithm ed25519 -out "$OUT/controller.pem" 2> /dev/null || check "openssl could not make an Ed25519 key"
  key=$(openssl pkey -in "$OUT/controller.pem" -pubout -outform DER | tail -c 32 | base64 | tr '+/' '-_' | tr -d '=\n') || true
  [[ $key == -* ]] && break
done
[[ $key == -* ]] || check "no Ed25519 key beginning with - in 1000"
cozy --json machine authorize "$key" > "$OUT/authorize.json" && J '.changed == true' "$OUT/authorize.json" || check "machine authorize: the key was not added"
grep -qx -- "$key" "$HOME/.cozy/keys/authorized" || check "machine authorize: the key is not in keys/authorized"
# Authorized, it listens on every interface and says where other computers add it.
J '.machine.reach | test(":[0-9]+$")' "$OUT/authorize.json" || check "machine authorize: no address to add it at: $(jq -c .machine.reach "$OUT/authorize.json")"
cozy --json machine authorize "$key" | J '.changed == false' || check "machine authorize twice: added it again"
cozy --json machine authorize "$key" --remove > "$OUT/unauthorize.json" && J '.changed == true' "$OUT/unauthorize.json" || check "machine authorize --remove: the key was not removed"
! grep -qx -- "$key" "$HOME/.cozy/keys/authorized" || check "machine authorize --remove: the key is still in keys/authorized"
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
# As pwsh reads a path: $OUT is this shell's form, /d/a/… on Windows.
ps1=$OUT/completion.ps1
[ "$RUNNER_OS" != Windows ] || ps1=$(cygpath -m "$ps1")
bash -c ". '$OUT/completion.bash'; complete -p cozy" > /dev/null || check "completion bash: does not complete cozy"
[ ! -s "$OUT/completion.zsh" ] || zsh -c "autoload -U compinit; compinit -u; . '$OUT/completion.zsh'; (( \$+functions[_cozy] ))" \
  || check "completion zsh: no _cozy"
[ ! -s "$OUT/completion.fish" ] || fish -c "source '$OUT/completion.fish'; complete -C 'cozy mach'" | grep -q machine \
  || check "completion fish: cozy mach does not complete to machine"
[ ! -s "$OUT/completion.powershell" ] || PATH="$(dirname "$COZY"):$PATH" pwsh -NoProfile -c ". '$ps1'; (TabExpansion2 'cozy mach' 9).CompletionMatches.CompletionText" | grep -q machine \
  || check "completion powershell: cozy mach does not complete to machine"
echo "completion: ${shells[*]}"
}

# The newest release's version (GitHub's newest non-prerelease vX.Y.Z), as `cozy update` with no version finds it.
newest_release() {
  curl -fsSL ${GH_TOKEN:+-H "Authorization: Bearer $GH_TOKEN"} "https://api.github.com/repos/cozy-creator/cozy-creator/releases?per_page=30" \
    | jq -er '[.[] | select(.prerelease | not) | .tag_name | select(test("^v[0-9]+\\.[0-9]+\\.[0-9]+$"))][0] // empty | ltrimstr("v")'
}

# run.sh's last step before removal (after the steps that stop Calcifer): this computer's cozy (the candidate), having run, goes
# back to the newest release as a person's `cozy update <version>` does. Calcifer's store is one way (store.rs VERSION): the binary is
# the release's, and its Calcifer refuses to start on the newer store, saying so. Putting the candidate back (a dev build's file
# swapped in, `cozy doctor` restarting Calcifer on it, as `candidate_cozy` does) recovers: the project's runs and outputs are as
# they were. (An older release's own update is its code, frozen: run 40084450876 found 0.11.14's update waiting on Calcifer for
# good while an authorized machine served.)
update_to_newest() {
local newest back=$OUT/cozy-under-test candidate said before after
cp "$COZY" "$back"
candidate=$("$back" --version)
unset COZY_DOWNLOAD_URL  # GitHub's newest release, not --release's files
newest=$(newest_release) || true
[ -n "$newest" ] || { check "update: no release to update to"; return; }
run_history() { (cd "$RUN_PROJECT" && cozy --json run ls --limit 0) | jq -cS '[.runs[] | {id, number, status, target, output: [.output[]? | {name, index, sha256}]}] | sort_by(.number)'; }
before=$(run_history) || { check "update: the candidate lists no run history"; return; }
t0=$SECONDS
if cozy --json update "$newest" > "$OUT/update.json"; then
  # A release that reads this store takes it as before.
  J '.calcifer == "restarted"' "$OUT/update.json" || check "update: Calcifer was not restarted ($(jq -c .calcifer "$OUT/update.json"))"
  fixture_calcifer | J --arg v "$newest" '.calcifer.version == $v' || check "update: Calcifer does not run $newest"
  echo "update $newest: $((SECONDS - t0)) s; it reads this store"
else
  said=$(grep '^{' "$OUT/update.json" | jq -r 'select(.error) | [.error.message, .error.remedy // "", (.error.next // [] | join(" "))] | join(" ")' 2> /dev/null) || said=
  [[ $said =~ "Calcifer's store is at version "[0-9]+ && $said == *"cozy update"* ]] \
    || check "update $newest: not refused for Calcifer's newer store: $(tail -c 300 "$OUT/update.json")"
  echo "update $newest: $((SECONDS - t0)) s; its Calcifer refused the newer store: $said"
fi
[ "$(cozy --version)" = "cozy $newest" ] || check "update: cozy is $(cozy --version), not $newest"
CAND_COZY=$back install_candidate_binary
cozy --json doctor > "$OUT/update-forward.json" || check "update: cozy doctor on the candidate again: $(tail -c 300 "$OUT/update-forward.json")"
[ "$(cozy --version)" = "$candidate" ] || check "update: cozy is $(cozy --version), not the candidate $candidate"
grep -q "cozy ${candidate#cozy }," "$OUT/update-forward.json" || check "update: Calcifer is not the candidate ${candidate#cozy } again"
after=$(run_history) || { check "update: the candidate lists no run history again"; return; }
[ "$after" = "$before" ] || check "update: the project's runs are not as they were: $(diff <(jq . <<< "$before") <(jq . <<< "$after") | head -20)"
echo "update: back on ${candidate#cozy }, the project's $(jq length <<< "$after") runs and their outputs as they were"
}

# The agent API (cl-956, tracker design/agent-api.md): agent.js drives `cozy get`, `apply`, `diff`, `delete` and `explain` as an agent
# does, with the page's edits (e2e/page.py) between, and renders shots.py's Shot on local.
agent_api() {
[ -n "$LOCAL" ] || return 0
step "the agent API: nodes listed and read whole, files applied, a conflict with the page's edit settled, undone from the page; explain (cl-956)"
local json=$OUT/agent.json code=0 folder
HOLDER=$(web_path "$HOME/.cozy/calcifer/calcifer.lock.holder") COZY=$(web_path "$COZY") ARGV_LOG=$(web_path "$OUT/argv.log") \
  node "$(web_path "$here/agent.js")" > "$json" 2> "$OUT/agent.err" || code=$?
[ "$code" = 0 ] || check "agent: failed $(jq -c .failures "$json" 2> /dev/null): $(jq -c '(.failures // []) as $f | .seen | with_entries(select(.key as $k | $f | index($k)))' "$json" 2> /dev/null | head -c 2000) $(tail -c 500 "$OUT/agent.err")"
for folder in $(jq -r '.seen.project.folder // empty, .seen.pier // empty' "$json" 2> /dev/null || true); do drop_folder "$folder"; done
}

# cl-944 (Paul, 11-12: "the runtime will just ignore that input modality that the model cannot use"), past Calcifer's binder: an API
# caller's raw prompt, sent as `cozy run … field:=<json>`. A picture, a video and a sound in a prompt of pictures: the picture kept, the
# video and the sound left out, each its name in the text and one `ignored` row; a picture where only words go: its name, one row. Nothing
# is refused, and `cozy run watch` says each once.
ignored_inputs() {
[ -n "$LOCAL" ] || return 0
step "a raw prompt's inputs its function can't take: left out as names, one ignored row each, said once (cl-944)"
local raw=$here/project/raw.py dir=$OUT/ignored file pic clip hum prompt n lines
mkdir -p "$dir"
ffmpeg -v error -y -f lavfi -i color=c=red:size=64x48 -frames:v 1 "$dir/pic.png" \
  && ffmpeg -v error -y -f lavfi -i testsrc2=size=64x48:rate=24:duration=1 -c:v libx264 -pix_fmt yuv420p "$dir/clip.mp4" \
  && ffmpeg -v error -y -f lavfi -i sine=frequency=440:duration=1 "$dir/hum.wav" || { check "ignored: ffmpeg made no files"; return; }
for file in pic.png clip.mp4 hum.wav; do
  [ -n "$(row "$RUN_PROJECT" "ignored_${file%.*}" .name)" ] || project_import "$RUN_PROJECT" "$dir/$file" "ignored_${file%.*}" > /dev/null || check "ignored: import $file"
done
for _ in $(seq 100); do [ "$(row "$RUN_PROJECT" ignored_hum '.sha256 // empty')" ] && [ "$(row "$RUN_PROJECT" ignored_clip '.sha256 // empty')" ] && break; sleep 0.2; done
pic=$(row "$RUN_PROJECT" ignored_pic '.sha256 // empty' | jq -r .) clip=$(row "$RUN_PROJECT" ignored_clip '.sha256 // empty' | jq -r .) hum=$(row "$RUN_PROJECT" ignored_hum '.sha256 // empty' | jq -r .)
[ -n "$pic" ] && [ -n "$clip" ] && [ -n "$hum" ] || { check "ignored: the imports hold no files: $pic $clip $hum"; return; }
pic=sha256:${pic#sha256:} clip=sha256:${clip#sha256:} hum=sha256:${hum#sha256:}
prompt=$(jq -cn --arg a "$pic" --arg v "$clip" --arg h "$hum" '{text: "<A> by <V> with <H>", references: [{key: "A", type: "image", file: $a}, {key: "V", type: "video", file: $v}, {key: "H", type: "audio", file: $h}]}')
if cell ignored-pictures "$raw/pictures" "instructions:=$prompt"; then
  J '.result.kept == ["A"] and .result.text == "<A> by V with H"' "$OUT/ignored-pictures.json" \
    || check "ignored: a prompt of pictures didn't keep the picture and name the video and sound: $(jq -c .result "$OUT/ignored-pictures.json")"
  J '[.adjustments[]? | select(.kind == "ignored")] as $i | ($i | length) == 2
     and any($i[]; .requested == "V" and .field == "instructions.references.1" and (.reason | test("takes no videos")))
     and any($i[]; .requested == "H" and (.reason | test("takes no audio")))' "$OUT/ignored-pictures.json" \
    || check "ignored: not one ignored row each for the video and the sound: $(jq -c '[.adjustments[]? | select(.kind == "ignored")]' "$OUT/ignored-pictures.json")"
  n=$(cat "$OUT/ignored-pictures.number")
  lines=$( (cd "$RUN_PROJECT" && cozy run watch "$n") 2>&1 | grep -cE '^\s*ignored instructions\.references\.[12]: [VH] \(' || true)
  [ "$lines" = 2 ] || check "ignored: cozy run watch $n says the left-out inputs $lines times, not once each"
fi
prompt=$(jq -cn --arg a "$pic" '{text: "make <A> red", references: [{key: "A", type: "image", file: $a}]}')
if cell ignored-words "$raw/words" "prompt:=$prompt"; then
  J '.result.text == "make A red"' "$OUT/ignored-words.json" || check "ignored: words didn't take the picture's name: $(jq -c .result "$OUT/ignored-words.json")"
  J '[.adjustments[]? | select(.kind == "ignored")] | length == 1 and .[0].requested == "A" and (.[0].reason | test("takes no images"))' "$OUT/ignored-words.json" \
    || check "ignored: a picture where only words go isn't one ignored row: $(jq -c '[.adjustments[]? | select(.kind == "ignored")]' "$OUT/ignored-words.json")"
fi
}

# xs-180: what a function says of its file rides its run's result, and an orbit's every frame is a keyframe (ffprobe, Calcifer's own).
orbit_metadata() {
[ -n "$LOCAL" ] || return 0
step "an orbit's take: its metadata in cozy run's result, every frame a keyframe (xs-180)"
local probe file keys
cell orbit "$here/project/orbit.py/turn" frames:=24 || return 0
J '.result.video.metadata.orbit == {"degrees": 360, "direction": "cw", "loops": true}' "$OUT/orbit.json" || check "orbit: cozy run's result says no orbit: $(jq -c .result "$OUT/orbit.json" | head -c 300)"
probe=$(find "$HOME/.cozy/tools/ffmpeg" -name 'ffprobe*' 2> /dev/null | head -1 || true)
probe=${probe:-$(command -v ffprobe || true)} file=$(output orbit mp4 || true)
keys=$("$probe" -v error -select_streams v:0 -show_entries frame=key_frame -of csv=p=0 "$file" | cut -d, -f1 | sort | uniq -c | tr -s ' ' || true)
[ "$keys" = " 24 1" ] || check "orbit: not every frame of $file is a keyframe (count key_frame): ${keys:-no frames}"
}

# cl-840's one retry rule, on an install a person waits on, against a Hub this part stands up (Calcifer on it, then back): a Hub that
# takes the request and never answers is asked again by the backoff, each attempt a line, and given up within a minute, saying hub_slow;
# a 503 is asked again by the backoff, then named in the Hub's own words; a 404 is the Hub's refusal, failed at once, asked once.
install_retries() {
step "a package install against a Hub that's silent, down or says it has none: asked again, said, or failed at once (cl-840)"
local config=$HOME/.cozy/config.toml dir=$OUT/install-retries hub pid
mkdir -p "$dir"
python3 -I - "$dir" > "$dir/hub.log" 2>&1 <<'PY' &
import http.server, json, pathlib, sys, threading
dir = pathlib.Path(sys.argv[1])
class Hub(http.server.BaseHTTPRequestHandler):
    def answer(self, status, body):
        data = json.dumps(body).encode()
        self.send_response(status)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(data)))
        self.end_headers()
        self.wfile.write(data)
    def do_GET(self):
        mode = (dir / "mode").read_text().strip() if (dir / "mode").exists() else ""
        with open(dir / "asked", "a") as asked:
            asked.write(f"{mode} {self.command} {self.path}\n")
        if mode == "silent" and self.path.split("?")[0] == "/v1/packages/e2e/e2e-boxed":
            threading.Event().wait(600)
            return
        if mode == "down":
            return self.answer(503, {"error": {"kind": "unavailable", "code": "down", "message": "this Hub is down"}})
        self.answer(404, {"error": {"kind": "not_found", "code": "absent", "message": f"no {self.path} here"}})
    do_POST = do_GET
    def log_message(self, *args):
        pass
server = http.server.ThreadingHTTPServer(("127.0.0.1", 0), Hub)
(dir / "addr").write_text(f"127.0.0.1:{server.server_address[1]}")
server.serve_forever()
PY
pid=$!
for _ in $(seq 50); do [ -s "$dir/addr" ] && break; sleep 0.1; done
[ -s "$dir/addr" ] || { check "install retries: the stand-in Hub didn't start: $(tail -c 300 "$dir/hub.log")"; kill "$pid"; return; }
hub=http://$(cat "$dir/addr")
cp "$config" "$dir/config.toml"
# Top-level: after a [table] it would be that table's.
{ echo "hub_url = \"$hub\""; grep -v '^hub_url' "$dir/config.toml" || true; } > "$config"
cozy --json restart > "$dir/restart.json" 2>&1 || check "install retries: cozy restart onto the stand-in Hub: $(tail -c 300 "$dir/restart.json")"
install_retry silent e2e/e2e-boxed
grep -q hub_slow "$dir/silent.said" || check "install retries: a silent Hub isn't said hub_slow: $(tail -c 400 "$dir/silent.said")"
[ "$(grep -oE 'attempt [0-9]/5 in [0-9]' "$dir/silent.said" | cut -c9 | paste -sd' ')" = '2 3 4 5' ] && [ "$(grep -c '^silent ' "$dir/asked")" = 5 ] \
  && [ "$(cat "$dir/silent.took")" -lt 60 ] \
  || check "install retries: a silent Hub isn't asked again by the backoff, each attempt once, then given up within a minute ($(cat "$dir/silent.took") s, asked $(grep -c '^silent ' "$dir/asked") times): $(tail -c 400 "$dir/silent.said")"
install_retry down e2e/e2e-boxed
grep -qE 'attempt 5/5 in [0-9]' "$dir/down.said" && grep -q 'this Hub is down' "$dir/down.said" && [ "$(cat "$dir/down.took")" -lt 100 ] \
  || check "install retries: a 503 isn't asked again by the backoff, then named ($(cat "$dir/down.took") s): $(tail -c 400 "$dir/down.said")"
install_retry absent e2e/never-published
! grep -q attempt "$dir/absent.said" && [ "$(cat "$dir/absent.took")" -lt 20 ] \
  && [ "$(grep '^absent ' "$dir/asked" | grep never-published | sort | uniq -d | wc -l)" = 0 ] \
  || check "install retries: a 404 isn't failed at once, asked once ($(cat "$dir/absent.took") s): $(tail -c 300 "$dir/absent.said") $(grep '^absent ' "$dir/asked" | head -5 | tr '\n' ' ')"
cp "$dir/config.toml" "$config"
cozy --json restart > "$dir/restart-back.json" 2>&1 || check "install retries: cozy restart back onto the suite's Hub: $(tail -c 300 "$dir/restart-back.json")"
kill "$pid" 2> /dev/null || true
}
# install_retry <mode> <package>: `cozy package install` with the stand-in Hub in that mode: what it said, and its seconds.
install_retry() {
  local dir=$OUT/install-retries began=$SECONDS
  echo "$1" > "$dir/mode"
  if COZY_HUB_URL=http://$(cat "$dir/addr") cozy --json package install "$2" --on local > "$dir/$1.said" 2>&1; then echo "it installed" >> "$dir/$1.said"; fi
  echo $((SECONDS - began)) > "$dir/$1.took"
}


# cl-1028: every run is kept in its project. Outside one, `cozy run` and `POST /v1/runs` refuse before anything is done: one line
# and the next command, and nothing of the run is made.
outside_refuses() {
step "a run outside a project refuses before anything is done, naming the next command (cl-1028)"
local out=$OUT/outside lock=$HOME/.cozy/calcifer/calcifer.lock.holder began code=0 addr token made
mkdir -p "$out/folder"
began=$(date +%s%3N)
(cd "$out/folder" && cozy run "$here/weightless/reverse" text=outside) > "$out/run.txt" 2>&1 || code=$?
[ "$code" = 2 ] && grep -qxF "error[not_in_a_project]: Run this from inside a project folder, or create a project first." "$out/run.txt" \
  && [ "$(grep -cxF 'next: cozy new' "$out/run.txt")" = 1 ] && ! grep -qE '^(hint|cause):' "$out/run.txt" \
  || check "outside: cozy run outside a project said (exit $code): $(head -c 400 "$out/run.txt")"
addr=$(jq -r .address "$lock") token=$(jq -r .token "$lock")
code=$(curl -sS -o "$out/post.json" -w '%{http_code}' -H "Authorization: Bearer $token" -H 'Content-Type: application/json' \
  -H "Idempotency-Key: outside-$RANDOM$RANDOM" -X POST -d '{"target":"zhengpeng7/birefnet/remove_background","on":["local"]}' "http://$addr/v1/runs") || code=
[ "$code" = 400 ] && J '.error.code == "not_in_a_project" and any(.error.next[]; test("cozy new"))' "$out/post.json" \
  || check "outside: POST /v1/runs with no project answered ${code:-nothing}: $(head -c 300 "$out/post.json")"
made=$(python3 - "$HOME/.cozy/calcifer/state.db" "$began" <<'PY'
import sqlite3, sys
c = sqlite3.connect(f"file:{sys.argv[1]}?mode=ro", uri=True, timeout=10)
print(c.execute("SELECT count(*) FROM runs WHERE created_at >= ? AND (target LIKE '%reverse%' OR target LIKE '%birefnet%')", (int(sys.argv[2]),)).fetchone()[0])
PY
) || made=?
[ "$made" = 0 ] || check "outside: a refused run left $made rows in Calcifer's store"
}

# cl-1028: Calcifer's jobs (this shard's installs, downloads and updates on `local`; its removals, locks and publications, on no
# machine) are no project's runs: no run list shows one, GET /v1/runs lists nothing, their machine lists them, and Calcifer's store
# keeps of each machine, job type and outcome its newest ended job and those that ended in the minute before it: an older one goes as a
# newer one's end comes, whatever order they ended in. Beside the shard's, each outside any project: a package install that fails on
# `local` (its code builds no module, as run_failures'), then four models made from one file at once there (ending in any order), and
# a package installed and removed twice. The failed install stays, its reason on its machine, though downloads and installs completed
# after it (the review of #3423: one outcome or job type never takes another's).
jobs_kept() {
step "jobs are their machine's, not run history: its last of each job type and outcome (cl-1028)"
local out=$OUT/jobs lock=$HOME/.cozy/calcifer/calcifer.lock.holder code=0 addr token i pids=()
mkdir -p "$out"
[ -s "$OUT/tiny.safetensors" ] || safetensors_file "$OUT/tiny.safetensors"
rm -rf "$out/nomodule" && cp -r "$here/weightless" "$out/nomodule"
sed -i -e 's/^module-name = "e2e_weightless"/module-name = "e2e_no_such_module"/' -e 's/^name = "e2e-weightless"/name = "e2e-jobs-no-module"/' "$out/nomodule/pyproject.toml"
(cd "$out" && cozy --json package install "$out/nomodule" --on local -y) > "$out/failed.ndjson" 2>&1 || code=$?
[ "$code" != 0 ] || check "jobs: a package install whose code builds no module completed: $(tail -c 300 "$out/failed.ndjson")"
for i in 1 2 3 4; do
  (cd "$out" && cozy --json model install "$OUT/tiny.safetensors" "local/e2e-jobs-$i" --source-format as-is) > "$out/model-$i.ndjson" 2>&1 & pids+=($!)
done
for i in 1 2 3 4; do wait "${pids[i-1]}" || check "jobs: model $i of four made at once failed: $(tail -c 300 "$out/model-$i.ndjson")"; done
for i in 1 2; do
  (cd "$out" && cozy --json package install "$here/weightless" --on local -y) > "$out/install-$i.ndjson" 2>&1 \
    || check "jobs: package install $i outside a project failed: $(tail -c 300 "$out/install-$i.ndjson")"
  (cd "$out" && cozy --json package uninstall local/e2e-weightless --on local) > "$out/uninstall-$i.json" 2>&1 \
    || check "jobs: package uninstall $i outside a project failed: $(tail -c 300 "$out/uninstall-$i.json")"
done
cozy --json run ls --all --limit 0 > "$out/all.json" 2>&1 || true
J '.runs != null and ([.runs[] | select(.project_id == null)] | length == 0)' "$out/all.json" \
  || check "jobs: run ls --all lists runs of no project: $(jq -c '[.runs[]? | select(.project_id == null) | {id, kind, target}][:5]' "$out/all.json" 2> /dev/null | head -c 300)"
addr=$(jq -r .address "$lock") token=$(jq -r .token "$lock")
code=$(curl -sS -o "$out/list.json" -w '%{http_code}' -H "Authorization: Bearer $token" "http://$addr/v1/runs?limit=5") || code=
[ "$code" = 400 ] && J '.error.code == "not_in_a_project"' "$out/list.json" || check "jobs: GET /v1/runs answered ${code:-nothing}: $(head -c 300 "$out/list.json")"
cozy --json machine show local > "$out/local.json" 2>&1 || true
J '(.jobs // []) | length > 0 and all(.[]; .name == "" and .project_id == null)' "$out/local.json" \
  || check "jobs: machine show local lists no jobs, or names them as runs: $(jq -c '[.jobs[]? | {name, kind, target, status}]' "$out/local.json" 2> /dev/null | head -c 300)"
J 'any(.jobs[]?; .status == "failed" and (.target | contains("e2e-jobs-no-module")) and ((.reason.message // "") != ""))' "$out/local.json" \
  || check "jobs: the failed package install isn't on local with its reason after later ones completed: $(jq -c '[.jobs[]? | {kind, target, status, reason: .reason.message}]' "$out/local.json" 2> /dev/null | head -c 500)"
echo "jobs: on local $(jq -c '[.jobs[]? | {kind, status, target}]' "$out/local.json" 2> /dev/null | head -c 600)"
python3 - "$HOME/.cozy/calcifer/state.db" > "$out/kept.json" <<'PY'
import json, sqlite3, sys
c = sqlite3.connect(f"file:{sys.argv[1]}?mode=ro", uri=True, timeout=10)
# By job type (a store before it groups by kind). Each group: how many ended, and those kept that ended over a minute before its newest.
task = "task" if any(col[1] == "task" for col in c.execute("PRAGMA table_info(runs)")) else "kind"
groups = {}
for machine, kind, status, number, ended in c.execute(f"SELECT machine, {task}, status, number, settled_at FROM runs WHERE state = 'settled'"):
    groups.setdefault((machine, kind, status), []).append((number, ended or 0))
kept = []
for (machine, kind, status), rows in sorted(groups.items(), key=str):
    rows.sort()
    kept.append([machine, kind, status, len(rows), [number for number, ended in rows[:-1] if ended < rows[-1][1] - 65_000]])
print(json.dumps({"made": c.execute("SELECT max(number) FROM runs").fetchone()[0], "groups": kept}))
PY
echo "jobs: $(cat "$out/kept.json") (machine, job type, status, ended, kept over a minute behind its newest)"
J '.made >= 12 and any(.groups[]; .[0] == "local" and .[1] == "install" and .[2] == "failed") and any(.groups[]; .[0] == "local" and .[1] == "download" and .[2] == "completed")
   and any(.groups[]; .[0] == "local" and .[1] == "install" and .[2] == "completed") and any(.groups[]; .[0] == null and .[1] == "remove" and .[2] == "completed")' "$out/kept.json" \
  || check "jobs: no failed install beside completed downloads and installs on local and Calcifer's own removals, by job type, to prove what is kept: $(cat "$out/kept.json")"
J 'all(.groups[]; .[4] == [])' "$out/kept.json" \
  || check "jobs: Calcifer's store keeps an ended job over a minute behind the newest of its machine, job type and outcome (machine, job type, status, ended, those): $(cat "$out/kept.json")"
}
