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

step "renting, signed out"
# The catalog's shape, never its stock: an empty market or a sold-out SKU is not a code failure.
cozy machine rent > "$OUT/rent-catalog.txt" 2>&1 && grep -q -E '^NAME +GPU +GPUS +COMPUTE +VRAM +\$/HOUR' "$OUT/rent-catalog.txt" \
  || check "machine rent: no catalog table: $(head -c 300 "$OUT/rent-catalog.txt")"
cozy --json machine rent > "$OUT/rent-catalog.json" 2> "$OUT/rent-catalog.err" && J 'type == "array" and all(.[]; .name | length > 0)' "$OUT/rent-catalog.json" \
  || check "machine rent --json: no catalog array: $(head -c 300 "$OUT/rent-catalog.json") $(head -c 200 "$OUT/rent-catalog.err")"
code=0; in_project "$RUN_PROJECT" --json run birefnet/remove_background --file "image=$(picture dog1.jpg)" --on new > "$OUT/rent-signed-out.json" 2>&1 || code=$?
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
  [ "$code" = 1 ] && J '.error.code == "local_unavailable" and (.error.remedy | contains("--on new"))' "$OUT/refused-$name.json" \
    || check "cozy $*: exit $code, not refused local_unavailable: $(head -c 300 "$OUT/refused-$name.json") $(head -c 200 "$OUT/refused-$name.err")"
}
refused run run birefnet/remove_background --file "image=$dog"
refused run-on-local run birefnet/remove_background --input "$OUT/refused.yaml" --on local
(refused run-code run "$here/weightless/reverse" text=refused)
# Describing local code reads its source here and uses no machine: it works where none runs.
(in_project "$RUN_PROJECT" --json run "$here/weightless/reverse" --describe > "$OUT/describe-code.json" 2> "$OUT/describe-code.err") \
  && J '.fields | any(.name == "text")' "$OUT/describe-code.json" \
  || check "cozy run ./weightless/reverse --describe: $(tail -c 300 "$OUT/describe-code.json") $(tail -c 200 "$OUT/describe-code.err")"
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
grep -q "local runs aren't available on" "$OUT/refused.txt" && grep -q -- '--on new' "$OUT/refused.txt" \
  || check "the refusal does not say local runs aren't available and --on new: $(head -c 300 "$OUT/refused.txt")"

step "no local machine"
cozy --json machine ls > "$OUT/machines.json" && J '.machines | all(.name != "local")' "$OUT/machines.json" || check "machine ls: a local machine is listed"
cozy machine ls > "$OUT/machines.txt" 2>&1 && grep -q "local runs aren't available on" "$OUT/machines.txt" || check "machine ls does not say local runs aren't available"
fixture_calcifer > "$OUT/status.txt" && J '.calcifer.local_usable == false' "$OUT/status.txt" || check "status does not say local runs aren't available"
cozy --json doctor > "$OUT/doctor-gated.json" && J '.healthy and (.checks | any(.check == "machine" and (.state | contains("aren'"'"'t available"))))' "$OUT/doctor-gated.json" \
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
{ [ "$code" != 0 ] && grep -q mention_not_orbit "$OUT/angles-plain.json" && grep -q 'turn on Orbit' "$OUT/angles-plain.json" \
  && [ "$(cozy --json run ls --limit 0 | jq '.runs | length')" = "$before" ]; } \
  || check "angles: an angle of a video that isn't an orbit was not refused mention_not_orbit before any run (exit $code): $(tail -c 300 "$OUT/angles-plain.json")"
}

