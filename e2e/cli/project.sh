# e2e/cli/project.sh: a project on this computer through `cozy project` and Calcifer's HTTP surface (cl-758), the part cli/run.sh
# calls `projects`, with the local machine up. No model runs: pictures come from a weightless local function (project/draw.py), shots
# from project/shots.py, and plates from ffmpeg, so every path but a model's is real: the folder, the files, the lock, the walk, the
# adapter, the cut, the API a web UI uses and a browser on it.
# `version_skew` is called by run.sh's start, between the installer's cozy and the candidates: the candidate CLI against the release's
# running Calcifer.
# Alone (`e2e/cli/project.sh --out DIR`) it drives this computer's cozy as it is, in the default home, and leaves out what fails on
# purpose: no deliberately failing run goes in a person's history.
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

# plate <file> <lavfi source> <tone Hz> <video kb/s> [seconds] [size] [rate]: at 24 fps by default, a keyframe a second, constant bitrate;
# AAC 192k stereo at 32 kHz.
plate() {
  local secs=${5:-6} size=${6:-854x480} rate=${7:-24}
  ffmpeg -v error -y -f lavfi -i "$2=size=$size:rate=$rate:duration=$secs" -f lavfi -i "sine=frequency=$3:sample_rate=32000:duration=$secs" \
    -c:v libx264 -preset veryfast -b:v "$4k" -minrate "$4k" -maxrate "$4k" -bufsize "$4k" -x264-params nal-hrd=cbr -g "$rate" -keyint_min "$rate" -sc_threshold 0 \
    -pix_fmt yuv420p -c:a aac -b:a 192k -ac 2 -ar 32000 -shortest "$1"
}
still() { ffmpeg -v error -y -f lavfi -i "color=c=$2:size=832x480" -frames:v 1 "$1"; }
sha() { sha256sum "$1" | cut -c1-64; }
J() { jq -e "$@" > /dev/null; }
# prefix <project> <file>: `cozy project import`, and what project.yaml writes for it.
prefix() { (cd "$1" && cozy --json project import "$2" | jq -r .prefix) || check "import $2 failed"; }
# api <curl args…>: Calcifer, with the bearer of its lock record.
api() { curl -sS -H "Authorization: Bearer $TOKEN" "$@"; }
# status <project> [file]: its status as JSON.
status() { cozy --json project -p "$1" status 2> /dev/null; }
# row <project> <asset> <jq>: that of its row in status.
row() { status "$1" | jq -c --arg n "$2" ".assets[] | select(.name == \$n) | $3"; }
# made <project> <asset>: how many versions the lock keeps.
made() { jq --arg n "$2" '.assets[$n].versions | length' "$1/project.lock"; }
# runs <project>: every run its lock records.
runs() { jq '[.assets[].versions[].runs[]?] | length' "$1/project.lock"; }
# render <project> <args…>: on this computer, its words in $OUT/render.txt.
render() { local P=$1; shift; cozy project -p "$P" render "$@" --on local > "$OUT/render.txt" 2>&1; }
says() { grep -qF -- "$2" "$OUT/render.txt" || check "$1: render does not say '$2': $(tail -c 400 "$OUT/render.txt")"; }

