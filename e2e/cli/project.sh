# e2e/cli/project.sh: a video project on this computer through `cozy project` and Calcifer's HTTP surface (cl-716),
# the part cli/run.sh calls `projects`, with the local machine up. No model runs: the shots are attached plates and the
# other nodes a weightless local renderer (project/draw.py), so every path but a model's is real: the folder, the
# assets, the lock, the walk, the cut, the API a web UI uses and a browser on it. Fixtures are made here with ffmpeg
# (the job's, on this shell's PATH; Calcifer fetches its own).
# `version_skew` is called by run.sh's start, between the installer's cozy and the candidates: the candidate CLI
# against the release's running Calcifer.
here=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)

# The candidate CLI from its own path against an older Calcifer that lacks the route: a typed calcifer_outdated with
# its remedy, never a bare 404.
version_skew() {
  [ -f "$CAND_COZY" ] || return 0
  step "version skew: the candidate CLI against the release's Calcifer"
  local code=0
  "$CAND_COZY" project ls > "$OUT/skew.txt" 2>&1 || code=$?
  cat "$OUT/skew.txt"
  { [ "$code" != 0 ] && grep -q calcifer_outdated "$OUT/skew.txt" && ! grep -qiE 'untyped_answer|answered 404' "$OUT/skew.txt"; } \
    || check "skew: the candidate CLI against $(cozy --version)'s Calcifer did not say calcifer_outdated (exit $code)"
}

# plate <file> <lavfi source> <tone Hz> <video kb/s>: 6 s at 854x480, 24 fps, a keyframe a second, constant bitrate;
# AAC 192k stereo at 32 kHz.
plate() {
  ffmpeg -v error -y -f lavfi -i "$2=size=854x480:rate=24:duration=6" -f lavfi -i "sine=frequency=$3:sample_rate=32000:duration=6" \
    -c:v libx264 -preset veryfast -b:v "$4k" -minrate "$4k" -maxrate "$4k" -bufsize "$4k" -x264-params nal-hrd=cbr -g 24 \
    -pix_fmt yuv420p -c:a aac -b:a 192k -ac 2 -ar 32000 -shortest "$1"
}
still() { ffmpeg -v error -y -f lavfi -i "color=c=$2:size=832x480" -frames:v 1 "$1"; }
sha() { sha256sum "$1" | cut -c1-64; }
probe() {
  ffprobe -v error -show_entries "stream=codec_type,bit_rate,sample_rate,channels" -of json "$1" \
    | jq -c '{video: ([.streams[] | select(.codec_type == "video") | .bit_rate | tonumber] | first),
              audio: ([.streams[] | select(.codec_type == "audio") | {bit_rate: (.bit_rate | tonumber), sample_rate, channels}] | first)}'
}
# api <curl args…>: Calcifer, with the bearer of its lock record.
api() { curl -sS -H "Authorization: Bearer $TOKEN" "$@"; }

