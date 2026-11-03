# e2e/cli/project.sh: a video project on this computer through `cozy project` and Calcifer's HTTP surface (cl-716),
# the part cli/run.sh calls `projects`, with the local machine up. No model runs: the shots are attached plates and the
# other nodes a weightless local renderer (project/draw.py), so every path but a model's is real: the folder, the
# assets, the lock, the walk, the cut, the API a web UI uses and a browser on it. Fixtures are made here with ffmpeg
# (the job's, on this shell's PATH; Calcifer fetches its own).
# `version_skew` is called by run.sh's start, between the installer's cozy and the candidates: the candidate CLI
# against the release's running Calcifer.
here=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)

# The candidate CLI from its own path against the release's Calcifer: it works where that Calcifer serves the route (every
# release since 0.16.3 does `project ls`), and otherwise a typed calcifer_outdated with its remedy, never a bare 404.
version_skew() {
  [ -f "$CAND_COZY" ] || return 0
  step "version skew: the candidate CLI against the release's Calcifer"
  local code=0
  "$CAND_COZY" project ls > "$OUT/skew.txt" 2>&1 || code=$?
  cat "$OUT/skew.txt"
  { { [ "$code" = 0 ] || grep -q calcifer_outdated "$OUT/skew.txt"; } && ! grep -qiE 'untyped_answer|answered 404' "$OUT/skew.txt"; } \
    || check "skew: the candidate CLI against $(cozy --version)'s Calcifer neither worked nor said calcifer_outdated (exit $code)"
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
for node in kyogre groudon coast coast.wide awaken erupt clash; do grep -q "$node" "$OUT/project-new-status.txt" || check "a new project's status names no $node"; done
grep -q 'cozy reads no field' "$OUT/project-new-status.txt" && check "a new project's film writes a field cozy does not read: $(grep 'cozy reads no field' "$OUT/project-new-status.txt")"
mkdir -p "$P/plates" "$E"
plate "$P/plates/a.mp4" testsrc2 440 1800; plate "$P/plates/b.mp4" testsrc 550 900; plate "$P/plates/c.mp4" rgbtestsrc 660 1200
plate "$E/look.mp4" smptebars 330 600; still "$E/stage.png" teal
cp "$here/project/draw.py" "$P/draw.py"
cat > "$P/project.yaml" <<YAML
entities:
  - {name: place, kind: environment, description: "a plain test card", render: {with: ./draw.py/draw}}
  - {name: stage, kind: environment, description: "a teal stage", image: $E/stage.png,
     setups: [{name: a, camera: "wide", render: {with: ./draw.py/draw}}]}
references:
  - name: look
    video: $E/look.mp4
shots:
  - {name: a, summary: "test pattern a, moving like <look>", duration_s: 6, video: plates/a.mp4}
  - {name: b, summary: "test pattern b", duration_s: 6, video: plates/b.mp4}
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
J '.inputs["plates/a.mp4"] | has("original") or has("method") | not' "$P/project.lock" || check "project.lock says how this computer took plates/a.mp4: $(jq -c '.inputs["plates/a.mp4"]' "$P/project.lock")"
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

step "a project: a copy of its yaml, lock and real assets, its paths gone, needs nothing"
local copy=$OUT/garden-copy
mkdir -p "$copy"; cp "$P/project.yaml" "$P/project.lock" "$copy/"; cp -rL "$P/assets" "$copy/assets" || true
mv "$E" "$E-away"
cozy --json project -p "$copy" status > "$OUT/project-copy.json" 2> "$OUT/project-copy.err" || check "status of a copy failed: $(tail -c 300 "$OUT/project-copy.err")"
J '((.warnings // []) | map(select(test("missing"))) | length == 0) and ((.missing // []) | length == 0) and .absent == 0' "$OUT/project-copy.json" \
  || check "a copy with its assets, its paths gone: $(jq -c '{warnings, missing, absent}' "$OUT/project-copy.json")"
rm -rf "$copy/assets"
cozy project -p "$copy" status > "$OUT/project-copy-bare.txt" 2>&1 || true
grep -q 'next: cozy project pull' "$OUT/project-copy-bare.txt" || check "a copy without its assets: status does not say to pull: $(tail -c 300 "$OUT/project-copy-bare.txt")"
mv "$E-away" "$E"

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

references
never_refuse
reflinked
frames_otio
take_size
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

# references: a top-level reference reaches the shots that mention it (on any line, in any case) and no other,
# as the function gets it (shoot.py says what it was given); editing its file puts only those out of date and
# render all remakes only those; gone, they cannot render and the rest can. What is refused and warned about.
references() {
step "a project: references reach the shots that mention them"
local R=$OUT/refs
cozy project new "$R" > /dev/null 2>&1 || check "project new for references failed"
cp "$here/project/shoot.py" "$R/shoot.py"; cp "$here/project/draw.py" "$R/draw.py"
plate "$R/one.mp4" testsrc2 440 300; plate "$R/two.mp4" testsrc 550 300; plate "$R/three.mp4" rgbtestsrc 660 300
plate "$R/clip.mp4" smptebars 330 300; still "$R/look.png" teal
ffmpeg -v error -y -f lavfi -i "sine=frequency=220:sample_rate=32000:duration=2" "$R/voice.wav"
local ONE TWO THREE
cat > "$R/project.yaml" <<YAML
defaults:
  shot:
    with: ./shoot.py/shoot
references:
  - name: clip
    video: clip.mp4
    use_audio: true
  - name: narrator
    audio: voice.wav
  - name: look
    image: look.png
shots:
  - name: mentions
    summary: <clip> again, <narrator> over it, in <look>.
    duration_s: 6
    plate:
      video: one.mp4
  - name: again
    summary: A plain opening.
    beats:
      - at: 3
        description: Then as in <CLIP>.
    duration_s: 6
    plate:
      video: two.mp4
  - name: plain
    summary: Nothing named here.
    duration_s: 6
    plate:
      video: three.mp4
YAML
local code=0
cozy project -p "$R" render all --on local > "$OUT/refs-render.txt" 2>&1 || code=$?
[ "$code" = 0 ] || { check "references: render all: exit $code: $(tail -c 400 "$OUT/refs-render.txt")"; return; }
given() { jq -c '[.references[] | {name, kind, sha256, use_audio}]' "$R/$(jq -r --arg s "$1" '.nodes[$s].versions[-1].assets.given.path' "$R/project.lock")"; }
local CLIP LOOK VOICE want
CLIP=$(sha "$R/clip.mp4") LOOK=$(sha "$R/look.png") VOICE=$(sha "$R/voice.wav")
want="[{\"kind\":\"video\",\"name\":\"clip\",\"sha256\":\"$CLIP\",\"use_audio\":true},{\"kind\":\"audio\",\"name\":\"narrator\",\"sha256\":\"$VOICE\",\"use_audio\":false},{\"kind\":\"prop\",\"name\":\"look\",\"sha256\":\"$LOOK\",\"use_audio\":false}]"
ONE=$(given mentions) TWO=$(given again) THREE=$(given plain)
[ "$(jq -cS . <<< "$ONE")" = "$(jq -cS . <<< "$want")" ] || check "references: mentions was given $ONE, not $want"
[ "$(jq -c '[.[].name]' <<< "$TWO")" = '["clip"]' ] || check "references: a mention on a later line, in another case, was given $TWO"
[ "$THREE" = '[]' ] || check "references: a shot that mentions none was given $THREE"
J '.nodes.mentions.versions[-1].upstream | any(.node == "clip" and .asset == "sha256:'"$CLIP"'")' "$R/project.lock" \
  || check "references: the take does not record what it read: $(jq -c '.nodes.mentions.versions[-1].upstream' "$R/project.lock")"
[ "$(readlink "$R/assets/$CLIP.mp4")" = "$R/clip.mp4" ] \
  || check "references: clip.mp4 not taken in by symlink: $(ls -l "$R/assets/$CLIP.mp4")"
cozy --json project -p "$R" status > "$OUT/refs-status.json" 2> /dev/null || check "references: status failed"
J '(.stale | length == 0) and (.warnings | length == 0) and (.nodes[] | select(.name == "clip") | .kind == "reference" and .versions == 0 and .assets.video.sha256 == "sha256:'"$CLIP"'")' "$OUT/refs-status.json" \
  || check "references: after render all: $(jq -c '{stale, warnings, clip: (.nodes[]? | select(.name == "clip"))}' "$OUT/refs-status.json")"

step "a project: a reference edited puts only what mentions it out of date"
plate "$R/clip.mp4" rgbtestsrc 330 400
cozy project -p "$R" status > "$OUT/refs-changed.txt" 2>&1 || true
grep -q 'reference changed: clip.mp4' "$OUT/refs-changed.txt" || check "references: status does not say clip.mp4 changed: $(tail -c 300 "$OUT/refs-changed.txt")"
cozy --json project -p "$R" status > "$OUT/refs-changed.json" 2> /dev/null || true
J '[.stale[] | "\(.name): \(.why)"] | sort == ["again: clip changed", "mentions: clip changed"]' "$OUT/refs-changed.json" \
  || check "references: out of date: $(jq -c .stale "$OUT/refs-changed.json"), not exactly again and mentions"
cozy project -p "$R" render all --on local > "$OUT/refs-render-2.txt" 2>&1 || check "references: render all after the edit: $(tail -c 300 "$OUT/refs-render-2.txt")"
J '(.nodes.mentions.versions | length) == 2 and (.nodes.again.versions | length) == 2 and (.nodes.plain.versions | length) == 1' "$R/project.lock" \
  || check "references: render all did not remake exactly the two: $(jq -c '.nodes | map_values(.versions | length)' "$R/project.lock")"
CLIP=$(sha "$R/clip.mp4")
[ "$(given again | jq -r '.[0].sha256')" = "$CLIP" ] || check "references: again's new take was not given the edited clip: $(given again)"

step "a project: a reference gone, a reference rendered, a name used twice"
mv "$R/clip.mp4" "$R/clip-moved.mp4"
cozy project -p "$R" status > "$OUT/refs-missing.txt" 2>&1 || true
grep -q 'reference missing: clip.mp4: relink' "$OUT/refs-missing.txt" || check "references: a moved file: status does not say relink: $(tail -c 300 "$OUT/refs-missing.txt")"
code=0; cozy project -p "$R" render mentions --on local > "$OUT/refs-missing-render.txt" 2>&1 || code=$?
{ [ "$code" != 0 ] && grep -q 'reference missing' "$OUT/refs-missing-render.txt"; } || check "references: mentions rendered with its file gone (exit $code): $(tail -c 300 "$OUT/refs-missing-render.txt")"
cozy project -p "$R" render plain --on local > "$OUT/refs-plain.txt" 2>&1 || check "references: plain did not render with an unrelated file gone: $(tail -c 300 "$OUT/refs-plain.txt")"
mv "$R/clip-moved.mp4" "$R/clip.mp4"
code=0; cozy project -p "$R" render clip --on local > "$OUT/refs-made.txt" 2>&1 || code=$?
{ [ "$code" != 0 ] && grep -q reference_file "$OUT/refs-made.txt"; } || check "references: render clip was not refused (exit $code): $(tail -c 300 "$OUT/refs-made.txt")"
cp "$R/project.yaml" "$OUT/refs-ok.yaml"
cat >> "$R/project.yaml" <<YAML
entities:
  - name: CLIP
    kind: prop
    description: A clip.
YAML
code=0; cozy project -p "$R" status > "$OUT/refs-twice.txt" 2>&1 || code=$?
{ [ "$code" != 0 ] && grep -q 'two nodes are named' "$OUT/refs-twice.txt"; } || check "references: a name used twice, in another case, was not refused (exit $code): $(tail -c 300 "$OUT/refs-twice.txt")"
cp "$OUT/refs-ok.yaml" "$R/project.yaml"
cat >> "$R/project.yaml" <<YAML
entities:
  - name: set
    kind: environment
    description: Seen in <clip>.
    render:
      with: ./draw.py/draw
YAML
code=0; cozy project -p "$R" status > "$OUT/refs-reader.txt" 2>&1 || code=$?
{ [ "$code" = 0 ] && grep -q 'which only a shot reads: it stays plain text' "$OUT/refs-reader.txt"; } || check "references: an entity mentioning a clip did not warn and go on (exit $code): $(tail -c 300 "$OUT/refs-reader.txt")"
cozy project -p "$R" render set --on local > "$OUT/refs-reader-render.txt" 2>&1 || check "references: an entity mentioning a clip did not render: $(tail -c 300 "$OUT/refs-reader-render.txt")"
J '.nodes.set.versions[-1].upstream | all(.node != "clip")' "$R/project.lock" || check "references: an entity read the clip it mentioned: $(jq -c '.nodes.set.versions[-1].upstream' "$R/project.lock")"

step "a project: a shot's own references: is ignored, with the warning"
cp "$OUT/refs-ok.yaml" "$R/project.yaml"
sed -i 's|^  - name: plain$|  - name: plain\n    references:\n      - name: old\n        kind: video\n        video: clip.mp4|' "$R/project.yaml"
cozy --json project -p "$R" status > "$OUT/refs-old.json" 2> "$OUT/refs-old.err" || check "references: status with a shot's references: failed: $(tail -c 300 "$OUT/refs-old.err")"
J '.warnings | any(test("cozy reads no field shots\\.2\\.references; it is ignored"))' "$OUT/refs-old.json" \
  || check "references: a shot's references: did not warn: $(jq -c .warnings "$OUT/refs-old.json")"
J '[.nodes[].name] | index("old") == null' "$OUT/refs-old.json" || check "references: a shot's references: was read: $(jq -c '[.nodes[].name]' "$OUT/refs-old.json")"
}

# never_refuse: nothing a shot says is refused. A take shorter than duration_s is cut at its real length, a write or a
# setup's frame past its end takes its last frame, a plate its function does not take is left out, and status says so
# and what its function said. No text renders, and a function with no adapter gets one flat prompt: the summary, then
# each beat's description. A shot mentioning a state it writes splits there and reads its own frame, or, where it cannot, renders on
# through and says so. A mention that closes a cycle stays plain text, while a cycle of setup: and frame: is refused. An
# edit past a take's end leaves that cut out. A stale prompt: or description: is a field cozy does not read, and a
# text opening a YAML line with `[` is told where it goes.
never_refuse() {
step "a project: nothing a shot says is refused"
local N=$OUT/never code
cozy project new "$N" > /dev/null 2>&1 || check "project new for never_refuse failed"
cp "$here/project/shoot.py" "$N/shoot.py"; cp "$here/project/adapted.py" "$N/adapted.py"; cp "$here/project/draw.py" "$N/draw.py"
plate "$N/plate.mp4" testsrc2 440 300; still "$N/still.png" teal
cat > "$N/project.yaml" <<YAML
defaults:
  shot:
    with: ./shoot.py/shoot
references:
  - name: clip
    video: plate.mp4
entities:
  - name: place
    kind: environment
    description: A plain room.
    image: still.png
    setups:
      - name: a
        camera: wide
        frame:
          shot: long
          at: 30
        render:
          with: ./draw.py/draw
  - name: left
    kind: environment
    description: Beside <right>.
    render:
      with: ./draw.py/draw
  - name: right
    kind: environment
    description: Beside <left>.
    render:
      with: ./draw.py/draw
world_states:
  - name: closing
    image: still.png
  - name: marked
    image: still.png
  - name: too-short
    image: still.png
  - name: uncontinued
    image: still.png
shots:
  - name: long
    summary: A long shot.
    duration_s: 20
    plate:
      video: plate.mp4
    writes:
      - state: closing
        at: 12
  - name: silent
    duration_s: 6
    plate:
      video: plate.mp4
  - name: timed
    summary: A plain room.
    beats:
      - at: 3
        description: The rest, after <marked>.
      - at: 4
        description: A wall.
    duration_s: 6
    plate:
      video: plate.mp4
    writes:
      - state: marked
        at: 2
  - name: adapted
    summary: The scene.
    beats:
      - at: 0
        description: One.
      - at: 3
        description: Two.
    soundscape: Rain.
    music: Drums.
    duration_s: 6
    render:
      with: ./adapted.py/shoot
    plate:
      video: plate.mp4
  - name: plated
    summary: As in <clip>.
    duration_s: 6
    render:
      with: ./shoot.py/bare
    plate:
      video: plate.mp4
  - name: short
    summary: One call, its renderer's shortest piece over the write, after <too-short>.
    duration_s: 6
    render:
      with: ./shoot.py/pieces
    plate:
      video: plate.mp4
    writes:
      - state: too-short
        at: 2
  - name: whole
    summary: One call, its renderer cannot continue, as in <clip>, after <uncontinued>.
    duration_s: 6
    render:
      with: ./shoot.py/bare
    writes:
      - state: uncontinued
        at: 2
YAML
code=0; cozy project -p "$N" render all --on local > "$OUT/never-render.txt" 2>&1 || code=$?
[ "$code" = 0 ] || { check "never_refuse: render all: exit $code: $(tail -c 500 "$OUT/never-render.txt")"; return; }
cozy --json project -p "$N" status > "$OUT/never-status.json" 2> /dev/null || check "never_refuse: status failed"
said() { J --arg t "$1" '.warnings | any(contains($t))' "$OUT/never-status.json" || check "never_refuse: status does not say '$1': $(jq -c .warnings "$OUT/never-status.json")"; }
said 'long: rendered 6 s of 20 s'
said 'long: its video ends before 11.98 s: its last frame stands in'
said 'silent has no summary and no beats'
said 'and its beat times are not honoured'
said 'has no `version:`, so it reads as version 1'
said "place.a's frame is at 30 s, outside long's 0–20 s: the nearest frame stands in"
said 'place.a: its video ends before 30.00 s: its last frame stands in'
said "plated's renderer takes no plate: it renders without it"
said 'right mentions left, which is made from it: that mention stays plain text'
said "short mentions too-short and writes it at 2 s, but a call from 0 s to 2 s is under its renderer's 5 s"
said 'whole mentions uncontinued and writes it at 2 s, but its renderer cannot continue a video'
seen() { jq -e "$2" "$N/$(jq -r --arg s "$1" '.nodes[$s].versions[-1].assets.given.path' "$N/project.lock")" > /dev/null || check "never_refuse: $1 was given $(jq -c . "$N/$(jq -r --arg s "$1" '.nodes[$s].versions[-1].assets.given.path' "$N/project.lock")"), not $2"; }
seen long '.duration_s == 20'
seen adapted '.prompt == "adapted v1 adapted: The scene. | 0: One. | 3: Two. | Rain. | Drums."'
J '.nodes.adapted.versions[-1].invocations[0] | .function == "shoot" and (.input.prompt | startswith("adapted v1"))' "$N/project.lock" \
  || check "never_refuse: the take did not record its Invocation: $(jq -c '.nodes.adapted.versions[-1].invocations' "$N/project.lock")"
seen silent '.prompt == ""'
seen plated '[.references[].name] == ["clip"]'
seen timed '.continues and .prompt == "A plain room.\n\nThe rest, after <marked>.\nA wall." and ([.references[].name] == ["marked"])'
seen short '(.continues | not) and .duration_s == 6'
seen whole '.duration_s == 6'
J '(.nodes.timed.versions[-1] | (.runs | length) == 2 and .writes.marked != null) and (.nodes.short.versions[-1].runs | length) == 1 and (.nodes.whole.versions[-1].runs | length) == 1' "$N/project.lock" \
  || check "never_refuse: calls: $(jq -c '.nodes | map_values(.versions[-1].runs)' "$N/project.lock")"
J '(.nodes.left.versions | length) == 1 and (.nodes.right.versions | length) == 1 and (.nodes.left.versions[-1].upstream | any(.node == "right")) and (.nodes.right.versions[-1].upstream | length == 0)' "$N/project.lock" \
  || check "never_refuse: a cycle of mentions: $(jq -c '[.nodes.left, .nodes.right] | map(.versions[-1].upstream)' "$N/project.lock")"
J '.nodes["place.a"].versions | length == 1' "$N/project.lock" || check "never_refuse: a setup's frame past its shot made no version: $(jq -c '.nodes["place.a"]' "$N/project.lock")"
[ -s "$N/$(jq -r '.nodes.long.versions[-1].writes.closing.path' "$N/project.lock")" ] || check "never_refuse: long's write past its end kept no frame: $(jq -c '.nodes.long.versions[-1].writes' "$N/project.lock")"
frames() { ffprobe -v error -count_frames -select_streams v:0 -show_entries stream=nb_read_frames -of csv=p=0 "$1"; }
local cut; cut=$(jq -r '.cuts[] | select(.cut == "edit.mp4") | .frames' "$OUT/never-status.json")
{ [ "$cut" = $((7 * 144)) ] && [ "$(frames "$N/edit.mp4")" = "$cut" ]; } \
  || check "never_refuse: every take is 6 s, but status says $cut frames and edit.mp4 has $(frames "$N/edit.mp4"), not $((7 * 144))"

step "a project: an edit past a take's end, stale fields, and a line opened with a bracket"
cat >> "$N/project.yaml" <<YAML
edit:
  - shot: long
    start: 200
  - shot: silent
YAML
cozy project -p "$N" render all --on local > "$OUT/never-past.txt" 2>&1 || check "never_refuse: an edit past a take's end was refused: $(tail -c 300 "$OUT/never-past.txt")"
grep -q "the edit cuts long from frame 200, past its take's end: that cut is left out" "$OUT/never-past.txt" || check "never_refuse: render all did not say the cut is left out: $(tail -c 300 "$OUT/never-past.txt")"
[ "$(frames "$N/edit.mp4")" = 144 ] || check "never_refuse: edit.mp4 is not silent's 144 frames: $(frames "$N/edit.mp4")"
cp "$N/project.yaml" "$OUT/never-ok.yaml"
sed -i 's|^  - name: silent$|  - name: silent\n    prompt: Stale.\n    description: Stale.|' "$N/project.yaml"
cozy --json project -p "$N" status > "$OUT/never-stale.json" 2> /dev/null || check "never_refuse: status with stale fields failed"
J '[.warnings[] | select(test("cozy reads no field shots\\.1\\.(prompt|description); it is ignored"))] | length == 2' "$OUT/never-stale.json" \
  || check "never_refuse: stale prompt: and description: did not warn: $(jq -c .warnings "$OUT/never-stale.json")"
cp "$OUT/never-ok.yaml" "$N/project.yaml"
# A cycle of what the file declares, with no mention in it, is a malformed project.
sed -i 's|^          shot: long$|          shot: long-way|; s|^  - name: silent$|  - name: long-way\n    setup: place.a\n    duration_s: 6\n  - name: silent|' "$N/project.yaml"
code=0; cozy project -p "$N" status > "$OUT/never-cycle.txt" 2>&1 || code=$?
{ [ "$code" != 0 ] && grep -q 'a cycle: ' "$OUT/never-cycle.txt"; } || check "never_refuse: a cycle of setup: and frame: was not refused (exit $code): $(tail -c 300 "$OUT/never-cycle.txt")"
cp "$OUT/never-ok.yaml" "$N/project.yaml"
sed -i 's|^    summary: A long shot\.$|    summary: [Shot 1] A long shot.|' "$N/project.yaml"
code=0; cozy project -p "$N" status > "$OUT/never-bracket.txt" 2>&1 || code=$?
{ [ "$code" != 0 ] && grep -q 'description: |' "$OUT/never-bracket.txt"; } || check "never_refuse: a line opened with [ did not say where it goes (exit $code): $(tail -c 300 "$OUT/never-bracket.txt")"
cp "$OUT/never-ok.yaml" "$N/project.yaml"
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
{ [ -f "$P/assets/$S.png" ] && [ ! -L "$P/assets/$S.png" ]; } \
  || check "XFS: refs/stage.png not reflinked: $(ls -l "$P/assets/" | head -5)"
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
  - {name: a, summary: "test pattern a", duration_s: 8, video: plates/a.mp4}
  - {name: b, summary: "test pattern b", duration_s: 8, video: plates/b.mp4}
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
# cl-728: takes of a model's rounded size (H3's 1344x768, 1.6% from 16:9) with no `resolution:`: the edit is their size,
# their frames unencoded (no edge piece, each frame its take's) and the same file cut twice; a plate of another size still
# fills it.
take_size() {
step "a project: no resolution, takes at 1344x768 join unencoded"
local P=$OUT/rounded CACHE=$HOME/.cache/cozy/cuts first
cozy project new "$P" > /dev/null 2>&1 || check "rounded: project new failed"
mkdir -p "$P/plates"
wide() { ffmpeg -v error -y -f lavfi -i "$2=size=$3:rate=24:duration=3" -c:v libx264 -preset veryfast -g 24 -keyint_min 24 -sc_threshold 0 -pix_fmt yuv420p "$1"; }
wide "$P/plates/a.mp4" testsrc2 1344x768; wide "$P/plates/b.mp4" testsrc 1344x768; wide "$P/plates/c.mp4" rgbtestsrc 854x480
cat > "$P/project.yaml" <<'YAML'
aspect_ratio: "16:9"
fps: 24
shots:
  - name: a
    summary: |
      Test pattern a.
    duration_s: 3
    video: plates/a.mp4
  - name: b
    summary: |
      Test pattern b.
    duration_s: 3
    video: plates/b.mp4
edit:
  - {shot: a, start: 0, end: 48}
  - {shot: b, start: 24, end: 48}
YAML
cozy project -p "$P" render all --on local > "$OUT/rounded-1.txt" 2>&1 || check "rounded: render all failed: $(tail -c 300 "$OUT/rounded-1.txt")"
size() { ffprobe -v error -select_streams v:0 -show_entries stream=width,height -of csv=p=0 "$1"; }
[ "$(size "$P/edit.mp4")" = 1344,768 ] || check "rounded: edit.mp4 is $(size "$P/edit.mp4"), not its takes' 1344,768"
md5s() { ffmpeg -v error -i "$1" -map 0:v -f framemd5 - | grep -v '^#' | awk '{print $NF}'; }
[ "$( (md5s "$P/plates/a.mp4" | sed -n 1,48p; md5s "$P/plates/b.mp4" | sed -n 25,48p) | md5sum)" = "$(md5s "$P/edit.mp4" | md5sum)" ] || check "rounded: edit.mp4's frames are not its takes'"
! ls -1 "$CACHE" | grep -q "^$(sha "$P/plates/a.mp4" | cut -c1-16)-" || check "rounded: an edge of a was encoded"
first=$(sha "$P/edit.mp4")
cozy project -p "$P" render all --on local > "$OUT/rounded-2.txt" 2>&1 || check "rounded: render all again failed"
[ "$(sha "$P/edit.mp4")" = "$first" ] || check "rounded: cut again, edit.mp4 is another file"
sed -i 's#video: plates/b.mp4#video: plates/c.mp4#' "$P/project.yaml"
cozy project -p "$P" render all --on local > "$OUT/rounded-3.txt" 2>&1 || check "rounded: render all with an 854x480 plate failed"
[ "$(size "$P/edit.mp4")" = 1344,768 ] || check "rounded: a plate of another size: edit.mp4 is $(size "$P/edit.mp4"), not 1344,768"
}

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