projects() {
step "a project: new, walk-up, -p"
local P=$OUT/garden code ID
cozy project new "$P" > "$OUT/project-new.txt" 2>&1 || check "project new failed: $(tail -c 300 "$OUT/project-new.txt")"
[ "$(ls -A "$P" 2>/dev/null)" = project.yaml ] || check "project new made $(ls -A "$P" 2>/dev/null | tr '\n' ' '), not only project.yaml"
cozy project -p "$P" status > "$OUT/project-new-status.txt" 2>&1 || check "status of a new project failed: $(tail -c 300 "$OUT/project-new-status.txt")"
for name in kyogre groudon coast "coast wide" awaken erupt clash edit; do grep -q "$name" "$OUT/project-new-status.txt" || check "a new project's status names no $name"; done
mkdir -p "$P/sub"
(cd "$P/sub" && cozy --json project status) > "$OUT/project-status.json" 2> /dev/null || check "status from a subfolder failed"
(cd / && cozy --json project -p "$P" status) > /dev/null 2>&1 || check "status by -p from / failed"
ID=$(jq -r '.id // empty' "$OUT/project-status.json")
[ -n "$ID" ] || check "project status --json names no id"

hard_cut
memo
pins
orphans
keeps
world_states
adapters
cuts "$P" "$ID"
imports
[ -n "${ALONE:-}" ] || failures
http_surface "$P" "$ID"
[ -n "${ALONE:-}" ] || browser_session "$P" "$ID"

step "a project: the index, a moved folder missing, DELETE forgets"
cozy --json project ls > "$OUT/project-ls-1.json" || check "project ls failed"
J --arg p "$P" '.projects | any(.path == $p and (.missing | not))' "$OUT/project-ls-1.json" || check "project ls does not list $P"
mv "$P" "$OUT/garden-moved"
cozy --json project ls > "$OUT/project-ls-2.json" || check "project ls after the move failed"
J --arg i "$ID" '.projects | any(.id == $i and .missing)' "$OUT/project-ls-2.json" || check "a moved project is not missing"
[ "$(api -o /dev/null -w '%{http_code}' -X DELETE "http://$ADDR/v1/projects/$ID")" = 204 ] || check "DELETE /v1/projects/$ID did not answer 204"
cozy --json project ls > "$OUT/project-ls-3.json" || true
{ J --arg a "$ID" '.projects | all(.id != $a)' "$OUT/project-ls-3.json" && [ -f "$OUT/garden-moved/project.yaml" ]; } || check "DELETE did not forget the project, or touched its folder"
forget_all
}

# Format 1, of the file or of the lock, is said and left as it is: nothing writes over it.
hard_cut() {
step "a project: format 1 is refused, untouched"
local O=$OUT/old code before
mkdir -p "$O"
printf 'version: 1\nentities:\n  - {name: a, kind: character}\n' > "$O/project.yaml"
code=0; cozy project -p "$O" status > "$OUT/old-yaml.txt" 2>&1 || code=$?
{ [ "$code" != 0 ] && grep -q 'format 1' "$OUT/old-yaml.txt"; } || check "a format 1 project.yaml was read (exit $code): $(tail -c 200 "$OUT/old-yaml.txt")"
printf 'version: 2\nassets:\n  a:\n    description: A.\n' > "$O/project.yaml"
printf '{"nodes": {"a": {"kind": "character", "versions": [{"number": 1}]}}}\n' > "$O/project.lock"
before=$(sha "$O/project.lock")
code=0; cozy project -p "$O" status > "$OUT/old-lock.txt" 2>&1 || code=$?
{ [ "$code" != 0 ] && grep -q 'format 1' "$OUT/old-lock.txt" && [ "$(sha "$O/project.lock")" = "$before" ]; } || check "a format 1 project.lock was read or written (exit $code): $(tail -c 200 "$OUT/old-lock.txt")"
}

# The key: a run only when an asset has no state. What doesn't change it, what does, reverting, and what reads what.
memo() {
step "a project: render makes what has no state, and nothing else"
local T=$OUT/memo first was reads
mkdir -p "$T"
cat > "$T/project.yaml" <<YAML
version: 2
defaults:
  all:
    style: Plain.
  environment:
    with: $here/project/draw.py
  picture:
    with: $here/project/draw.py
assets:
  card:
    kind: environment
    description: A test card.
  poster:
    kind: picture
    description: <card> as a poster.
  cleaned:
    with: $here/project/clean.py
    kind: clean
    image: <card>
YAML
render "$T" all || check "memo: render all: $(tail -c 400 "$OUT/render.txt")"
[ "$(jq -c '[.assets.card, .assets.poster, .assets.cleaned | .versions | length]' "$T/project.lock")" = "[1,1,1]" ] || check "memo: not one version each: $(jq -c '.assets | map_values(.versions | length)' "$T/project.lock")"
J '.assets.card.versions[0].seed != null and .assets.cleaned.versions[0].seed == null' "$T/project.lock" || check "memo: a seed was not drawn for draw, or was for clean, which takes none"
J '.format == 2 and (.files | to_entries | all(.value.path | test("^files/[0-9a-f]{64}")))' "$T/project.lock" || check "memo: the lock is not format 2 with files under files/"
J '.assets.poster.versions[0].reads[0].name == "card"' "$T/project.lock" || check "memo: poster's version does not record reading card"
first=$(runs "$T")

step "a project: nothing to do, and what changes no key"
render "$T" all || check "memo: render all again: $(tail -c 300 "$OUT/render.txt")"
[ "$(runs "$T")" = "$first" ] || check "memo: a second render all ran something"
was=$(row "$T" card .inputs)
{ printf '# a note\n'; cat "$T/project.yaml"; } > "$T/y" && mv "$T/y" "$T/project.yaml"
sed -i 's/description: A test card\./description: "  A test card.  "/' "$T/project.yaml"
sed -i 's/^    kind: environment$/    kind: "environment"/' "$T/project.yaml"
[ "$(row "$T" card .inputs)" = "$was" ] || check "memo: a comment, quotes or trailing spaces moved card's key"
sed -i 's/description: "  A test card.  "/description: A test card./; s/kind: "environment"/kind: environment/' "$T/project.yaml"

step "a project: a text edit, and reverting returns the old output with no run"
local old_file; old_file=$(row "$T" card .path)
sed -i 's/description: A test card\./description: Another card./' "$T/project.yaml"
[ "$(row "$T" card .status)" = '"out of date: description changed"' ] || check "memo: a text edit: card is $(row "$T" card .status)"
[ "$(row "$T" poster .status)" = '"waits for what it reads"' ] || check "memo: poster reads card, which has no state: $(row "$T" poster .status)"
render "$T" poster || check "memo: render poster: $(tail -c 300 "$OUT/render.txt")"
[ "$(jq -c '[.assets.card, .assets.poster | .versions | length]' "$T/project.lock")" = "[2,2]" ] || check "memo: rendering a reader did not make what it reads first: $(jq -c '.assets | map_values(.versions | length)' "$T/project.lock")"
first=$(runs "$T")
sed -i 's/description: Another card\./description: A test card./' "$T/project.yaml"
J '[.assets[] | select(.name == "card" or .name == "poster") | .status] == ["current", "current"]' <<< "$(status "$T")" || check "memo: reverted, not current: $(status "$T" | jq -c '[.assets[] | {name, status}]')"
[ "$(row "$T" card .path)" = "$old_file" ] || check "memo: reverted, card reads $(row "$T" card .path), not $old_file"
render "$T" all || check "memo: render all, reverted"
[ "$(runs "$T")" = "$first" ] || check "memo: reverting ran something"

step "a project: defaults and options are inputs; a release in with is not"
sed -i 's/style: Plain\./style: Bold./' "$T/project.yaml"
J '([.stale[]?.name] | index("card")) and ([.missing[]?] | index("poster"))' <<< "$(status "$T")" || check "memo: a defaults edit did not put card out of date, and poster waiting on it: $(status "$T" | jq -c '{stale, missing}')"
sed -i 's/style: Bold\./style: Plain./' "$T/project.yaml"
cat >> "$T/project.yaml" <<YAML
  keyed:
    with: _/birefnet@9.9.9
    kind: clean
    image: <card>
YAML
was=$(row "$T" keyed .inputs)
sed -i 's#_/birefnet@9.9.9#birefnet#' "$T/project.yaml"
[ "$(row "$T" keyed .inputs)" = "$was" ] || check "memo: a release or _/ in with moved the key"
sed -i 's#with: birefnet#with: ~/a/checkout/of/birefnet#' "$T/project.yaml"
[ "$(row "$T" keyed .inputs)" = "$was" ] || check "memo: a local checkout of the package moved the key"
sed -i 's#with: ~/a/checkout/of/birefnet#with: other#' "$T/project.yaml"
[ "$(row "$T" keyed .inputs)" != "$was" ] || check "memo: another package did not move the key"
sed -i 's#with: other#with: birefnet\n    options: {steps: 2}#' "$T/project.yaml"
[ "$(row "$T" keyed .inputs)" != "$was" ] || check "memo: options did not move the key"
[ "$(status "$T" | jq -c '[.warnings[]? | select(test("no section"))]')" = "[]" ] || check "memo: a format 2 file warned of a section"
sed -i '/^  keyed:$/,$d' "$T/project.yaml"

step "a project: --new and -n"
first=$(made "$T" card)
render "$T" card --new || check "memo: --new: $(tail -c 300 "$OUT/render.txt")"
[ "$(made "$T" card)" = "$((first + 1))" ] || check "memo: --new made $(made "$T" card) of card, from $first"
render "$T" card -n 2 || check "memo: -n 2: $(tail -c 300 "$OUT/render.txt")"
[ "$(made "$T" card)" = "$((first + 3))" ] || check "memo: -n 2 made $(made "$T" card) of card, from $((first + 1))"
J '[.assets.card.versions[-3:][].seed] | unique | length == 3' "$T/project.lock" || check "memo: --new and -n did not draw new seeds"
J '[.assets.card.versions[].number] == [range(1; (.assets.card.versions | length) + 1)]' "$T/project.lock" || check "memo: card's numbers are not in order"
render "$T" cleaned || check "memo: cleaned over a new card"
first=$(made "$T" cleaned)
render "$T" cleaned --new || check "memo: --new on a function with no seed: $(tail -c 300 "$OUT/render.txt")"
says memo "takes no seed"
[ "$(made "$T" cleaned)" = "$first" ] || check "memo: --new on a function with no seed made a version"
code=0; cozy project -p "$T" render card -n 2 --approve --on local > "$OUT/render.txt" 2>&1 || code=$?
{ [ "$code" != 0 ] && says memo --approve; } || check "memo: -n 2 --approve was not refused (exit $code)"
}

# Approved and pinned are one thing, a hash in project.yaml: one line changes, and everything else stands.
pins() {
step "a project: approve writes one line, comments and all"
local T=$OUT/pinned before after version file changed
mkdir -p "$T"
cat > "$T/project.yaml" <<YAML
# Paul's note at the top.
version: 2
assets:
  # the hero
  card:   # never change this one
    with: $here/project/draw.py
    kind: picture
    description: A test card.    # a note on it
    aspect_ratio: "16:9"

  flat: {with: $here/project/draw.py, kind: picture, description: A flat one.}
  cleaned:
    with: $here/project/clean.py
    kind: clean
    image: <card>
  poster:
    with: $here/project/draw.py
    kind: picture
    description: <card> as a poster.
# the end
YAML
render "$T" all || check "pins: render all: $(tail -c 300 "$OUT/render.txt")"
render "$T" card -n 2 || check "pins: render card -n 2: $(tail -c 300 "$OUT/render.txt")"
before=$OUT/pins-before.yaml; cp "$T/project.yaml" "$before"
# An edit saved just before the approve stands.
sed -i 's/^# the end$/# the end, as edited just now/' "$T/project.yaml"
cozy project -p "$T" approve card 1 > "$OUT/approve.txt" 2>&1 || check "pins: approve card 1: $(tail -c 300 "$OUT/approve.txt")"
grep -q '# the end, as edited just now' "$T/project.yaml" || check "pins: an edit saved just before approve is gone"
file=$(jq -r '.assets.card.versions[0].files.image' "$T/project.lock"); file=${file#sha256:}
sed -i 's/^# the end, as edited just now$/# the end/' "$T/project.yaml"
diff "$before" "$T/project.yaml" > "$OUT/approve.diff" || true
{ [ "$(grep -c '^[<>]' "$OUT/approve.diff")" = 1 ] && grep -q "^> *image: ${file:0:12}$" "$OUT/approve.diff"; } || check "pins: approve changed more than its one line: $(cat "$OUT/approve.diff")"
J '.assets[] | select(.name == "card") | .approved == 1 and .pinned and .status == "pinned: version 1"' <<< "$(status "$T")" || check "pins: status does not say card is pinned to 1: $(row "$T" card .)"
! jq -e '.assets.card | has("pin")' "$T/project.lock" > /dev/null || check "pins: the lock holds an approval"

step "a project: a pin stops a run, and status says what changed"
sed -i 's/description: A test card\./description: Another card./' "$T/project.yaml"
version=$(made "$T" card)
render "$T" card || check "pins: render a pinned card: $(tail -c 300 "$OUT/render.txt")"
[ "$(made "$T" card)" = "$version" ] || check "pins: a text edit re-rendered a pinned card"
changed=$(row "$T" card .status)
[ "$changed" = '"pinned: version 1; its inputs changed: description"' ] || check "pins: status of an edited pin is $changed"
render "$T" card --new || check "pins: --new on a pinned card: $(tail -c 300 "$OUT/render.txt")"
[ "$(made "$T" card)" = "$((version + 1))" ] || check "pins: --new did not make a take to look at"
[ "$(row "$T" card .approved)" = 1 ] || check "pins: --new moved the pin"
sed -i 's/description: Another card\./description: A test card./' "$T/project.yaml"
[ "$(row "$T" poster .inputs)" = "$(row "$T" poster .inputs)" ] || true

step "a project: unpin removes the line"
cozy project -p "$T" unpin card > "$OUT/unpin.txt" 2>&1 || check "pins: unpin card: $(tail -c 300 "$OUT/unpin.txt")"
diff "$before" "$T/project.yaml" > "$OUT/unpin.diff" || check "pins: unpin did not give back the file: $(cat "$OUT/unpin.diff")"
J '.assets[] | select(.name == "card") | (.pinned | not) and .approved == null' <<< "$(status "$T")" || check "pins: unpinned, status is $(row "$T" card .)"

step "a project: a pin over an input keeps it, and a line of its own"
render "$T" cleaned --new > /dev/null 2>&1 || true
cozy project -p "$T" approve cleaned 1 > "$OUT/approve.txt" 2>&1 || check "pins: approve cleaned 1: $(tail -c 300 "$OUT/approve.txt")"
grep -Eq '^    image: [0-9a-f"]+  # was <card>$' "$T/project.yaml" || check "pins: a pin over <card> is not one line keeping it: $(grep -n 'image:' "$T/project.yaml")"
cozy project -p "$T" unpin cleaned > "$OUT/unpin.txt" 2>&1 || check "pins: unpin cleaned: $(tail -c 300 "$OUT/unpin.txt")"
diff "$before" "$T/project.yaml" > "$OUT/unpin.diff" || check "pins: unpin cleaned did not give back the file: $(cat "$OUT/unpin.diff")"
cozy project -p "$T" approve flat 1 > "$OUT/approve.txt" 2>&1 || check "pins: approve an asset on one line: $(tail -c 300 "$OUT/approve.txt")"
grep -Eq '^  flat: \{image: [0-9a-f"]+, with: .*/draw\.py, kind: picture, description: A flat one\.\}$' "$T/project.yaml" || check "pins: flat is $(grep -n 'flat:' "$T/project.yaml")"
cozy project -p "$T" unpin flat > "$OUT/unpin.txt" 2>&1 || check "pins: unpin flat: $(tail -c 300 "$OUT/unpin.txt")"
diff "$before" "$T/project.yaml" > "$OUT/unpin.diff" || check "pins: unpin flat did not give back the file: $(cat "$OUT/unpin.diff")"

step "a project: re-approving art brings its clean back with no run"
local cleans; cleans=$(made "$T" cleaned)
cozy project -p "$T" approve card 1 > /dev/null 2>&1 || check "pins: approve card 1 again"
render "$T" cleaned || check "pins: render cleaned: $(tail -c 300 "$OUT/render.txt")"
cozy project -p "$T" approve card 2 > /dev/null 2>&1 || check "pins: approve card 2"
render "$T" cleaned || check "pins: render cleaned on 2: $(tail -c 300 "$OUT/render.txt")"
cleans=$(made "$T" cleaned)
cozy project -p "$T" approve card 1 > /dev/null 2>&1 || check "pins: approve card 1 again"
render "$T" cleaned || check "pins: render cleaned on 1 again: $(tail -c 300 "$OUT/render.txt")"
[ "$(made "$T" cleaned)" = "$cleans" ] || check "pins: re-approving art ran its clean: $(made "$T" cleaned), not $cleans"

step "a project: a hash of a file, a constant, a hash of nothing"
local C=$OUT/constants pic; mkdir -p "$C"; still "$C/teal.png" teal
printf 'version: 2\nassets: {}\n' > "$C/project.yaml"
pic=$(prefix "$C" "$C/teal.png")
ffmpeg -v error -y -f lavfi -i sine=frequency=220:duration=1 "$C/voice.wav"
local sound; sound=$(prefix "$C" "$C/voice.wav")
cat > "$C/project.yaml" <<YAML
version: 2
assets:
  teal:
    image: $pic
  singer:
    image: $pic
    audio: $sound
  wrong:
    with: $here/project/draw.py
    kind: picture
    image: cafe0000
    description: It renders without that.
  nothing:
    image: ./path.png
  no package:
    kind: picture
    description: Nothing makes me.
  no kind:
    with: $here/project/draw.py
    description: An image, then.
  no adapter:
    with: $here/project/draw.py
    kind: unheard
    description: Nor me either.
YAML
J '.assets[] | select(.name == "teal") | .pinned and (.with == null) and (.approved == null)' <<< "$(status "$C")" || check "pins: a constant: $(row "$C" teal .)"
J '.assets[] | select(.name == "singer") | (.files | keys) == ["audio", "image"]' <<< "$(status "$C")" || check "pins: a constant of two files: $(row "$C" singer .files)"
J '.warnings | any(contains("no file'"'"'s hash starts with cafe0000")) and any(contains("is not a hash"))' <<< "$(status "$C")" || check "pins: a bad pin is not warned of: $(status "$C" | jq -c .warnings)"
render "$C" teal || check "pins: render a constant: $(tail -c 300 "$OUT/render.txt")"
says pins "is a constant"
code=0; cozy project -p "$C" unpin teal > "$OUT/unpin.txt" 2>&1 || code=$?
[ "$code" != 0 ] || check "pins: unpinning a constant took its one line"
render "$C" wrong || check "pins: wrong's bad pin stopped it: $(tail -c 300 "$OUT/render.txt")"
[ "$(made "$C" wrong)" = 1 ] || check "pins: wrong was not made without its bad pin"

step "a project: an asset with no package, no kind, or no adapter says so, and the rest goes on"
J '.warnings | any(contains("no package has no `with`, and no `defaults.picture.with`"))' <<< "$(status "$C")" || check "pins: an asset with nothing to make is not said: $(status "$C" | jq -c .warnings)"
J '.assets[] | select(.name == "no kind") | .kind == "image"' <<< "$(status "$C")" || check "pins: an operation with no kind is not an image: $(row "$C" 'no kind' .)"
sed -i 's/It renders without that\./Rendered again./' "$C/project.yaml"
render "$C" all || check "pins: render all with what can't be made: $(tail -c 400 "$OUT/render.txt")"
says pins "no adapter: "
says pins "has no adapter for kind unheard: it can't be made"
[ "$(made "$C" wrong)" = 2 ] || check "pins: what could be made was not: wrong has $(made "$C" wrong)"
}

# Orphans (cl-761): what the file no longer names keeps its versions and files until `reclaim --yes`.
orphans() {
step "a project: a renamed asset keeps its versions with rename"
local T=$OUT/orphaned code shared gone_file
mkdir -p "$T"
cat > "$T/project.yaml" <<YAML
version: 2
assets:
  old:
    with: $here/project/draw.py
    kind: picture
    description: The old one.
  reader:
    with: $here/project/draw.py
    kind: picture
    description: <old> on a shelf.
YAML
render "$T" all || check "orphans: render all: $(tail -c 300 "$OUT/render.txt")"
sed -i 's/^  old:$/  new:/; s/<old>/<new>/' "$T/project.yaml"
J '.orphans[0] | .name == "old" and .versions == 1 and .bytes > 0 and .renamed_to == "new"' <<< "$(status "$T")" || check "orphans: status does not list old with its bytes: $(status "$T" | jq -c .orphans)"
cozy project -p "$T" status > "$OUT/orphan.txt" 2>&1 || true
grep -q 'cozy project rename old new' "$OUT/orphan.txt" || check "orphans: status does not suggest the rename"
cozy project -p "$T" rename old new > "$OUT/rename.txt" 2>&1 || check "orphans: rename: $(tail -c 300 "$OUT/rename.txt")"
{ J '.orphans == null or (.orphans | length == 0)' <<< "$(status "$T")" && [ "$(made "$T" new)" = 1 ] && [ "$(row "$T" new .status)" = '"current"' ]; } || check "orphans: renamed, new is $(row "$T" new .)"
case "$(row "$T" reader .status)" in *'out of date'*) ;; *) check "orphans: renaming did not put its reader's text out of date: $(row "$T" reader .status)" ;; esac

step "a project: reclaim lists, and --yes deletes what nothing else names"
render "$T" all || check "orphans: render all renamed"
shared=$(jq -r '.assets.new.versions[0].files.image' "$T/project.lock")
# Another asset's version names the same file: it stays. The rest of the orphan goes.
cat >> "$T/project.yaml" <<YAML
  doomed:
    with: $here/project/draw.py
    kind: picture
    description: Going.
  kept:
    with: $here/project/draw.py
    kind: picture
    description: Stays.
YAML
render "$T" all || check "orphans: render all with doomed"
gone_file=$(jq -r '.assets.doomed.versions[0].files.image' "$T/project.lock")
jq --arg s "$gone_file" '.assets.kept.versions[0].writes = {"a frame": {"image": $s}}' "$T/project.lock" > "$T/l" && mv "$T/l" "$T/project.lock"
sed -i '/^  doomed:$/,/^    description: Going\.$/d' "$T/project.yaml"
cat >> "$T/project.yaml" <<YAML
  second:
    with: $here/project/draw.py
    kind: picture
    description: Second.
YAML
render "$T" second || check "orphans: render second"
local second_file; second_file=$(jq -r '.assets.second.versions[0].files.image' "$T/project.lock")
sed -i '/^  second:$/,/^    description: Second\.$/d' "$T/project.yaml"
cozy --json project -p "$T" reclaim > "$OUT/reclaim.json" 2>&1 || check "orphans: reclaim: $(tail -c 300 "$OUT/reclaim.json")"
J '[.orphans[].name] == ["doomed", "second"] and (.deleted | not)' "$OUT/reclaim.json" || check "orphans: reclaim lists $(jq -c . "$OUT/reclaim.json")"
{ [ "$(made "$T" second)" = 1 ] && [ -f "$T/files/${second_file#sha256:}.png" ]; } || check "orphans: a listing deleted something"
cozy --json project -p "$T" reclaim --yes > "$OUT/reclaim.json" 2>&1 || check "orphans: reclaim --yes: $(tail -c 300 "$OUT/reclaim.json")"
J '.deleted and .files >= 1 and .bytes > 0' "$OUT/reclaim.json" || check "orphans: reclaim --yes says $(jq -c . "$OUT/reclaim.json")"
J '(.assets | has("doomed") or has("second")) | not' "$T/project.lock" || check "orphans: their versions are still in the lock"
[ ! -e "$T/files/${second_file#sha256:}.png" ] || check "orphans: a file nothing names was not deleted"
[ -f "$T/files/${gone_file#sha256:}.png" ] || check "orphans: a file another asset's version names was deleted"
[ -f "$T/files/${shared#sha256:}.png" ] || check "orphans: a kept asset's file was deleted"

step "a project: reclaim refuses while the project renders"
cat >> "$T/project.yaml" <<YAML
  waiting:
    with: $here/project/shots.py
    kind: slow
    seconds: 15
YAML
cozy project -p "$T" render waiting --on local > "$OUT/slow.txt" 2>&1 &
local going=$!
for _ in $(seq 60); do J '.busy' <<< "$(status "$T")" && break; sleep 0.5; done
code=0; cozy project -p "$T" reclaim --yes > "$OUT/busy.txt" 2>&1 || code=$?
{ [ "$code" != 0 ] && grep -q 'rendering' "$OUT/busy.txt"; } || check "orphans: reclaim --yes while rendering (exit $code): $(tail -c 300 "$OUT/busy.txt")"
wait "$going" || check "orphans: the slow render: $(tail -c 300 "$OUT/slow.txt")"
}