projects() {
step "a project: new, walk-up, -p"
local P=$OUT/garden E=$OUT/elsewhere CACHE=$HOME/.cache/cozy/cuts code
cozy project new "$P" > "$OUT/project-new.txt" 2>&1 || check "project new failed: $(tail -c 300 "$OUT/project-new.txt")"
[ "$(ls -A "$P" 2>/dev/null)" = project.yaml ] || check "project new made $(ls -A "$P" 2>/dev/null | tr '\n' ' '), not only project.yaml"
# The new project's film parses as it stands: its nodes are all listed, nothing refused.
cozy project -p "$P" status > "$OUT/project-new-status.txt" 2>&1 || check "status of a new project failed: $(tail -c 300 "$OUT/project-new-status.txt")"
for node in ash pikachu meadow meadow.wide rustle friends; do grep -q "$node" "$OUT/project-new-status.txt" || check "a new project's status names no $node"; done
mkdir -p "$P/plates" "$E"
plate "$P/plates/a.mp4" testsrc2 440 1800; plate "$P/plates/b.mp4" testsrc 550 900; plate "$P/plates/c.mp4" rgbtestsrc 660 1200
plate "$E/look.mp4" smptebars 330 600; still "$E/stage.png" teal
cp "$here/project/draw.py" "$P/draw.py"
cat > "$P/project.yaml" <<YAML
entities:
  - {name: place, kind: environment, description: "a plain test card", render: {with: ./draw.py/draw}}
  - {name: stage, kind: environment, description: "a teal stage", image: $E/stage.png,
     setups: [{name: a, camera: "wide", render: {with: ./draw.py/draw}}]}
shots:
  - {name: a, prompt: "test pattern a", duration_s: 6, video: plates/a.mp4, references: [{name: look, kind: video, video: $E/look.mp4}]}
  - {name: b, prompt: "test pattern b", duration_s: 6, video: plates/b.mp4}
edit: [{shot: a, start: 12, end: 132}, {shot: b, start: 12, end: 132}]
YAML
local A B C
A=$(sha "$P/plates/a.mp4") B=$(sha "$P/plates/b.mp4") C=$(sha "$P/plates/c.mp4")
# From a subfolder: the project is found walking up. render all makes place and stage.a here and cuts the edit.
code=0; (cd "$P/plates" && cozy project render all --on local) > "$OUT/project-render-1.txt" 2>&1 || code=$?
{ [ "$code" = 0 ] && [ -s "$P/edit.mp4" ]; } || check "project render all from plates/: exit $code, edit.mp4 $([ -s "$P/edit.mp4" ] && echo made || echo missing): $(tail -c 400 "$OUT/project-render-1.txt")"
# -p from elsewhere.
(cd / && cozy --json project -p "$P" status) > "$OUT/project-status.json" 2> "$OUT/project-status.err" || check "project -p status from /: $(tail -c 300 "$OUT/project-status.err")"
local ID; ID=$(jq -r '.id // empty' "$OUT/project-status.json" 2>/dev/null) || ID=
[ -n "$ID" ] || check "project status --json names no id: $(head -c 300 "$OUT/project-status.json")"
cozy project -p "$P" status > "$OUT/project-status.txt" 2>&1 || check "project status (text) failed"

step "a project: inputs taken into assets/, by symlink on ext4"
[ "$(readlink "$P/assets/$A.mp4")" = "$P/plates/a.mp4" ] || check "assets/$A.mp4 is not a symlink to plates/a.mp4: $(ls -l "$P/assets/" | head -5)"
J '.inputs["plates/a.mp4"].method == "symlink"' "$P/project.lock" || check "project.lock: plates/a.mp4 not taken by symlink: $(jq -c '.inputs["plates/a.mp4"]' "$P/project.lock")"
J --arg a "assets/$A.mp4" '.nodes[] | select(.name == "a") | .path == $a and .assets.video.original == "plates/a.mp4"' "$OUT/project-status.json" \
  || check "status: shot a is not assets/$A.mp4 from plates/a.mp4: $(jq -c '.nodes[]? | select(.name == "a")' "$OUT/project-status.json")"

step "a project: --approve with -n 2 refused"
code=0; cozy project -p "$P" render a -n 2 --approve --on local > "$OUT/project-approve-n.txt" 2>&1 || code=$?
{ [ "$code" != 0 ] && grep -q -- '--approve' "$OUT/project-approve-n.txt" && J '(.pending // {}) | length == 0' "$P/project.lock"; } \
  || check "render a -n 2 --approve was not refused before any run (exit $code): $(tail -c 300 "$OUT/project-approve-n.txt")"

step "a project: the edit's bitrates and sound, matched to its takes"
local pa pb pe
pa=$(probe "$P/plates/a.mp4") pb=$(probe "$P/plates/b.mp4") pe=$(probe "$P/edit.mp4")
echo "a $pa; b $pb; edit $pe"
# 5 s of each take, copied but for the edges each encoded at its own take's bitrate: the edit is near their mean.
jq -en --argjson a "$pa" --argjson b "$pb" --argjson e "$pe" '(($a.video + $b.video) / 2) as $m | $e.video > 0.8 * $m and $e.video < 1.2 * $m
  and $e.audio.bit_rate > 0.95 * ([$a.audio.bit_rate, $b.audio.bit_rate] | max) and $e.audio.bit_rate < 1.05 * ([$a.audio.bit_rate, $b.audio.bit_rate] | max)
  and $e.audio.sample_rate == "32000" and $e.audio.channels == 2' > /dev/null \
  || check "edit.mp4's bitrates are not its takes' (edit $pe; a $pa; b $pb)"

step "a project: edge pieces in cozy's cache, reused, keyed by take"
pieces() { (cd "$CACHE" 2>/dev/null && ls -1 | grep -E "^(${A:0:16}|${B:0:16}|${C:0:16})-" | while read -r f; do echo "$f $(stat -c %s "$f")"; done) | sort || true; }
pieces > "$OUT/cuts-1.txt"; cat "$OUT/cuts-1.txt"
[ -s "$OUT/cuts-1.txt" ] || check "no edge piece of a or b in $CACHE: $(ls "$CACHE" 2>&1 | head -5)"
local hidden; hidden=$(cd "$P" && find . -mindepth 1 -name '.*' | head -5)
[ -z "$hidden" ] || check "the project holds hidden files: $hidden"
cozy project -p "$P" render all --on local > "$OUT/project-render-2.txt" 2>&1 || check "render all again failed: $(tail -c 300 "$OUT/project-render-2.txt")"
pieces > "$OUT/cuts-2.txt"; cmp -s "$OUT/cuts-1.txt" "$OUT/cuts-2.txt" || check "the second cut made edge pieces again: $(diff "$OUT/cuts-1.txt" "$OUT/cuts-2.txt" | head -5)"
sed -i 's#video: plates/b.mp4#video: plates/c.mp4#' "$P/project.yaml"
cozy project -p "$P" render all --on local > "$OUT/project-render-3.txt" 2>&1 || check "render all with b's plate swapped failed: $(tail -c 300 "$OUT/project-render-3.txt")"
pieces > "$OUT/cuts-3.txt"; cat "$OUT/cuts-3.txt"
{ grep -q "^${C:0:16}-" "$OUT/cuts-3.txt" && [ "$(grep "^${A:0:16}-" "$OUT/cuts-1.txt")" = "$(grep "^${A:0:16}-" "$OUT/cuts-3.txt")" ]; } \
  || check "b swapped for c: c's edge pieces not made, or a's not reused: $(diff "$OUT/cuts-1.txt" "$OUT/cuts-3.txt" | head -6)"

step "a project: versions made by local code, numbered, approved only in the lock"
cozy project -p "$P" render place -n 2 --on local > "$OUT/project-place.txt" 2>&1 || check "render place -n 2 failed: $(tail -c 300 "$OUT/project-place.txt")"
jq -c '.nodes.place | {approved, versions: [.versions[] | {number, assets}]}' "$P/project.lock" || true
local real=1 v
for v in $(jq -r '.nodes.place.versions[].assets.image.path' "$P/project.lock" 2>/dev/null); do { [ -f "$P/$v" ] && [ ! -L "$P/$v" ]; } || real=''; done
{ [ -n "$real" ] && J '.nodes.place.versions | length == 3 and all(.assets.image.path | test("^assets/[0-9a-f]{64}\\.png$")) and ([.[].number] == [1, 2, 3])' "$P/project.lock"; } \
  || check "place's versions are not 1-3 as real assets/<sha256>.png: $(jq -c '[.nodes.place.versions[]? | {number, path: .assets.image.path}]' "$P/project.lock")"
ls -la "$P/assets" > "$OUT/assets-1.txt"
cozy project -p "$P" approve place 2 > "$OUT/project-approve.txt" 2>&1 || check "approve place 2 failed: $(tail -c 300 "$OUT/project-approve.txt")"
ls -la "$P/assets" > "$OUT/assets-2.txt"
{ J '.nodes.place.approved == .nodes.place.versions[1].assets.image.path' "$P/project.lock" && cmp -s "$OUT/assets-1.txt" "$OUT/assets-2.txt"; } \
  || check "approving place 2 changed a file, or the lock does not approve version 2: $(jq -c .nodes.place.approved "$P/project.lock")"

step "a project: a reference changed, then missing"
still "$E/stage.png" orange
cozy project -p "$P" status > "$OUT/project-changed.txt" 2>&1 || true
{ grep -q 'reference changed: .*stage.png' "$OUT/project-changed.txt" && grep -q 'stage.a .*out of date: stage changed' "$OUT/project-changed.txt"; } \
  || check "an edited reference: status does not say it changed and stage.a is out of date: $(tail -c 400 "$OUT/project-changed.txt")"
mv "$E/stage.png" "$E/stage-moved.png"
cozy project -p "$P" status > "$OUT/project-missing.txt" 2>&1 || true
code=0; cozy project -p "$P" render stage.a --on local > "$OUT/project-missing-render.txt" 2>&1 || code=$?
{ grep -q 'reference missing: .*stage.png: relink' "$OUT/project-missing.txt" && [ "$code" != 0 ] && grep -q 'reference missing' "$OUT/project-missing-render.txt"; } \
  || check "a moved reference: status does not say relink, or stage.a rendered (exit $code): $(tail -c 300 "$OUT/project-missing.txt")"
mv "$E/stage-moved.png" "$E/stage.png"

step "a project: a node renamed"
local approved; approved=$(jq -r .nodes.place.approved "$P/project.lock")
sed -i 's/{name: place, kind: environment/{name: yard, kind: environment/' "$P/project.yaml"
cozy project -p "$P" status > "$OUT/project-orphan.txt" 2>&1 || true
grep -q 'cozy project rename place yard' "$OUT/project-orphan.txt" || check "status does not suggest the rename: $(tail -c 300 "$OUT/project-orphan.txt")"
cozy project -p "$P" rename place yard > "$OUT/project-rename.txt" 2>&1 || check "rename place yard failed: $(tail -c 300 "$OUT/project-rename.txt")"
cozy --json project -p "$P" status > "$OUT/project-renamed.json" 2>/dev/null || true
{ J '((.orphans // []) | length == 0) and (.nodes[] | select(.name == "yard") | .versions == 3 and .approved == 2)' "$OUT/project-renamed.json" \
  && [ "$(jq -r .nodes.yard.approved "$P/project.lock")" = "$approved" ]; } \
  || check "after the rename yard is not place's 3 versions with 2 approved: $(jq -c '{orphans, yard: (.nodes[]? | select(.name == "yard"))}' "$OUT/project-renamed.json")"

reflinked
frames_otio
http_surface "$P" "$ID"
browser_session "$P" "$ID"

step "a project: the index, a moved folder missing, DELETE forgets"
cozy --json project ls > "$OUT/project-ls-1.json" || check "project ls failed"
J --arg p "$P" '.projects | any(.path == $p and (.missing | not))' "$OUT/project-ls-1.json" || check "project ls does not list $P: $(head -c 300 "$OUT/project-ls-1.json")"
mv "$P" "$OUT/garden-moved"
cozy --json project ls > "$OUT/project-ls-2.json" || check "project ls after the move failed"
J --arg i "$ID" '.projects | any(.id == $i and .missing)' "$OUT/project-ls-2.json" || check "a moved project is not missing: $(jq -c --arg i "$ID" '.projects[]? | select(.id == $i)' "$OUT/project-ls-2.json")"
local id2; id2=$(cozy --json project -p "$OUT/garden-moved" status 2>/dev/null | jq -r '.id // empty') || id2=
for id in "$ID" "$id2"; do
  [ -z "$id" ] || [ "$(api -o /dev/null -w '%{http_code}' -X DELETE "http://$ADDR/v1/projects/$id")" = 204 ] || check "DELETE /v1/projects/$id did not answer 204"
done
cozy --json project ls > "$OUT/project-ls-3.json" || true
{ J --arg a "$ID" --arg b "$id2" '.projects | all(.id != $a and .id != $b)' "$OUT/project-ls-3.json" && [ -f "$OUT/garden-moved/project.yaml" ]; } \
  || check "DELETE did not forget the project, or touched its folder"
}

# A reference on a filesystem with copy-on-write clones (an XFS image made here): taken in by reflink, the project's
# own; its original moved changes nothing; its original edited is taken in again, its reader out of date.
reflinked() {
step "a project: references reflinked on XFS"
local img=$OUT/xfs.img X=/mnt/cozy-xfs
command -v mkfs.xfs > /dev/null || { check "no mkfs.xfs on this runner"; return; }
truncate -s 300M "$img" && mkfs.xfs -q "$img" && sudo mkdir -p "$X" && sudo mount -o loop "$img" "$X" && sudo chown "$(id -u):$(id -g)" "$X" \
  || { check "could not mount an XFS image at $X"; return; }
local P=$X/clone
cozy project new "$P" > /dev/null 2>&1 || check "project new on XFS failed"
mkdir -p "$P/refs"; cp "$here/project/draw.py" "$P/draw.py"; still "$P/refs/stage.png" navy
cat > "$P/project.yaml" <<YAML
entities:
  - {name: stage, kind: environment, description: "a navy stage", image: refs/stage.png,
     setups: [{name: a, camera: "wide", render: {with: ./draw.py/draw}}]}
YAML
cozy project -p "$P" render stage.a --on local > "$OUT/xfs-render.txt" 2>&1 || check "render stage.a on XFS failed: $(tail -c 300 "$OUT/xfs-render.txt")"
local S; S=$(sha "$P/refs/stage.png")
{ J '.inputs["refs/stage.png"].method == "reflink"' "$P/project.lock" && [ -f "$P/assets/$S.png" ] && [ ! -L "$P/assets/$S.png" ]; } \
  || check "XFS: refs/stage.png not reflinked: $(jq -c '.inputs["refs/stage.png"]' "$P/project.lock")"
mv "$P/refs/stage.png" "$P/refs/stage-moved.png"
cozy project -p "$P" status > "$OUT/xfs-moved.txt" 2>&1 || true
{ ! grep -q 'reference missing' "$OUT/xfs-moved.txt" && grep -q 'stage.a .*current' "$OUT/xfs-moved.txt"; } \
  || check "XFS: a reflinked original moved, and status changed: $(tail -c 300 "$OUT/xfs-moved.txt")"
still "$P/refs/stage.png" maroon
cozy project -p "$P" status > "$OUT/xfs-edited.txt" 2>&1 || true
{ ! grep -q 'reference changed' "$OUT/xfs-edited.txt" && grep -q 'stage.a .*out of date: stage changed' "$OUT/xfs-edited.txt" && [ -f "$P/assets/$S.png" ]; } \
  || check "XFS: an edited original is not taken in again with stage.a out of date: $(tail -c 300 "$OUT/xfs-edited.txt")"
}

# cl-716 A2: an edit in frames, cut frame-exact. Odd cuts (a 7-151, b 13-140, a 48-96, b 150-161 at 24 fps): ffprobe's
# exact count, every frame its intended source frame (copied ones md5-equal), edge pieces named by frames and none for
# the keyframe-aligned a 48-96, edit.otio read by OpenTimelineIO with its clips and frames, the lock's probed shapes,
# status's frames. Then b's take at 25 fps: the full path, retimed and said so; then b back: the first edit again, its
# edges from the cache.
frames_otio() {
step "a project: an edit in frames, frame-exact, and its edit.otio"
local P=$OUT/frames CACHE=$HOME/.cache/cozy/cuts SA SB SR n miss=0 w
local FR=(uv run -q --no-project --with numpy python -I "$here/project/frames.py" "$(command -v ffmpeg)")
local OT=(uv run -q --no-project --with opentimelineio==0.17.0 python -I "$here/project/otio_check.py")
cozy project new "$P" > /dev/null 2>&1 || check "frames: project new failed"
mkdir -p "$P/plates"
gop() { ffmpeg -v error -y -f lavfi -i "$2=size=854x480:rate=$3:duration=8" -f lavfi -i "sine=frequency=$4:sample_rate=48000:duration=8" \
  -c:v libx264 -preset veryfast -g "$3" -keyint_min "$3" -sc_threshold 0 -pix_fmt yuv420p -c:a aac -b:a 160k -ac 2 -shortest "$1"; }
gop "$P/plates/a.mp4" testsrc2 24 440; gop "$P/plates/b.mp4" testsrc 24 550; gop "$P/plates/r.mp4" testsrc 25 550
SA=$(sha "$P/plates/a.mp4") SB=$(sha "$P/plates/b.mp4") SR=$(sha "$P/plates/r.mp4")
cat > "$P/project.yaml" <<'YAML'
fps: 24
shots:
  - {name: a, prompt: "test pattern a", duration_s: 8, video: plates/a.mp4}
  - {name: b, prompt: "test pattern b", duration_s: 8, video: plates/b.mp4}
edit: [{shot: a, start: 7, end: 151}, {shot: b, start: 13, end: 140}, {shot: a, start: 48, end: 96}, {shot: b, start: 150, end: 161}]
YAML
ls -1 "$CACHE" 2>/dev/null | sort > "$OUT/frames-cache-0.txt" || true
cozy project -p "$P" render all --on local > "$OUT/frames-render-1.txt" 2>&1 || check "frames: render all failed: $(tail -c 300 "$OUT/frames-render-1.txt")"
n=$(ffprobe -v error -count_frames -select_streams v:0 -show_entries stream=nb_read_frames,r_frame_rate -of csv=p=0 "$P/edit.mp4" 2>/dev/null) || n=
[ "$n" = "24/1,330" ] || check "frames: edit.mp4 is $n, not 330 frames at 24/1 ((151-7)+(140-13)+(96-48)+(161-150))"
grep -q "edit.mp4.*: 330 frames (" "$OUT/frames-render-1.txt" || check "frames: the cut event does not say 330 frames: $(grep -i 'edit.mp4' "$OUT/frames-render-1.txt" | tail -1)"
ls -1 "$CACHE" | sort > "$OUT/frames-cache-1.txt"
for w in "${SA:0:16}-f7-f24-" "${SA:0:16}-f144-f151-" "${SB:0:16}-f13-f24-" "${SB:0:16}-f120-f140-" "${SB:0:16}-f150-f161-"; do
  grep -q "^$w[0-9a-f]\{8\}\.ts$" "$OUT/frames-cache-1.txt" || { check "frames: no edge piece $w*"; miss=1; }
done
[ "$(grep -c "^\(${SA:0:16}\|${SB:0:16}\)-f" "$OUT/frames-cache-1.txt")" = 5 ] || check "frames: not exactly the 5 edge pieces (none for the keyframe-aligned a 48-96): $(grep "^\(${SA:0:16}\|${SB:0:16}\)-f" "$OUT/frames-cache-1.txt" | paste -sd' ')"
"${FR[@]}" "$P/edit.mp4" 24 "$P/plates/a.mp4:7:151" "$P/plates/b.mp4:13:140" "$P/plates/a.mp4:48:96" "$P/plates/b.mp4:150:161" > "$OUT/frames-1.txt" 2>&1 || true
{ tail -1 "$OUT/frames-1.txt" | grep -q 'frames PASS' && grep -q '264 of 330 md5-equal' "$OUT/frames-1.txt"; } \
  || check "frames: not every frame its source frame, or the copied 264 not bit-exact: $(grep -E 'FAIL|md5-equal' "$OUT/frames-1.txt" | head -3 | paste -sd' ')"
"${OT[@]}" "$P/edit.otio" 24 "a:7:151:assets/$SA.mp4" "b:13:140:assets/$SB.mp4" "a:48:96:assets/$SA.mp4" "b:150:161:assets/$SB.mp4" > "$OUT/otio-1.txt" 2>&1 || true
tail -1 "$OUT/otio-1.txt" | grep -q 'otio PASS' || check "frames: edit.otio is not the edit: $(grep FAIL "$OUT/otio-1.txt" | head -3 | paste -sd' ')"
J '(.inputs["plates/a.mp4"].video | .width == 854 and .height == 480 and .fps == 24 and .frames == 192) and (.inputs["plates/b.mp4"].video | .fps == 24 and .frames == 192)' "$P/project.lock" \
  || check "frames: project.lock does not hold the plates' probed shapes: $(jq -c '.inputs | map_values(.video)' "$P/project.lock")"
cozy --json project -p "$P" status > "$OUT/frames-status-1.json" 2>/dev/null || true
J '.fps == 24 and .cuts[0].frames == 330 and (.cuts[0].retimed // []) == []' "$OUT/frames-status-1.json" \
  || check "frames: status is not 330 frames at 24 fps, nothing retimed: $(jq -c '{fps, cuts}' "$OUT/frames-status-1.json")"

step "a project: a take at 25 fps, retimed on the full path"
sed -i 's#video: plates/b.mp4#video: plates/r.mp4#' "$P/project.yaml"
cozy project -p "$P" render all --on local > "$OUT/frames-render-2.txt" 2>&1 || check "frames: render all with r failed: $(tail -c 300 "$OUT/frames-render-2.txt")"
n=$(ffprobe -v error -count_frames -select_streams v:0 -show_entries stream=nb_read_frames,r_frame_rate -of csv=p=0 "$P/edit.mp4" 2>/dev/null) || n=
[ "$n" = "24/1,330" ] || check "frames: the retimed edit.mp4 is $n, not 330 frames at 24/1"
grep -q "330 frames (13.8 s at 24 fps); it retimes b (25 fps) to 24 fps" "$OUT/frames-render-2.txt" || check "frames: the cut does not say it retimes b: $(grep -i 'edit.mp4' "$OUT/frames-render-2.txt" | tail -1)"
"${FR[@]}" "$P/edit.mp4" 24 "$P/plates/a.mp4:7:151" "$P/plates/r.mp4:13:140" "$P/plates/a.mp4:48:96" "$P/plates/r.mp4:150:161" > "$OUT/frames-2.txt" 2>&1 || true
tail -1 "$OUT/frames-2.txt" | grep -q 'frames PASS' || check "frames: the retimed cut's frames are not their sources': $(grep FAIL "$OUT/frames-2.txt" | head -3 | paste -sd' ')"
"${OT[@]}" "$P/edit.otio" 24 "a:7:151:assets/$SA.mp4" "b:13:140:assets/$SR.mp4:25.0" "a:48:96:assets/$SA.mp4" "b:150:161:assets/$SR.mp4:25.0" > "$OUT/otio-2.txt" 2>&1 || true
tail -1 "$OUT/otio-2.txt" | grep -q 'otio PASS' || check "frames: the retimed edit.otio is not the edit: $(grep FAIL "$OUT/otio-2.txt" | head -3 | paste -sd' ')"
cozy --json project -p "$P" status > "$OUT/frames-status-2.json" 2>/dev/null || true
cozy project -p "$P" status > "$OUT/frames-status-2.txt" 2>&1 || true
{ J '.cuts[0].frames == 330 and .cuts[0].retimed == ["b (25 fps)"]' "$OUT/frames-status-2.json" && grep -q "retimes b (25 fps) to 24 fps" "$OUT/frames-status-2.txt"; } \
  || check "frames: status does not name b as retimed: $(jq -c .cuts "$OUT/frames-status-2.json")"
J '.inputs["plates/r.mp4"].video | .fps == 25 and .frames == 200' "$P/project.lock" || check "frames: project.lock does not hold r's 25 fps and 200 frames"

step "a project: the first edit again, its edges from the cache"
sed -i 's#video: plates/r.mp4#video: plates/b.mp4#' "$P/project.yaml"
cozy project -p "$P" render all --on local > "$OUT/frames-render-3.txt" 2>&1 || check "frames: render all with b back failed"
ls -1 "$CACHE" | sort > "$OUT/frames-cache-3.txt"
[ -z "$(comm -13 "$OUT/frames-cache-1.txt" "$OUT/frames-cache-3.txt")" ] || check "frames: the first edit again made edge pieces anew: $(comm -13 "$OUT/frames-cache-1.txt" "$OUT/frames-cache-3.txt" | head -3 | paste -sd' ')"
"${FR[@]}" "$P/edit.mp4" 24 "$P/plates/a.mp4:7:151" "$P/plates/b.mp4:13:140" "$P/plates/a.mp4:48:96" "$P/plates/b.mp4:150:161" > "$OUT/frames-3.txt" 2>&1 || true
{ tail -1 "$OUT/frames-3.txt" | grep -q 'frames PASS' && grep -q '264 of 330 md5-equal' "$OUT/frames-3.txt"; } || check "frames: the re-cut from the cache is not the first edit"
}

# Range, HEAD, ETag and 416 on a cut and an asset; Calcifer's Host, Origin and bearer rules.
http_surface() {
step "a project: Calcifer's HTTP surface"
local P=$1 ID=$2 U len et png lock=$HOME/.cozy/calcifer/calcifer.lock.holder
ADDR=$(jq -r .address "$lock") TOKEN=$(jq -r .token "$lock")
U=http://$ADDR/v1/projects/$ID/cuts/edit.mp4 len=$(stat -c %s "$P/edit.mp4")
api -r 1000-1999 -D "$OUT/range.hdr" -o "$OUT/range.bin" "$U" || true
{ head -1 "$OUT/range.hdr" | grep -q ' 206' && cmp -s "$OUT/range.bin" <(tail -c +1001 "$P/edit.mp4" | head -c 1000) \
  && grep -qi "content-range: bytes 1000-1999/$len" "$OUT/range.hdr"; } || check "Range 1000-1999 of edit.mp4: $(tr -d '\r' < "$OUT/range.hdr" | head -3 | paste -sd' ')"
api -I "$U" > "$OUT/head.hdr" || true
et=$(grep -i '^etag:' "$OUT/head.hdr" | cut -d' ' -f2 | tr -d '\r')
{ head -1 "$OUT/head.hdr" | grep -q ' 200' && grep -qi "content-length: $len" "$OUT/head.hdr" && grep -qi 'accept-ranges: bytes' "$OUT/head.hdr"; } \
  || check "HEAD edit.mp4: $(tr -d '\r' < "$OUT/head.hdr" | paste -sd' ')"
[ "$(api -o /dev/null -w '%{http_code}' -H "If-None-Match: $et" "$U")" = 304 ] || check "If-None-Match $et on edit.mp4 is not 304"
[ "$(api -o /dev/null -w '%{http_code}' -r "$((len + 10))-" "$U")" = 416 ] || check "a Range past edit.mp4's end is not 416"
png=$(jq -r '.nodes.yard.versions[1].assets.image.path' "$P/project.lock")
api -D "$OUT/asset.hdr" -o "$OUT/asset.bin" "http://$ADDR/v1/projects/$ID/$png" || true
{ head -1 "$OUT/asset.hdr" | grep -q ' 200' && cmp -s "$OUT/asset.bin" "$P/$png" && grep -qi "etag: \"$(basename "$png" .png)\"" "$OUT/asset.hdr"; } \
  || check "GET $png: $(tr -d '\r' < "$OUT/asset.hdr" | head -4 | paste -sd' ')"
# A rebinding Host, a foreign Origin, no bearer: each refused before anything is read or done.
[ "$(api -o /dev/null -w '%{http_code}' -H "Host: rebind.example:${ADDR##*:}" "http://$ADDR/v1/projects")" = 403 ] || check "a rebinding Host was not refused 403"
[ "$(api -o /dev/null -w '%{http_code}' -X POST -H 'Origin: http://evil.example' -H 'Content-Type: application/json' -d '{"names":["all"],"on":["local"]}' "http://$ADDR/v1/projects/$ID/runs")" = 403 ] \
  || check "a POST with a foreign Origin was not refused 403"
[ "$(curl -sS -o /dev/null -w '%{http_code}' "http://$ADDR/v1/projects")" = 401 ] || check "a request with no bearer was not refused 401"
}

# The browser session (project/browser.js) on the runner's Google Chrome, with playwright-core to drive it.
browser_session() {
step "a project: a browser on Calcifer's session cookie, and another site refused"
local P=$1 ID=$2 port pw=$OUT/playwright server
CHROME=$(command -v google-chrome || true)
[ -n "$CHROME" ] || { check "no Google Chrome on this runner"; return; }
npm install -s --prefix "$pw" playwright-core@1.55.0 > "$OUT/npm.log" 2>&1 || { check "npm install playwright-core failed: $(tail -c 300 "$OUT/npm.log")"; return; }
port=$(python3 -c 'import socket; s = socket.socket(); s.bind(("127.0.0.1", 0)); print(s.getsockname()[1])')
python3 -m http.server "$port" --bind 127.0.0.1 --directory "$here/project" > "$OUT/cross-site.log" 2>&1 & server=$!
for _ in $(seq 50); do curl -fsS -o /dev/null "http://localhost:$port/cross-site.html" && break; sleep 0.1; done
# The re-cut the page follows: the edit's last entry a second shorter.
sed 's/{shot: b, start: 12, end: 132}/{shot: b, start: 12, end: 108}/' "$P/project.yaml" > "$OUT/recut.yaml"
local code=0 log=$HOME/.cozy/calcifer/calcifer.log seen
seen=$(stat -c %s "$log" 2>/dev/null || echo 0)
CHROME=$CHROME PLAYWRIGHT=$pw/node_modules/playwright-core node "$here/project/browser.js" "$ADDR" "$TOKEN" "$ID" "$port" \
  "cp '$OUT/recut.yaml' '$P/project.yaml' && '$COZY' project -p '$P' render all --on local" > "$OUT/browser.json" 2> "$OUT/browser.err" || code=$?
kill "$server" 2> /dev/null || true
[ "$code" = 0 ] || { check "the browser failed: $(tail -c 400 "$OUT/browser.err")"; return; }
J '.session == 204 and (.cookie | any(.name == "cozy_session" and .httpOnly and .sameSite == "Strict")) and .read_on_cookie.status == 200' "$OUT/browser.json" \
  || check "browser: no HttpOnly SameSite=Strict session read the project: $(jq -c '{session, cookie, read_on_cookie}' "$OUT/browser.json")"
J '.video.seeked_to > 0 and (.same_site_ranges | any(.status == 206 and .cookie))' "$OUT/browser.json" \
  || check "browser: <video> did not seek edit.mp4 by Range on the cookie: $(jq -c '{video, same_site_ranges}' "$OUT/browser.json")"
J '(.events_on_cookie | index("open")) and (.events_on_cookie | any(startswith("cut "))) and (.events_on_cookie | index("ended"))' "$OUT/browser.json" \
  || check "browser: EventSource on the cookie did not follow the re-cut: $(jq -c .events_on_cookie "$OUT/browser.json")"
# What Calcifer saw of the other site (its log names each refusal): the fetch and both POSTs refused, none carried out.
tail -c +$((seen + 1)) "$log" | grep 'refused ' > "$OUT/browser-refused.txt" || true
{ grep -qE 'refused GET /v1/projects code="(origin_not_allowed|session_cross_site|unauthenticated)"' "$OUT/browser-refused.txt" \
  && [ "$(grep -cE "refused POST /v1/projects/$ID/runs code=\"origin_not_allowed\"" "$OUT/browser-refused.txt")" -ge 2 ] \
  && J '(.cross_site_page.fetch_get | startswith("not readable")) and (.cross_site_at_calcifer | all(.cookie | not))' "$OUT/browser.json"; } \
  || check "browser: another site was not refused by Calcifer: $(cat "$OUT/browser-refused.txt" | cut -c1-200 | paste -sd' '); page $(jq -c .cross_site_page "$OUT/browser.json")"
J '(.pending // {}) | length == 0' "$P/project.lock" && ! grep -q nothing "$P/project.lock" || check "another site's POST started a render"
}
