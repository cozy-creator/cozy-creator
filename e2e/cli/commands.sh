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
! COZY_HUB_URL=http://127.0.0.1:9 cozy --json package ls birefnet --limit 1 > "$OUT/hub-env.json" 2>&1 \
  || check "COZY_HUB_URL=http://127.0.0.1:9: package ls did not go there"
# A run made at that other Hub (local code reads none), then the variable unset: Calcifer still lists and shows it.
if [ -n "$LOCAL" ] && (cd "$here" && COZY_HUB_URL=http://127.0.0.1:9 cell other-hub ./weightless/reverse "text=hub$RANDOM"); then
  n=$(cat "$OUT/other-hub.number")
  cozy --json --full run ls --limit 0 | J --argjson n "$n" 'any(.runs[]; .number == $n and .hub == "http://127.0.0.1:9")' \
    || check "run ls: run $n, made at another Hub, is not listed"
  cozy --json run show "$n" --once | J '.hub == "http://127.0.0.1:9" and .status == "completed"' || check "run show $n: not its record at the other Hub"
fi

step "signed out"
cozy --json whoami > "$OUT/whoami.json" && J '.logged_in == false' "$OUT/whoami.json" || check "whoami: not signed out"
cozy --json logout > "$OUT/logout.json" || check "logout while signed out failed"

step packages
cozy --json package ls birefnet --limit 1 > "$OUT/search.json" && J '(.packages | length) == 1 and .packages[0].package == "birefnet"' "$OUT/search.json" \
  || check "package ls --limit 1: not exactly birefnet"
cozy --json package info birefnet > "$OUT/package-info.json" && J '(.latest_release | length > 0) and (.releases | length > 0)' "$OUT/package-info.json" \
  || check "package info birefnet: no releases"
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
cozy --json model info zhengpeng7/birefnet > "$OUT/model-info.json" && J '.releases[0].lanes | length > 0' "$OUT/model-info.json" || check "model info zhengpeng7/birefnet: no lanes"
lane=$(jq -r '.releases[0].lanes[0].lane' "$OUT/model-info.json") || true

step "renting, signed out"
# The catalog's shape, never its stock: an empty market or a sold-out SKU is not a code failure.
cozy machine rent > "$OUT/rent-catalog.txt" 2>&1 && grep -q -E '^NAME +GPU +GPUS +COMPUTE +VRAM +\$/HOUR' "$OUT/rent-catalog.txt" \
  || check "machine rent: no catalog table: $(head -c 300 "$OUT/rent-catalog.txt")"
cozy --json machine rent > "$OUT/rent-catalog.json" 2> "$OUT/rent-catalog.err" && J 'type == "array" and all(.[]; .name | length > 0)' "$OUT/rent-catalog.json" \
  || check "machine rent --json: no catalog array: $(head -c 300 "$OUT/rent-catalog.json") $(head -c 200 "$OUT/rent-catalog.err")"
code=0; cozy --json run -p "$RUN_PROJECT" birefnet/remove_background --file "image=$(picture dog1.jpg)" --on new > "$OUT/rent-signed-out.json" 2>&1 || code=$?
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
cozy --json package remove local/e2e-weightless --on local > "$OUT/remove.ndjson" || check "package remove local/e2e-weightless --on local failed"
cozy --json package ls --on local > "$OUT/packages-after.json" && ! grep -q 'e2e-weightless' "$OUT/packages-after.json" \
  || check "package remove: local/e2e-weightless is still installed"

step "models on local"
cozy --json model download zhengpeng7/birefnet --lane "$lane" --on local > "$OUT/download.ndjson" || check "model download --lane $lane failed"
cozy --json model download zhengpeng7/birefnet local/e2e-birefnet > "$OUT/download-alias.ndjson" || check "model download zhengpeng7/birefnet local/e2e-birefnet failed"
# A local .safetensors file, made into a model the machine keeps by name: the file is written to the machine
# and made from its object (object://sha256:…/<name>). No format knows its one key `w`, so it is kept as-is.
safetensors_file "$OUT/tiny.safetensors"
cozy --json model download "$OUT/tiny.safetensors" local/e2e-tiny --source-format as-is > "$OUT/download-file.ndjson" || check "model download ./tiny.safetensors local/e2e-tiny failed: $(grep -oE '"message": *"[^"]*"' "$OUT/download-file.ndjson" | tail -1)"
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
refused run run -p "$RUN_PROJECT" birefnet/remove_background --file "image=$dog"
refused run-on-local run -p "$RUN_PROJECT" birefnet/remove_background --input "$OUT/refused.yaml" --on local
refused run-warm run birefnet/remove_background --warm installed
(cd "$here" && refused run-code run -p "$RUN_PROJECT" ./weightless/reverse text=refused)
# Describing local code reads its source here and uses no machine: it works where none runs.
(cd "$here" && cozy --json run ./weightless/reverse --describe > "$OUT/describe-code.json" 2> "$OUT/describe-code.err") \
  && J '.fields | any(.name == "text")' "$OUT/describe-code.json" \
  || check "cozy run ./weightless/reverse --describe: $(tail -c 300 "$OUT/describe-code.json") $(tail -c 200 "$OUT/describe-code.err")"
refused install package install birefnet
refused install-code package install "$here/weightless"
refused remove package remove birefnet
refused download model download zhengpeng7/birefnet
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
cozy status > "$OUT/status.txt" 2>&1 && grep -q "local runs aren't available on" "$OUT/status.txt" || check "status does not say local runs aren't available"
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
[ -n "$(row "$RUN_PROJECT" bound_clip .name)" ] || cozy project -p "$RUN_PROJECT" import "$dir/clip.mp4" bound_clip > /dev/null || check "bound: import the clip"
for _ in $(seq 60); do [ "$(row "$RUN_PROJECT" bound_clip .duration_ms)" != null ] && break; sleep 0.5; done
# Frames 48 up to 144 at 24 fps: 2 to 6 s.
if (cd "$dir" && cell bound "$here/inspect/inspect" 'clip=<bound_clip:48-144>' --file still=./still.png folder=./folder label=bound); then
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
if (cd "$dir" && cell bound-digest "$here/inspect/inspect" --file "clip=./clip.mp4" --file "still=sha256:$hex" folder=./folder label=digest); then
  grep -qE "\"still_bytes\": *$size" "$OUT/bound-digest.ndjson" || check "bound-digest: the run was not given the uploaded picture"
fi
before=$(cozy --json run ls --limit 0 | jq '.runs | length')
(cd "$dir" && ALLOW=file_not_held cell bound-unheld "$here/inspect/inspect" --file "clip=./clip.mp4" --file "still=sha256:$(head -c 64 /dev/urandom | sha256sum | cut -c1-64)" folder=./folder) || true
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
if (cd "$dir" && cell own-refs "$here/project/mentions.py/mentions" 'instructions=<Amy> waves at <Amy>' 'references:=[{"name": "Amy", "image": "./amy.png"}]'); then
  grep -qE '"keys": *\[ *"Amy" *\]' "$OUT/own-refs.ndjson" && grep -qE '"widths": *\[ *48 *\]' "$OUT/own-refs.ndjson" \
    || check "own references: <Amy> did not send its picture: $(grep -oE '"(text|keys|widths)": *("[^"]*"|\[[^]]*\])' "$OUT/own-refs.ndjson" | head -3 | paste -sd' ')"
fi
(cd "$dir" && cozy --json run -p "$RUN_PROJECT" "$here/project/mentions.py/mentions" 'instructions=<Nobody> waves' --on local) > "$OUT/own-refs-nothing.json" 2>&1 || code=$?
[ "$code" != 0 ] && grep -q input_missing "$OUT/own-refs-nothing.json" && grep -q 'cozy project import <file> Nobody' "$OUT/own-refs-nothing.json" \
  || check "own references: <Nobody> was not refused with the command that makes it a box (exit $code): $(tail -c 300 "$OUT/own-refs-nothing.json")"
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
  cozy --json project -p "$RUN_PROJECT" import "$dir/$name.mp4" "hostile_$name" > "$OUT/hostile-$name.json" 2>&1 || true
done
for _ in $(seq 60); do [[ $(row "$RUN_PROJECT" hostile_control .duration_ms) =~ ^[0-9]+$ ]] && break; sleep 0.5; done
sleep 5
[[ $(row "$RUN_PROJECT" hostile_control .duration_ms) =~ ^[0-9]+$ ]] || check "hostile media: the control clip was never probed: $(tail -c 200 "$OUT/hostile-control.json")"
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
rm -rf "$src" "$seen"; mkdir -p "$src/folder/sub" "$src/sub" "$seen"
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
size=$(stat -c %s "$src/still.png")
[ ! -e /work ] || { check "unseen: /work is on this computer too"; return; }
docker pull -q "$image" > /dev/null || { check "unseen: docker pull $image"; return; }
# inside <name> <cozy words…>: the CLI in /work (or $DIR in it), a copy of $src that only its container has. $AFTER, a shell
# line, then runs there, its words on stderr. Its stdout is unseen-<name>.out, its stderr unseen-<name>.err. $REFUSED: its
# failure is the caller's to judge.
inside() {
  local name=$1; shift
  printf '%s\x1f' "$@" >> "$OUT/argv.log"; printf '\x1e' >> "$OUT/argv.log"  # docker runs the binary itself
  cp "$HOME/.cozy/calcifer/calcifer.lock.holder" "$HOME/.cozy/config.toml" "$seen/"
  docker run --rm --network host --pid host -u "$(id -u):$(id -g)" -e HOME=/home/cli -e COZY_HUB_URL="${COZY_HUB_URL:-}" \
    -e COZY_PROJECT="${UNSEEN_PROJECT:-}" -e AFTER="${AFTER:-}" -e DIR="${DIR:-.}" \
    --tmpfs "/home/cli:rw,exec,uid=$(id -u),gid=$(id -g),mode=0700" --tmpfs "/work:rw,exec,uid=$(id -u),gid=$(id -g),mode=0700" \
    -v "$seen:/seen:ro" -v "$COZY:/usr/local/bin/cozy:ro" -v "$src:/src:ro" -v /etc/ssl/certs:/etc/ssl/certs:ro \
    ${CAND_RT:+-v "$(dirname "$CAND_RT"):/wheels:ro"} -w /work "$image" \
    sh -c 'mkdir -p ~/.cozy/calcifer && cp /seen/calcifer.lock.holder ~/.cozy/calcifer/ && cp /seen/config.toml ~/.cozy/ \
      && cp -r /src/. /work && cd "$DIR" && cozy "$@" && { [ -z "$AFTER" ] || sh -c "$AFTER" >&2; }' sh "$@" \
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
# A project made from in there, by its name, then named by COZY_PROJECT: no path of either side's names it.
if inside new --json project new unseen-film; then
  UNSEEN_PROJECT=$(jq -r .name "$OUT/unseen-new.out")
  J '.path | startswith("/work") | not' "$OUT/unseen-new.out" || check "unseen new: made in the container's folder: $(cat "$OUT/unseen-new.out")"
fi
export UNSEEN_PROJECT
if inside files --json run ./inspect/inspect --file clip=./clip.mp4 --file still=./still.png folder=./folder label=files --on local; then
  said files '"clip_seconds": *(2\.9|3\.0|3\.1)' && said files "\"still_bytes\": *$size" && said files '"folder_files": *\[ *"a.txt", *"sub/b.txt" *\]'
  said files '"target": *"local/e2e-inspect/inspect"'
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
  AFTER='sha256sum again/*' inside show run show "$n" -o again && grep -q "^$hex  again/" "$OUT/unseen-show.err" \
    || check "unseen show: run show $n -o again: $(tail -c 300 "$OUT/unseen-show.err")"
fi
inside install --json package install ./weightless --on local && J '.run.status == "completed"' <(tail -1 "$OUT/unseen-install.out") \
  || check "unseen install: $(tail -c 300 "$OUT/unseen-install.out")"
# Its recipes read from Calcifer's copy, then refused only at the model's header, which a sign-in reads: from a path it couldn't
# see, the package was refused first (`target_not_found`).
refused_inside quantize not_logged_in model quantize zhengpeng7/birefnet --lane int8 --package ./weightless --on local
AFTER='grep -c "name = \"e2e-weightless\"" uv.lock' DIR=weightless inside lock --json package lock && grep -qx 1 "$OUT/unseen-lock.err" \
  || check "unseen lock: no uv.lock written in /work: $(tail -c 300 "$OUT/unseen-lock.err")"
# Locked in Calcifer's copy of the tree, which it holds by the tree's digest.
ls -d "$HOME"/.cozy/calcifer/code/*/tree/e2e_weightless > /dev/null 2>&1 || check "unseen lock: Calcifer holds no copy of the package: $(ls "$HOME/.cozy/calcifer/code" 2> /dev/null | head -3)"
DIR=weightless refused_inside publish not_logged_in package publish
inside import project import ./still.png && grep -qx still "$OUT/unseen-import.out" || check "unseen import: not named still: $(cat "$OUT/unseen-import.out")"
inside into project import ./other.png --into still && grep -qx still.2 "$OUT/unseen-into.out" || check "unseen into: not still.2: $(cat "$OUT/unseen-into.out")"
[ "$(row "${UNSEEN_PROJECT:-none}" still .versions)" = 2 ] || check "unseen: still has not 2 versions: $(row "${UNSEEN_PROJECT:-none}" still .)"
inside model --json model download ./tiny.safetensors local/e2e-unseen --source-format as-is --on local \
  && cozy --json model ls --on local | grep -q e2e-unseen || check "unseen model: local/e2e-unseen is not on local"
if [ -f "${CAND_RT:-}" ]; then
  inside runtime --json machine update local --runtime "/wheels/$(basename "$CAND_RT")" -y || true
fi
# Local code is a project's or goes by content: a path to your tool list is refused, as no client's folder is Calcifer's.
local lock=$HOME/.cozy/calcifer/calcifer.lock.holder
[ "$(curl -sS -o "$OUT/unseen-list.json" -w '%{http_code}' -H "Authorization: Bearer $(jq -r .token "$lock")" -H 'Content-Type: application/json' \
  -d '{"add": "/work/weightless"}' "http://$(jq -r .address "$lock")/v1/tools/list")" = 400 ] && J '.error.code == "tool_list_local"' "$OUT/unseen-list.json" \
  || check "unseen: a path added to the tool list: $(cat "$OUT/unseen-list.json")"
unset UNSEEN_PROJECT
}

# cl-820: local code edited between runs on `local`, as cl-817's edit.sh timed it (an edit took 7.9 s read in place, 10.2 s once it
# went by content with an environment per tree). The environment is keyed on what it is built from (lock, pyproject, path
# dependencies), never the code: an edit to the code alone runs on the same one, an edit back runs the first code again, and a new
# dependency builds another. Each step's `cozy run` wall time, the reply and the environment TensorD ran it in.
local_edits() {
step "local code edited between runs"
[ -n "$LOCAL" ] || return 0
local code=$OUT/edits/e2e-edits envs=$HOME/.cozy/machine/var/lib/cozy/tensord/editable
local module=$code/e2e_weightless/__init__.py first now
rm -rf "$code" && mkdir -p "$OUT/edits" && cp -r "$here/weightless" "$code"
sed -i 's/^name = "e2e-weightless"/name = "e2e-edits"/' "$code/pyproject.toml"
: > "$OUT/edits.txt"
timed() {  # timed <step> <reply>: the environment its run had, on stdout
  local t0 t1 env
  t0=$(date +%s.%N)
  cozy --json run -p "$RUN_PROJECT" "$code/reverse" text=abc --on local > "$OUT/edit-$1.ndjson" 2> "$OUT/edit-$1.err" \
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

# cl-816: the machine quotes local code from its own runs of it, by the installation it holds, not by a manifest object a run
# ended with. Quoted after its install, before any run, it stands on nothing; after one run, on that run (`measured`), its run
# time that run's, nothing to prepare. The second run is timed beside the quote.
local_quote() {
step "local code is quoted from its runs"
[ -n "$LOCAL" ] || return 0
local code=$OUT/quoted/e2e-quoted first quoted
rm -rf "$code" && mkdir -p "$OUT/quoted" && cp -r "$here/weightless" "$code"
sed -i 's/^name = "e2e-weightless"/name = "e2e-quoted"/' "$code/pyproject.toml"
estimate() {  # estimate <name>: the local machine's quote of one run, in <name>.json
  cozy --json run -p "$RUN_PROJECT" "$code/reverse" text=abc --estimate --on local > "$OUT/$1-all.json" 2> "$OUT/$1.err" \
    || { check "$1: cozy run --estimate failed: $(tail -c 300 "$OUT/$1.err")"; return 1; }
  jq '.machines[] | select(.machine == "local")' "$OUT/$1-all.json" > "$OUT/$1.json"
  jq -c '{basis, width, start_ms, finish_ms, prepare_ms}' "$OUT/$1.json" | sed "s/^/$1: /" | tee -a "$OUT/quoted.txt"
}
: > "$OUT/quoted.txt"
estimate quote-unrun || return 0
jq -e '.basis != "measured"' "$OUT/quote-unrun.json" > /dev/null || check "local quote: measured before any run of it"
cell quote-first "$code/reverse" text=abc || return 0
estimate quote-run || return 0
first=$(run_get "$OUT" quote-first .execution_ms)
quoted=$(jq '.finish_ms - .start_ms' "$OUT/quote-run.json")
echo "first run ${first:-?} ms; quoted ${quoted} ms" | tee -a "$OUT/quoted.txt"
J '.basis == "measured"' "$OUT/quote-run.json" || check "local quote: after a run, not measured: $(jq -c . "$OUT/quote-run.json")"
J '.prepare_ms == 0' "$OUT/quote-run.json" || check "local quote: its installation not found, $(jq .prepare_ms "$OUT/quote-run.json") ms to prepare"
[ -n "$first" ] && jq -en --argjson q "$quoted" --argjson f "$first" '($q - $f | fabs) <= ([0.2 * $f, 100] | max)' > /dev/null \
  || check "local quote: its run time ${quoted} ms, the run's ${first:-?} ms"
cell quote-second "$code/reverse" text=abc && echo "second run $(run_get "$OUT" quote-second .execution_ms) ms" | tee -a "$OUT/quoted.txt"
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

# After a BiRefNet run: the warm levels find it installed.
run_flags() {
step "run flags"
before=$(cozy --json run ls --limit 0 | jq '.runs | length') || true
(cd "$here" && cozy --json run ./weightless/reverse --describe) > "$OUT/describe.json" || check "run --describe failed"
J '.fields | any(.name == "text")' "$OUT/describe.json" || check "run --describe: no text field"
[ "$(cozy --json run ls --limit 0 | jq '.runs | length')" = "$before" ] || check "run --describe started a run"
note=$OUT/inputs/note.txt; mkdir -p "$OUT/inputs"; echo "an asset" > "$note"
if (cd "$here" && cell fidelity ./weightless/fidelities --file "notes=$note" --file-fidelity notes=high); then
  grep -q 'notes=high' "$OUT/fidelity.ndjson" || check "--file-fidelity notes=high: the executor saw $(grep -o '"assets":\[[^]]*\]' "$OUT/fidelity.ndjson")"
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

# Calcifer's time left, on a weightless run whose stages and steps time it as a model's do (tool.py's `stepped`, its first step slow as a
# warm-up is). Its first run here says `first_run`, with no number, until it has two steps of its own. With history, each report has a
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
    jq -se 'map(select((.completed // 0) < 2)) | length > 0 and all(.eta_s == null and .eta_basis == "first_run")' "$out.progress" > /dev/null \
      || check "time left: a first run had a number before two steps of its own: $(head -c 600 "$out.progress")"
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
(cd "$here" && cozy --json run -p "$RUN_PROJECT" ./weightless/wait seconds:=120 --on local > "$OUT/wait.ndjson" 2>&1) &
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

step "run ls filters"
cozy --json run ls --state canceled > "$OUT/canceled.json" && J --argjson n "${n:-0}" '(.runs | length > 0) and all(.runs[]; .status == "canceled") and any(.runs[]; .number == $n)' "$OUT/canceled.json" \
  || check "run ls --state canceled: wrong runs"
}

# After `basics`: its model downloads are runs of zhengpeng7/birefnet.
run_filters() {
step "run ls filters"
cozy --json run ls --package zhengpeng7/birefnet > "$OUT/by-package.json" && J '(.runs | length > 0) and all(.runs[]; .target | startswith("zhengpeng7/birefnet"))' "$OUT/by-package.json" \
  || check "run ls --package zhengpeng7/birefnet: none listed, or others listed"
cozy --json run ls --on local > "$OUT/by-machine.json" && J '(.runs | length > 0) and all(.runs[]; .machine == "local")' "$OUT/by-machine.json" \
  || check "run ls --on local: other machines listed"
cozy --json run ls --limit 1 > "$OUT/limit.json" && J '(.runs | length) == 1' "$OUT/limit.json" || check "run ls --limit 1: not one run"
}

run_play() {
step "run play"
# The link opens the browser player (GitHub Pages) with this machine, the output and a token in its fragment; the
# token's expiry is --expires from now. The output: a weightless still (the project cell's renderer).
(cd "$here" && cell play-still ./project/draw.py/draw "instructions=play $RANDOM") || true
first=$(cat "$OUT/play-still.number")
name=$(jq -r '.output[0].name // empty' "$OUT/play-still.json") || true
expiry() {  # the token's expiry (unix s), from the link's fragment (unpadded base64url JSON before its `.`)
  local t; t=$(sed -n 's/.*[#&]c=\([^.&]*\).*/\1/p' <<< "$1" | tr '_-' '/+')
  while (( ${#t} % 4 )); do t+==; done
  base64 -d <<< "$t" 2> /dev/null | jq -r '.e // empty' 2> /dev/null || true
}
if [ "$first" = none ] || [ -z "$name" ]; then check "run play: no still to play"; else
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
cozy --json status | J --arg v "${newest#v}" '.calcifer.version == $v' || check "update: Calcifer does not run ${newest#v}"
}