# --take and --in: another draw of the same inputs, the take resolved here, the rest the adapter's.
keeps() {
step "a project: --take and --in reach the adapter as op.keep"
local T=$OUT/keeps code look
mkdir -p "$T"; plate "$T/look.mp4" testsrc2 440 800
printf 'version: 2\nassets: {}\n' > "$T/project.yaml"
look=$(prefix "$T" "$T/look.mp4")
cat > "$T/project.yaml" <<YAML
version: 2
assets:
  look:
    video: $look
  clash:
    with: $here/project/shots.py
    kind: shot
    summary: Two lights meet on <look>.
YAML
given() { cat "$T/$(jq -r --arg s "${1:-clash}" '.files[.assets[$s].versions[-1].files.given].path' "$T/project.lock")"; }
render "$T" clash || check "keeps: render: $(tail -c 300 "$OUT/render.txt")"
J '.keep == null and (.prompt | contains("clash") and contains("look=")) and .seed != null' <<< "$(given)" || check "keeps: the adapter's words did not reach the function: $(given)"
code=0; render "$T" clash --take 9 || code=$?
{ [ "$code" = 2 ] && says keeps "has no take 9"; } || check "keeps: --take 9 exit $code: $(tail -c 300 "$OUT/render.txt")"
render "$T" clash --take 1 --in 3 || check "keeps: --take 1 --in 3: $(tail -c 300 "$OUT/render.txt")"
J '.keep == 3' <<< "$(given)" || check "keeps: --in 3 is not op.keep: $(given)"
J '.assets.clash.versions[-1] | .kept == {"take": 1, "until_s": 3.04} and (.seed != null)' "$T/project.lock" || check "keeps: the version does not record what it keeps: $(jq -c '.assets.clash.versions[-1].kept' "$T/project.lock")"
says keeps "take 2 is take 1 to 3.04 s"
render "$T" clash --in 2 || check "keeps: --in 2: $(tail -c 300 "$OUT/render.txt")"
J '.assets.clash.versions[-1].kept.take == 2' "$T/project.lock" || check "keeps: --in alone keeps the take in use: $(jq -c '.assets.clash.versions[-1].kept' "$T/project.lock")"
render "$T" all --in 2 || check "keeps: all --in: $(tail -c 300 "$OUT/render.txt")"
says keeps "--take and --in keep the start of one asset"
}