# cl-854: a lock on Runtime 0.43 can't speak a 0.23 machine's executor protocol. One whose range takes the machine's
# Runtime runs on it, its install saying to re-lock; one whose range refuses it installs nothing, refused in one line
# naming the re-lock. Both locks are PyPI's 0.43.1 and TensorFS 0.22.1.
old_runtime() {
step "a lock on an older executor protocol: the machine's pair where its range takes it, else one line naming the re-lock"
local code=0
if cell old-runtime "$here/old-runtime/takes/echo" text=old; then
  grep -q "can't speak this machine's executor protocol.*uv lock --upgrade-package cozy-runtime" "$OUT/old-runtime.err" \
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
ls -d "$HOME"/.cozy/calcifer/code/*/tree/e2e_weightless > /dev/null 2>&1 || check "unseen lock: Calcifer holds no copy of the package: $(ls "$HOME/.cozy/calcifer/code" 2> /dev/null | head -3)"
DIR=weightless refused_inside publish not_logged_in package publish
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
local code=$OUT/edits/e2e-edits envs=$HOME/.cozy/machine/var/lib/cozy/tensord/editable
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
package_at "$dir" e2e_local_relay '"cozy-runtime>=0.45", "msgspec>=0.19", "minimax-h3-tools"' \
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
callee=$(find "$HOME/.cozy/machine" -name generation.json -path '*generations*' -exec jq -r \
  'select(.package == "e2e-relay") | .callees[]? | select(.distribution | ascii_downcase | gsub("[-_.]+"; "-") == "minimax-h3-tools") | .package' \
  {} + 2> /dev/null | sort -u | paste -sd, -)
echo "hub published: $E2E_RELAY's environment names its callee ${callee:-nothing}"
[ "$callee" = _/minimax-h3-tools ] || check "hub published: $E2E_RELAY's environment names its callee ${callee:-nothing}, not _/minimax-h3-tools"
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
package_at "$dir/maker" e2e_latents_maker '"cozy-runtime>=0.45", "msgspec>=0.19"' <<'PY'
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
package_at "$dir/caller" e2e_latents_caller '"cozy-runtime>=0.45", "msgspec>=0.19", "e2e-latents-maker"' \
  "$(printf '\n[tool.uv.sources]\ne2e-latents-maker = { path = "../maker" }')" <<'PY'
import hashlib
from importlib.metadata import version

import e2e_latents_maker
import msgspec

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
    shot = await call(e2e_latents_maker.shot, payload=e2e_latents_maker.Ask(text=payload.text))
    data = b"" if shot.latents is None else shot.latents.read_bytes()
    return Got(
        latents=type(shot.latents).__name__,
        clip=type(shot.clip).__name__,
        same_bytes=f"sha256:{hashlib.sha256(data).hexdigest()}" == shot.sha256,
        runtime=version("cozy-runtime"),
    )


app.entrypoint(relay, accelerator=False)
PY
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

# After `basics`: its model installs are runs of zhengpeng7/birefnet.
run_filters() {
step "run ls filters"
cozy --json run ls --all --package zhengpeng7/birefnet > "$OUT/by-package.json" && J '(.runs | length > 0) and all(.runs[]; .target | startswith("zhengpeng7/birefnet"))' "$OUT/by-package.json" \
  || check "run ls --package zhengpeng7/birefnet: none listed, or others listed"
cozy --json run ls --on local > "$OUT/by-machine.json" && J '(.runs | length > 0) and all(.runs[]; .machine == "local")' "$OUT/by-machine.json" \
  || check "run ls --on local: other machines listed"
cozy --json run ls --limit 1 > "$OUT/limit.json" && J '(.runs | length) == 1' "$OUT/limit.json" || check "run ls --limit 1: not one run"
}

run_playback() {
step "run playback API"
# The link opens the browser player (GitHub Pages) with this machine, the output and a token in its fragment; the
# token's expiry comes from the API query. The output is a weightless still.
local holder=$HOME/.cozy/calcifer/calcifer.lock.holder base id
base="http://$(jq -r .address "$holder")"
(cd "$here" && cell play-still ./project/draw.py/draw "instructions=play $RANDOM") || true
first=$(cat "$OUT/play-still.number")
name=$(jq -r '.output[0].name // empty' "$OUT/play-still.json") || true
expiry() {  # the token's expiry (unix s), from the link's fragment (unpadded base64url JSON before its `.`)
  local t; t=$(sed -n 's/.*[#&]c=\([^.&]*\).*/\1/p' <<< "$1" | tr '_-' '/+')
  while (( ${#t} % 4 )); do t+==; done
  base64 -d <<< "$t" 2> /dev/null | jq -r '.e // empty' 2> /dev/null || true
}
if [ "$first" = none ] || [ -z "$name" ]; then check "run playback API: no still to play"; else
  id=$(cozy --json run watch "$first" --once | jq -r .id)
  for expires in 30s 2h; do
    url=$(curl --fail-with-body -sS -G -H "Authorization: Bearer $(jq -r .token "$holder")" \
      --data-urlencode "output=$name" --data-urlencode "expires=$expires" "$base/v1/runs/$id/play" \
      | tee "$OUT/play-$expires.json" | jq -r '.url // empty') || true
    curl -fsS -o /dev/null "${url%%#*}" || check "run playback API: its player page ${url%%#*} does not answer"
    [[ $url == *"o=$name"* && $url == *"a="* ]] || check "run playback API: the link names no machine or not output $name: $url"
    want=$(( $(date +%s) + ${expires%[sh]} * $([ "${expires: -1}" = h ] && echo 3600 || echo 1) )) e=$(expiry "$url")
    [ -n "$e" ] && [ $(( e - want )) -le 10 ] && [ $(( want - e )) -le 10 ] || check "run playback API expires $expires: the token expires at ${e:-?}, not about $want"
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
cozy --json machine ls > "$OUT/machines.json" && J '.machines | any(.name == "local")' "$OUT/machines.json" || check "machine ls: no local"
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
fixture_calcifer | J --arg v "${newest#v}" '.calcifer.version == $v' || check "update: Calcifer does not run ${newest#v}"
}