# A scene a beat writes: a screenshot at `at` into the beat. What the shot reads after that beat goes to its function, in its key; what
# it doesn't is a free frame, outside its key. A mention names exactly the asset it names.
world_states() {
step "a project: scenes written by beats"
local T=$OUT/series look take frames
mkdir -p "$T"; plate "$T/look.mp4" testsrc2 440 800 8
printf 'version: 2\nassets: {}\n' > "$T/project.yaml"
look=$(prefix "$T" "$T/look.mp4")
cat > "$T/project.yaml" <<YAML
version: 2
assets:
  look:
    video: $look
  coast:
    with: $here/project/draw.py
    kind: environment
    description: A coast.
  first:
    with: $here/project/shots.py
    kind: shot
    summary: Dusk on <look> and <coast>.
  long take:
    with: $here/project/shots.py
    kind: shot
    summary: Walking on <look>.
    beats:
      - duration: 4
        description: She runs on <coast>.
        writes:
          - scene: coast.1
            at: 1
          - scene: coast.2
            at: 3
      - duration: 4
        description: She stands on <coast.1>, then <coast.2>.
        writes:
          - scene: harbor.1
            at: 1
  next:
    with: $here/project/shots.py
    kind: shot
    summary: Dawn on <look> and <coast.2>, by <harbor.1>.
  poster:
    with: $here/project/draw.py
    kind: picture
    description: <coast.1> as a poster.
YAML
J '[.assets[] | select(.name == "coast.1" or .name == "coast.2") | .kind] == ["scene", "scene"]' <<< "$(status "$T")" || check "series: revisions are not assets: $(status "$T" | jq -c '[.assets[].name]')"
render "$T" all || check "series: render all: $(tail -c 400 "$OUT/render.txt")"
given() { cat "$T/$(jq -r --arg s "$1" '.files[.assets[$s].versions[-1].files.given].path' "$T/project.lock")"; }
J '.writes == ["coast.1", "coast.2"] and .at == [1, 3]' <<< "$(given 'long take')" || check "series: long take's writes are not its beats', on the shot's clock: $(given 'long take')"
J '.prompt | contains("coast.2=") and contains("look=")' <<< "$(given next)" || check "series: next does not read <coast.2>: $(given next)"
take=$(jq -c '.assets["long take"].versions[-1].writes | keys' "$T/project.lock")
[ "$take" = '["coast.1","coast.2"]' ] || check "series: long take's take holds $take"
[ "$(made "$T" coast.1)" = null ] || [ "$(made "$T" coast.1)" = 0 ] || check "series: a sent revision has versions of its own"
[ "$(row "$T" coast.1 .status)" = '"written by long take 1"' ] || check "series: coast.1 is $(row "$T" coast.1 .status)"
J --arg want "$(jq -r '.assets["long take"].versions[-1].writes["coast.2"].image' "$T/project.lock")" '.assets["next"].versions[-1].reads[] | select(.name == "coast.2") | .files.image == $want' "$T/project.lock" \
  || check "series: next's <coast.2> is not what long take wrote: $(jq -c '.assets["next"].versions[-1].reads' "$T/project.lock")"
J --arg want "$(jq -r '.assets["long take"].versions[-1].writes["coast.1"].image' "$T/project.lock")" '.assets.poster.versions[-1].reads[] | select(.name == "coast.1") | .files.image == $want' "$T/project.lock" \
  || check "series: poster's <coast.1> is not that revision"
J '.assets["first"].versions[-1].reads | map(.name) | index("coast")' "$T/project.lock" || check "series: first does not read coast"
[ "$(jq -r '.assets["first"].versions[-1].reads[] | select(.name == "coast") | .files.image' "$T/project.lock")" = "$(jq -r '.assets.coast.versions[-1].files.image' "$T/project.lock")" ] || check "series: first's <coast> is not coast itself"

step "a project: a free frame, moved, re-renders nothing"
J '.assets["harbor.1"].versions[-1] | .made_by.with == "cozy/frame" and (.files.image != null)' "$T/project.lock" || check "series: harbor.1 is not a free frame: $(jq -c '.assets["harbor.1"].versions[-1].made_by' "$T/project.lock")"
frames=$(runs "$T")
local was=$(row "$T" 'long take' .inputs)
sed -i '/scene: harbor.1/{n;s/at: 1/at: 2/}' "$T/project.yaml"
[ "$(row "$T" 'long take' .inputs)" = "$was" ] || check "series: moving a free write moved its shot's key"
render "$T" 'harbor.1' || check "series: render a free frame: $(tail -c 300 "$OUT/render.txt")"
says series "made here, free"
J '.assets["harbor.1"].versions | length == 1' "$T/project.lock" || check "series: a tool kept its old version: $(jq -c '.assets["harbor.1"].versions | length' "$T/project.lock")"
J '.assets["harbor.1"].versions[-1].digests.at != null' "$T/project.lock" || check "series: a frame's key does not hold its time"
[ "$(runs "$T")" = "$frames" ] || check "series: moving a free frame ran something"

step "a project: pinning a sent write puts its shot out of date; a write past its beat warns"
local pic; still "$T/pic.png" navy; pic=$(prefix "$T" "$T/pic.png")
sed -i "/^  poster:\$/i\\  coast.2:\\n    image: $pic" "$T/project.yaml"
case "$(row "$T" 'long take' .status)" in *'out of date'*) ;; *) check "series: a pinned sent write did not put its shot out of date: $(row "$T" 'long take' .status)" ;; esac
J '.assets[] | select(.name == "coast.2") | .pinned' <<< "$(status "$T")" || check "series: coast.2 is not pinned"
sed -i '/^  coast.2:$/,+1d' "$T/project.yaml"
sed -i '0,/at: 1$/s//at: 40/' "$T/project.yaml"
J '.warnings | any(contains("at 40 s into a beat 4 s long"))' <<< "$(status "$T")" || check "series: a write past its beat did not warn: $(status "$T" | jq -c .warnings)"
sed -i '0,/scene: harbor.1/s//&\n            description: Not a field of a write./' "$T/project.yaml"
J '.warnings | any(contains("write of harbor.1: cozy reads no field description"))' <<< "$(status "$T")" || check "series: another field on a write did not warn: $(status "$T" | jq -c .warnings)"
sed -i 's/^    summary: Walking on <look>\.$/    writes: {coast.9: 1}\n&/' "$T/project.yaml"
J '.warnings | any(contains("go on the beat"))' <<< "$(status "$T")" || check "series: writes on an asset did not warn: $(status "$T" | jq -c .warnings)"

step "a project: a package that takes no writes: frames from the landed video"
local B=$OUT/bare; mkdir -p "$B"; plate "$B/look.mp4" testsrc2 440 800
printf 'version: 2\nassets: {}\n' > "$B/project.yaml"
look=$(prefix "$B" "$B/look.mp4")
cat > "$B/project.yaml" <<YAML
version: 2
assets:
  look:
    video: $look
  moves:
    with: $here/project/shots.py
    kind: bare
    plate: <look>
    summary: It walks.
    beats:
      - duration: 4
        description: The wall turns.
        writes:
          - scene: wall.1
            at: 3
      - duration: 2
        description: It stands by <wall.1>.
YAML
render "$B" moves || check "bare: render: $(tail -c 400 "$OUT/render.txt")"
says bare "takes no writes"
J '.assets.moves.versions[-1].writes["wall.1"].image != null' "$B/project.lock" || check "bare: no frame was cut for wall.1: $(jq -c '.assets.moves.versions[-1].writes' "$B/project.lock")"
}

# An adapter's no run, its warnings, a typed `fields`, and a list of calls as one version.
adapters() {
step "a project: an adapter that answers no run, a typed one, and one with two calls"
local T=$OUT/adapters look
mkdir -p "$T"; plate "$T/look.mp4" testsrc2 440 800
printf 'version: 2\nassets: {}\n' > "$T/project.yaml"
look=$(prefix "$T" "$T/look.mp4")
cat > "$T/project.yaml" <<YAML
version: 2
assets:
  look:
    video: $look
  same:
    with: $here/project/shots.py
    kind: shot
    no_run: true
    summary: Already <look>.
  checked:
    with: $here/project/shots.py
    kind: typed
    summary: Some words.
    no_such_field: Nothing reads this.
  both:
    with: $here/project/shots.py
    kind: two calls
    summary: One then the other.
YAML
render "$T" same || check "adapters: no run: $(tail -c 400 "$OUT/render.txt")"
J '.assets.same.versions[0] | (.runs // [] | length == 0) and .files.video != null' "$T/project.lock" || check "adapters: None is not what it reads, with no run: $(jq -c '.assets.same.versions' "$T/project.lock")"
[ "$(jq -r .assets.same.versions[0].files.video "$T/project.lock")" = "$(row "$T" look '.files.video.sha256' | tr -d '"')" ] || check "adapters: None's files are not what it reads"
render "$T" same --new || check "adapters: no run, --new"
says adapters "has nothing to make"
[ "$(made "$T" same)" = 1 ] || check "adapters: a second version of what is its reads"
render "$T" checked || check "adapters: typed: $(tail -c 400 "$OUT/render.txt")"
says adapters "reads no field no_such_field"
render "$T" both || check "adapters: two calls: $(tail -c 400 "$OUT/render.txt")"
J '.assets.both.versions | length == 1 and (.[0] | (.runs | length == 2) and (.invocations | length == 2) and .files.first != null and .files.second != null)' "$T/project.lock" || check "adapters: two calls are not one version: $(jq -c '.assets.both.versions' "$T/project.lock")"
[ "$(cat "$T/$(jq -r '.files[.assets.both.versions[0].files.second].path' "$T/project.lock")")" = "$(jq -r '.assets.both.versions[0].files.first' "$T/project.lock")" ] || check "adapters: the second call was not given the first's output"
}

# Runs that fail on purpose, on a machine with no history to keep clean: never alone.
failures() {
step "a project: a failed run is no version, and its seed is kept for the next try"
local T=$OUT/failing look seed
mkdir -p "$T"; plate "$T/look.mp4" testsrc2 440 800
printf 'version: 2\nassets: {}\n' > "$T/project.yaml"
look=$(prefix "$T" "$T/look.mp4")
cat > "$T/project.yaml" <<YAML
version: 2
assets:
  look:
    video: $look
  no:
    with: $here/project/shots.py
    kind: shot
    refuse: Not this one.
    summary: <look>.
  reader:
    with: $here/project/shots.py
    kind: shot
    summary: <no> and <look>.
YAML
code=0; render "$T" reader || code=$?
{ [ "$code" != 0 ] && says failures "Not this one."; } || check "failures: Refuse (exit $code): $(tail -c 400 "$OUT/render.txt")"
[ "$(made "$T" no)" = null ] || [ "$(made "$T" no)" = 0 ] || check "failures: a refusal made a version"
says failures "reader cannot render: what it reads was not made"
[ -d "${PACKAGES:-}/minimax-h3" ] || return 0
step "a project: H3's adapter refuses to keep another package's take"
local H=$OUT/refused; mkdir -p "$H"; plate "$H/look.mp4" testsrc2 440 800
printf 'version: 2\nassets: {}\n' > "$H/project.yaml"
look=$(prefix "$H" "$H/look.mp4")
cat > "$H/project.yaml" <<YAML
version: 2
assets:
  look:
    video: $look
  clash:
    with: $here/project/shots.py
    kind: shot
    summary: <look>.
    beats:
      - duration: 6
YAML
render "$H" clash || check "refused: the first take: $(tail -c 300 "$OUT/render.txt")"
sed -i "s#with: .*shots.py#with: $PACKAGES/minimax-h3#" "$H/project.yaml"
code=0; render "$H" clash --take 1 --in 5 || code=$?
{ [ "$code" != 0 ] && says refused "has no latents this package can read"; } || check "refused: another package's take (exit $code): $(tail -c 300 "$OUT/render.txt")"
}

# The two kinds of pieces, in frames; two cuts side by side, each its own film and timeline; an edit recut on approve with no machine;
# a take at another rate; takes at a model's rounded size joined unencoded.
cuts() {
step "a project: two cuts side by side, trimmed and whole"
local P=$1 ID=$2 T=$OUT/cuts A B R n CACHE=$HOME/.cache/cozy/cuts
local FR=(uv run -q --no-project --with numpy python -I "$here/project/frames.py" "$(command -v ffmpeg)")
local OT=(uv run -q --no-project --with opentimelineio==0.17.0 python -I "$here/project/otio_check.py")
mkdir -p "$T/plates"
plate "$T/plates/a.mp4" testsrc2 440 1800 8; plate "$T/plates/b.mp4" testsrc 550 900 8; plate "$T/plates/r.mp4" testsrc 550 900 8 854x480 25
printf 'version: 2\nassets: {}\n' > "$T/project.yaml"
A=$(prefix "$T" "$T/plates/a.mp4") B=$(prefix "$T" "$T/plates/b.mp4") R=$(prefix "$T" "$T/plates/r.mp4")
local SA SB SR; SA=$(sha "$T/plates/a.mp4") SB=$(sha "$T/plates/b.mp4") SR=$(sha "$T/plates/r.mp4")
cat > "$T/project.yaml" <<YAML
version: 2
assets:
  a:
    video: $A
  b:
    video: $B
  edit:
    with: cozy
    kind: edit
    fps: 24
    pieces:
      - {video: <a>, start: 7, end: 151}
      - {video: <b>, start: 13, end: 140}
      - {video: <a>, start: 48, end: 96}
      - {video: <b>, start: 150, end: 161}
  trailer:
    with: cozy
    kind: edit
    fps: 24
    pieces:
      - <b>
      - <a>
YAML
J '.warnings == null or (.warnings | length == 0)' <<< "$(status "$T")" || check "cuts: warnings: $(status "$T" | jq -c .warnings)"
render "$T" all || check "cuts: render all: $(tail -c 400 "$OUT/render.txt")"
n=$(ffprobe -v error -count_frames -select_streams v:0 -show_entries stream=nb_read_frames,r_frame_rate -of csv=p=0 "$T/edit.mp4" 2>/dev/null) || n=
[ "$n" = "24/1,330" ] || check "cuts: edit.mp4 is $n, not 330 frames at 24/1 ((151-7)+(140-13)+(96-48)+(161-150))"
n=$(ffprobe -v error -count_frames -select_streams v:0 -show_entries stream=nb_read_frames,r_frame_rate -of csv=p=0 "$T/trailer.mp4" 2>/dev/null) || n=
[ "$n" = "24/1,384" ] || check "cuts: trailer.mp4 is $n, not 384 frames, two whole takes"
{ [ -s "$T/edit.otio" ] && [ -s "$T/trailer.otio" ]; } || check "cuts: each cut does not write its own .otio"
says cuts "edit: "
says cuts "trailer: "
"${FR[@]}" "$T/edit.mp4" 24 "$T/plates/a.mp4:7:151" "$T/plates/b.mp4:13:140" "$T/plates/a.mp4:48:96" "$T/plates/b.mp4:150:161" > "$OUT/frames-1.txt" 2>&1 || true
tail -1 "$OUT/frames-1.txt" | grep -q 'frames PASS' || check "cuts: not every frame its source frame: $(grep -E 'FAIL|md5-equal' "$OUT/frames-1.txt" | head -3 | paste -sd' ')"
"${OT[@]}" "$T/edit.otio" 24 "a:7:151:files/$SA.mp4" "b:13:140:files/$SB.mp4" "a:48:96:files/$SA.mp4" "b:150:161:files/$SB.mp4" > "$OUT/otio-1.txt" 2>&1 || true
tail -1 "$OUT/otio-1.txt" | grep -q 'otio PASS' || check "cuts: edit.otio is not the edit: $(grep FAIL "$OUT/otio-1.txt" | head -3 | paste -sd' ')"
J --arg a "sha256:$SA" '.files[$a] | .width == 854 and .height == 480 and .fps == 24 and .frames == 192' "$T/project.lock" || check "cuts: project.lock does not hold a's measures: $(jq -c --arg a "sha256:$SA" '.files[$a]' "$T/project.lock")"
J '[.cuts[].frames] == [330, 384] and ([.cuts[].cut] == ["edit.mp4", "trailer.mp4"])' <<< "$(status "$T")" || check "cuts: status: $(status "$T" | jq -c .cuts)"

step "a project: same takes, not cut again; the key follows the pieces"
local first made_at; first=$(sha "$T/edit.mp4"); made_at=$(stat -c %Y%N "$T/edit.mp4" 2>/dev/null || stat -c %Y "$T/edit.mp4")
render "$T" all || check "cuts: render all again: $(tail -c 300 "$OUT/render.txt")"
says cuts "(unchanged)"
[ "$(stat -c %Y%N "$T/edit.mp4" 2>/dev/null || stat -c %Y "$T/edit.mp4")" = "$made_at" ] || check "cuts: same takes, cut again"
J '.assets.edit.versions | length == 1' "$T/project.lock" || check "cuts: a cut kept its old version"

step "a project: a take at 25 fps, retimed; a take that isn't a piece any more"
sed -i "s/^    video: $B\$/    video: $R/" "$T/project.yaml"
render "$T" all || check "cuts: with r: $(tail -c 300 "$OUT/render.txt")"
n=$(ffprobe -v error -count_frames -select_streams v:0 -show_entries stream=nb_read_frames,r_frame_rate -of csv=p=0 "$T/edit.mp4" 2>/dev/null) || n=
[ "$n" = "24/1,330" ] || check "cuts: the retimed edit.mp4 is $n, not 330 frames at 24/1"
says cuts "it retimes b (25 fps) to 24 fps"
"${FR[@]}" "$T/edit.mp4" 24 "$T/plates/a.mp4:7:151" "$T/plates/r.mp4:13:140" "$T/plates/a.mp4:48:96" "$T/plates/r.mp4:150:161" > "$OUT/frames-2.txt" 2>&1 || true
tail -1 "$OUT/frames-2.txt" | grep -q 'frames PASS' || check "cuts: the retimed cut's frames are not their sources': $(grep FAIL "$OUT/frames-2.txt" | head -3 | paste -sd' ')"
sed -i "s/^    video: $R\$/    video: $B/" "$T/project.yaml"
render "$T" all || check "cuts: b back: $(tail -c 300 "$OUT/render.txt")"
[ "$(sha "$T/edit.mp4")" = "$first" ] || check "cuts: the first edit again is another file"

step "a project: approve recuts here, and a tool reads a newest out of date, and says so"
local S=$OUT/recut; mkdir -p "$S"; plate "$S/look.mp4" testsrc2 440 800 3
printf 'version: 2\nassets: {}\n' > "$S/project.yaml"
A=$(prefix "$S" "$S/look.mp4")
cat > "$S/project.yaml" <<YAML
version: 2
assets:
  look:
    video: $A
  shot:
    with: $here/project/shots.py
    kind: bare
    plate: <look>
    summary: One.
  edit:
    with: cozy
    kind: edit
    pieces:
      - <shot>
YAML
render "$S" all || check "recut: render all: $(tail -c 300 "$OUT/render.txt")"
[ -s "$S/edit.mp4" ] || check "recut: no edit.mp4"
sed -i 's/summary: One\./summary: Two./; s/^    pieces:$/    fps: 30\n&/' "$S/project.yaml"
render "$S" look || check "recut: render look"
says recut "which is out of date"
cozy project -p "$S" approve shot 1 > "$OUT/approve.txt" 2>&1 || check "recut: approve: $(tail -c 300 "$OUT/approve.txt")"
[ -s "$S/edit.mp4" ] || check "recut: approve lost edit.mp4"
rm "$S/edit.mp4"
render "$S" look || check "recut: render look again"
[ -s "$S/edit.mp4" ] || check "recut: a current cut's film was not put back at the root"

step "a project: takes at a model's rounded size join unencoded, and a plate of another size fills it"
local W=$OUT/rounded; mkdir -p "$W"
wide() { ffmpeg -v error -y -f lavfi -i "$2=size=$3:rate=24:duration=3" -c:v libx264 -preset veryfast -g 24 -keyint_min 24 -sc_threshold 0 -pix_fmt yuv420p "$1"; }
wide "$W/a.mp4" testsrc2 1344x768; wide "$W/b.mp4" testsrc 1344x768; wide "$W/c.mp4" rgbtestsrc 854x480
printf 'version: 2\nassets: {}\n' > "$W/project.yaml"
A=$(prefix "$W" "$W/a.mp4") B=$(prefix "$W" "$W/b.mp4") R=$(prefix "$W" "$W/c.mp4")
cat > "$W/project.yaml" <<YAML
version: 2
assets:
  a:
    video: $A
  b:
    video: $B
  edit:
    with: cozy
    kind: edit
    aspect_ratio: "16:9"
    pieces:
      - {video: <a>, start: 0, end: 48}
      - {video: <b>, start: 24, end: 48}
YAML
render "$W" all || check "rounded: render all: $(tail -c 300 "$OUT/render.txt")"
size() { ffprobe -v error -select_streams v:0 -show_entries stream=width,height -of csv=p=0 "$1"; }
[ "$(size "$W/edit.mp4")" = 1344,768 ] || check "rounded: edit.mp4 is $(size "$W/edit.mp4"), not its takes' 1344,768"
md5s() { ffmpeg -v error -i "$1" -map 0:v -f framemd5 - | grep -v '^#' | awk '{print $NF}'; }
[ "$( (md5s "$W/a.mp4" | sed -n 1,48p; md5s "$W/b.mp4" | sed -n 25,48p) | md5sum)" = "$(md5s "$W/edit.mp4" | md5sum)" ] || check "rounded: edit.mp4's frames are not its takes'"
! ls -1 "$CACHE" | grep -q "^$(sha "$W/a.mp4" | cut -c1-16)-" || check "rounded: an edge of a was encoded"
sed -i "s/^    video: $B\$/    video: $R/" "$W/project.yaml"
render "$W" all || check "rounded: with an 854x480 plate: $(tail -c 300 "$OUT/render.txt")"
[ "$(size "$W/edit.mp4")" = 1344,768 ] || check "rounded: a plate of another size: edit.mp4 is $(size "$W/edit.mp4"), not 1344,768"

# For the HTTP surface: a cut in the first project of the suite.
cp "$T/plates/a.mp4" "$P/a.mp4"
A=$(prefix "$P" "$P/a.mp4")
cat > "$P/project.yaml" <<YAML
version: 2
assets:
  a:
    video: $A
  edit:
    with: cozy
    kind: edit
    pieces:
      - <a>
YAML
render "$P" all || check "cuts: the HTTP surface's project: $(tail -c 300 "$OUT/render.txt")"
}

# `cozy project import` by path and URL, a real file either way, what to paste, with no machine.
imports() {
step "a project: import by path and by URL"
local U=$OUT/imported code hash value
local seed=https://github.com/cozy-creator/cozy-creator/raw/master/release-gate/ref/seed.jpg
local lost=https://raw.githubusercontent.com/cozy-creator/cozy-creator/master/release-gate/ref/no-such.jpg
mkdir -p "$U"; printf 'version: 2\nassets: {}\n' > "$U/project.yaml"
cozy project -p "$U" import "$seed" > "$OUT/import-url.txt" 2> "$OUT/import-url.err" || check "import: a URL: $(tail -c 300 "$OUT/import-url.err")"
value=$(sed -n '2s/^image: //p' "$OUT/import-url.txt")
hash=$(sha256sum "$U"/files/*.jpg | cut -c1-64)
{ [ "${#value}" = 12 ] && [ "${hash:0:12}" = "$value" ] && [ ! -L "$U/files/$hash.jpg" ] && [ "$(sed -n 1p "$OUT/import-url.txt")" = "imported $value" ]; } \
  || check "import: a URL did not print its 12 characters and land as a real file: $(cat "$OUT/import-url.txt"); $(ls -l "$U/files" 2>&1 | head -3)"
! grep -q github "$U/project.lock" || check "import: the lock names the URL"
code=0; cozy project -p "$U" import "$lost" > "$OUT/import-lost.txt" 2>&1 || code=$?
[ "$code" != 0 ] || check "import: a URL that is not there was accepted"
printf 'not a picture\n' > "$U/note.txt"
code=0; cozy project -p "$U" import "$U/note.txt" > "$OUT/import-txt.txt" 2>&1 || code=$?
[ "$code" != 0 ] || check "import: a text file was accepted"
still "$U/kyogre.png" red
cozy project -p "$U" import "$U/kyogre.png" --character > "$OUT/import-char.txt" 2>&1 || check "import: --character: $(tail -c 300 "$OUT/import-char.txt")"
{ grep -q '^kyogre source:$' "$OUT/import-char.txt" && grep -q '^  with: birefnet$' "$OUT/import-char.txt" && grep -q '^  image: <kyogre source>$' "$OUT/import-char.txt"; } \
  || check "import: --character does not print both assets: $(cat "$OUT/import-char.txt")"
cozy project -p "$U" import "$U/kyogre.png" > "$OUT/import-again.txt" 2>&1 || check "import: again: $(tail -c 300 "$OUT/import-again.txt")"
grep -q '^already imported ' "$OUT/import-again.txt" || check "import: a second import does not say so: $(cat "$OUT/import-again.txt")"
}

# Range, HEAD, ETag and 416 on a cut and an asset; Calcifer's Host, Origin and bearer rules.
# cl-728: takes of a model's rounded size (H3's 1344x768, 1.6% from 16:9) with no `resolution:`: the edit is their size,
# their frames unencoded (no edge piece, each frame its take's) and the same file cut twice; a plate of another size still
# fills it.
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
png=$(jq -r '.files | to_entries[0].value.path' "$P/project.lock")
api -D "$OUT/asset.hdr" -o "$OUT/asset.bin" "http://$ADDR/v1/projects/$ID/$png" || true
{ head -1 "$OUT/asset.hdr" | grep -q ' 200' && cmp -s "$OUT/asset.bin" "$P/$png" && grep -qi "etag: \"$(basename "$png" .mp4)\"" "$OUT/asset.hdr"; } \
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
# The re-cut the page follows: the edit a second shorter.
sed 's/      - <a>/      - {video: <a>, start: 0, end: 120}/' "$P/project.yaml" > "$OUT/recut.yaml"
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

# The projects this run opened, out of the owner's list; their folders are gone with $OUT.
forget_all() {
  local id
  for id in $(cozy --json project ls 2> /dev/null | jq -r --arg o "$OUT" '.projects[] | select(.path | startswith($o)) | .id'); do
    api -o /dev/null -X DELETE "http://$ADDR/v1/projects/$id" || true
  done
}

# Alone: this computer's cozy, as it is, in the default home.
if [ "${BASH_SOURCE[0]}" = "$0" ]; then
  . "$here/../lib.sh"
  parse "$@"
  ALONE=1
  hub_checked() { :; }
  projects
  finish project
fi
