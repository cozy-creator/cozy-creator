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
for name in kyogre kyogre_sheet groudon coast coast_plate awaken erupt clash edit; do grep -q "$name" "$OUT/project-new-status.txt" || check "a new project's status names no $name"; done
mkdir -p "$P/sub"
(cd "$P/sub" && cozy --json project status) > "$OUT/project-status.json" 2> /dev/null || check "status from a subfolder failed"
(cd / && cozy --json project -p "$P" status) > /dev/null 2>&1 || check "status by -p from / failed"
ID=$(jq -r '.id // empty' "$OUT/project-status.json")
[ -n "$ID" ] || check "project status --json names no id"

cut_of_takes
said_once
film_is_its_file
short_film_heals
cut_versions
by_asset
hard_cut
memo
pins
project_runs
entities
orphans
joining
queueing
every_render
keeps
written
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

# Several runs at once, off a terminal and on one: every line names its asset, a preview is no take, a warning says whose it is, once,
# and what `--json` gives is as it was.
by_asset() {
step "a project: runs at once, each line starting with its asset"
local T=$OUT/by-asset look name
mkdir -p "$T"; plate "$T/look.mp4" testsrc2 440 800 2
printf 'version: 2\nassets: {}\n' > "$T/project.yaml"
look=$(prefix "$T" "$T/look.mp4")
cat > "$T/project.yaml" <<YAML
version: 2
assets:
  look:
    video: $look
  amy:
    kind: image
    render: {with: $here/project/draw.py}
    description: A picture.
  sophia:
    kind: image
    render: {with: $here/project/draw.py}
    description: Another.
  long:
    kind: video
    render: {with: $here/project/shots.py}
    summary: <look> and <amy>.
    segments: 2
    each: 3
  short:
    kind: video
    render: {with: $here/project/shots.py}
    summary: <look> and <sophia>.
    segments: 1
    each: 2
  cut:
    kind: video
    render: {with: cozy}
    pieces:
      - <long>
      - <short>
YAML
render "$T" all || check "by asset: render all: $(tail -c 400 "$OUT/render.txt")"
cp "$OUT/render.txt" "$OUT/by-asset.txt"
for name in amy sophia long short cut; do
  grep -qE "^$name: (take|version) [0-9]+" "$OUT/by-asset.txt" || check "by asset: no line says $name landed: $(cat "$OUT/by-asset.txt")"
done
says "by asset" "long: shot 1/2 : segment 1/2 ready: files/"
says "by asset" "long: shot 1/2 : segment 2/2, denoise"
says "by asset" "short: shot 2/2 : segment 1/1, denoise"
# Nothing of a run's own, and every line its asset's, the project's first line and the last aside.
! grep -qE '^run [0-9]+ · |^  on |^  inputs: |^  call |^(image|video)  ' "$OUT/by-asset.txt" || check "by asset: a run's own lines: $(cat "$OUT/by-asset.txt")"
grep -vE '^(amy|sophia|long|short|cut): |^by-asset( on [a-z]+)?: following|^rendered ' "$OUT/by-asset.txt" > "$OUT/by-asset-other.txt" || true
[ ! -s "$OUT/by-asset-other.txt" ] || check "by asset: a line with no asset: $(cat "$OUT/by-asset-other.txt")"
[ "$(grep -c 'lens is dusty' "$OUT/by-asset.txt")" = 1 ] || check "by asset: a warning not said once, or not at all: $(grep 'lens is dusty' "$OUT/by-asset.txt")"
grep -qE '^(long|short): warning: the fixture' "$OUT/by-asset.txt" || check "by asset: a warning names no asset: $(grep 'lens is dusty' "$OUT/by-asset.txt")"
J '.board.jobs | map(select(.asset == "long")) | .[0] | .state == "done" and .noun == "take" and .number == 1 and (.path | startswith("files/"))' <<< "$(status "$T")" || check "by asset: status's board: $(status "$T" | jq -c .board)"
# Shots are what makes a video by a package: a still and a cut count for nothing.
J '.board.shots == 2 and ([.board.jobs[] | {(.asset): .shot}] | add) == {amy: null, sophia: null, long: 1, short: 2, cut: null}' <<< "$(status "$T")" || check "by asset: its shots: $(status "$T" | jq -c '{shots: .board.shots, jobs: [.board.jobs[] | {asset, shot}]}')"

step "a project: --json is each run's own events"
cozy --json project -p "$T" render short --on local > "$OUT/by-asset.ndjson" 2> "$OUT/by-asset-json.err" || check "by asset: --json: $(tail -c 300 "$OUT/by-asset-json.err")"
for type in run.completed output_item.landed run.log; do
  jq -se --arg t "$type" 'any(.type == $t)' "$OUT/by-asset.ndjson" > /dev/null || check "by asset: --json has no $type: $(cut -c1-120 "$OUT/by-asset.ndjson" | head -20)"
done

step "a project: --plain on a terminal is one line per event"
script -qfec "stty rows 24 cols 120; '$COZY' project -p '$T' render sophia --plain --on local" "$OUT/by-asset-plain.txt" > /dev/null 2>&1 || check "by asset: --plain: $(tail -c 400 "$OUT/by-asset-plain.txt")"
{ grep -aqE '^sophia: version [0-9]+' "$OUT/by-asset-plain.txt" && ! grep -aq $'\x1b\\[?1049h' "$OUT/by-asset-plain.txt"; } || check "by asset: --plain drew a view: $(cat -v "$OUT/by-asset-plain.txt" | tail -c 400)"

step "a project: on a terminal, the live view and then a block in the scrollback"
FORCE_HYPERLINK=1 script -qfec "stty rows 24 cols 120; '$COZY' project -p '$T' render all --on local" "$OUT/by-asset-tty.txt" > /dev/null 2>&1 || check "by asset: on a terminal: $(tail -c 400 "$OUT/by-asset-tty.txt")"
sed -e 's/\x1b\]8;;[^\x1b]*\x1b\\//g' -e 's/\r$//' "$OUT/by-asset-tty.txt" | sed -n '/\x1b\[?1049l/,$p' > "$OUT/by-asset-block.txt"
grep -aqE '^long +take [0-9]+ +[0-9]+s +files/[0-9a-f]{8}\.mp4$' "$OUT/by-asset-block.txt" || check "by asset: the block has no long, or no short link: $(cat -v "$OUT/by-asset-block.txt")"
grep -aqE '^cut +(take|version) [0-9]+ +.*cut\.mp4, [0-9.]+ s' "$OUT/by-asset-block.txt" || check "by asset: the block has no cut with its length: $(cat -v "$OUT/by-asset-block.txt")"
grep -aqE '^rendered 2 takes and 3 versions|^rendered .* in ' "$OUT/by-asset-block.txt" || check "by asset: the block has no total: $(cat -v "$OUT/by-asset-block.txt")"
grep -aqF "$look" "$OUT/by-asset-tty.txt" && check "by asset: the input's name in the view"
grep -aq 'running' "$OUT/by-asset-tty.txt" || check "by asset: the live view never showed a job running"
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
  image:
    with: $here/project/draw.py
    style: Plain.
assets:
  card:
    kind: image
    description: A test card.
  poster:
    kind: image
    description: <card> as a poster.
  copied:
    render: {with: $here/project/copied.py}
    kind: image
    image: <card>
YAML
render "$T" all || check "memo: render all: $(tail -c 400 "$OUT/render.txt")"
[ "$(jq -c '[.assets.card, .assets.poster, .assets.copied | .versions | length]' "$T/project.lock")" = "[1,1,1]" ] || check "memo: not one version each: $(jq -c '.assets | map_values(.versions | length)' "$T/project.lock")"
J '.assets.card.versions[0].seed != null and .assets.copied.versions[0].seed == null' "$T/project.lock" || check "memo: a seed was not drawn for draw, or was for clean, which takes none"
J '.format == 2 and (.files | to_entries | all(.value.path | test("^files/[0-9a-f]{64}")))' "$T/project.lock" || check "memo: the lock is not format 2 with files under files/"
J '.assets.poster.versions[0].reads[0].name == "card"' "$T/project.lock" || check "memo: poster's version does not record reading card"
first=$(runs "$T")

step "a project: nothing to do, and what changes no key"
render "$T" all --missing || check "memo: render all again: $(tail -c 300 "$OUT/render.txt")"
[ "$(runs "$T")" = "$first" ] || check "memo: a second render all --missing ran something"
was=$(row "$T" card .inputs)
{ printf '# a note\n'; cat "$T/project.yaml"; } > "$T/y" && mv "$T/y" "$T/project.yaml"
sed -i 's/description: A test card\./description: "  A test card.  "/' "$T/project.yaml"
sed -i 's/^    kind: image$/    kind: "image"/' "$T/project.yaml"
[ "$(row "$T" card .inputs)" = "$was" ] || check "memo: a comment, quotes or trailing spaces moved card's key"
sed -i 's/description: "  A test card.  "/description: A test card./; s/kind: "image"/kind: image/' "$T/project.yaml"

step "a project: a text edit, and reverting returns the old output with no run"
local old_file; old_file=$(row "$T" card .path)
sed -i 's/description: A test card\./description: Another card./' "$T/project.yaml"
[ "$(row "$T" card .status)" = '"out of date: description changed"' ] || check "memo: a text edit: card is $(row "$T" card .status)"
[ "$(row "$T" poster .status)" = '"waits for what it reads"' ] || check "memo: poster reads card, which has no state: $(row "$T" poster .status)"
render "$T" poster --missing || check "memo: render poster: $(tail -c 300 "$OUT/render.txt")"
[ "$(jq -c '[.assets.card, .assets.poster | .versions | length]' "$T/project.lock")" = "[2,2]" ] || check "memo: rendering a reader did not make what it reads first: $(jq -c '.assets | map_values(.versions | length)' "$T/project.lock")"
first=$(runs "$T")
sed -i 's/description: Another card\./description: A test card./' "$T/project.yaml"
J '[.assets[] | select(.name == "card" or .name == "poster") | .status] == ["current", "current"]' <<< "$(status "$T")" || check "memo: reverted, not current: $(status "$T" | jq -c '[.assets[] | {name, status}]')"
[ "$(row "$T" card .path)" = "$old_file" ] || check "memo: reverted, card reads $(row "$T" card .path), not $old_file"
render "$T" all --missing || check "memo: render all, reverted"
[ "$(runs "$T")" = "$first" ] || check "memo: reverting ran something"

step "a project: defaults and options are inputs; a release in with is not"
sed -i 's/style: Plain\./style: Bold./' "$T/project.yaml"
J '([.stale[]?.name] | index("card")) and ([.missing[]?] | index("poster"))' <<< "$(status "$T")" || check "memo: a defaults edit did not put card out of date, and poster waiting on it: $(status "$T" | jq -c '{stale, missing}')"
sed -i 's/style: Bold\./style: Plain./' "$T/project.yaml"
cat >> "$T/project.yaml" <<YAML
  keyed:
    render: {with: _/birefnet@9.9.9}
    kind: image
    image: <card>
YAML
was=$(row "$T" keyed .inputs)
sed -i 's#_/birefnet@9.9.9#birefnet#' "$T/project.yaml"
[ "$(row "$T" keyed .inputs)" = "$was" ] || check "memo: a release or _/ in with moved the key"
sed -i 's#with: birefnet#with: ~/a/checkout/of/birefnet#' "$T/project.yaml"
[ "$(row "$T" keyed .inputs)" = "$was" ] || check "memo: a local checkout of the package moved the key"
sed -i 's#with: ~/a/checkout/of/birefnet#with: other#' "$T/project.yaml"
[ "$(row "$T" keyed .inputs)" != "$was" ] || check "memo: another package did not move the key"
sed -i 's#with: other#with: birefnet, steps: 2#' "$T/project.yaml"
[ "$(row "$T" keyed .inputs)" != "$was" ] || check "memo: options did not move the key"
[ "$(status "$T" | jq -c '[.warnings[]? | select(test("no section"))]')" = "[]" ] || check "memo: a format 2 file warned of a section"
sed -i '/^  keyed:$/,$d' "$T/project.yaml"

step "a project: each render a new version, and -n"
first=$(made "$T" card)
render "$T" card || check "memo: render card: $(tail -c 300 "$OUT/render.txt")"
[ "$(made "$T" card)" = "$((first + 1))" ] || check "memo: a render made $(made "$T" card) of card, from $first"
render "$T" card -n 2 || check "memo: -n 2: $(tail -c 300 "$OUT/render.txt")"
[ "$(made "$T" card)" = "$((first + 3))" ] || check "memo: -n 2 made $(made "$T" card) of card, from $((first + 1))"
J '[.assets.card.versions[-3:][].seed] | unique | length == 3' "$T/project.lock" || check "memo: a render and -n did not draw new seeds"
J '[.assets.card.versions[].number] == [range(1; (.assets.card.versions | length) + 1)]' "$T/project.lock" || check "memo: card's numbers are not in order"
render "$T" copied || check "memo: copied over a new card"
first=$(made "$T" copied)
render "$T" copied || check "memo: a render of a function with no seed: $(tail -c 300 "$OUT/render.txt")"
says memo "takes no seed"
[ "$(made "$T" copied)" = "$first" ] || check "memo: a render of a function with no seed made a version"
code=0; cozy project -p "$T" render card -n 2 --approve --on local > "$OUT/render.txt" 2>&1 || code=$?
{ [ "$code" != 0 ] && says memo --approve; } || check "memo: -n 2 --approve was not refused (exit $code)"
}

# A run belongs to a project, as a commit to a repository: outside one it refuses, and in one its outputs land in files/ by hash,
# where --file and approve name them, and reclaim takes them once nothing does.
project_runs() {
step "a project: a run outside one refuses, and in one lands in files/"
local R=$OUT/run-project away code hash other real lines
away=$(mktemp -d)
code=0; (cd "$away" && cozy run "$here/project/draw.py" prompt=outside) > "$OUT/outside.txt" 2>&1 || code=$?
rmdir "$away"
{ [ "$code" = 2 ] && grep -qF 'not in a project: cd into one, or make one' "$OUT/outside.txt" && grep -qF 'next: cozy project new my-project' "$OUT/outside.txt"; } || check "runs: outside a project: exit $code: $(tail -c 300 "$OUT/outside.txt")"
cozy project new "$R" > /dev/null || check "runs: project new"
cat > "$R/project.yaml" <<YAML
version: 2
defaults:
  image:
    with: $here/project/draw.py
assets:
  kyogre:
    kind: image
    description: A blue whale.
YAML
real=$(cd "$R" && pwd -P)
(cd "$R" && cozy run "$here/project/draw.py" prompt=first) > "$OUT/run-first.txt" 2>&1 || check "runs: a run in the project: $(tail -c 300 "$OUT/run-first.txt")"
hash=$(sed -n 's/^image: \([0-9a-f]\{12\}\)$/\1/p' "$OUT/run-first.txt"); [ -n "$hash" ] || check "runs: no line to paste: $(tail -c 300 "$OUT/run-first.txt")"
ls "$R/files/$hash"* > /dev/null 2>&1 || check "runs: $hash is not in files/: $(ls "$R/files")"
[ "$(find "$R/.runs" -type f 2> /dev/null | wc -l)" = 0 ] || check "runs: a file stayed where it landed: $(find "$R/.runs" -type f)"
J --arg h "$hash" '.runs | length == 1 and (.[0] | (.sha256 | ltrimstr("sha256:") | startswith($h)) and .run > 0 and .output != null)' "$R/project.lock" || check "runs: the lock: $(jq -c .runs "$R/project.lock")"
J '.run_files == 1' <<< "$(status "$R")" || check "runs: status does not count it: $(status "$R" | jq -c .run_files)"
cozy run -p "$R" "$here/project/draw.py" prompt=second > "$OUT/run-second.txt" 2>&1 || check "runs: -p: $(tail -c 300 "$OUT/run-second.txt")"
other=$(sed -n 's/^image: \([0-9a-f]\{12\}\)$/\1/p' "$OUT/run-second.txt"); [ -n "$other" ] && [ "$other" != "$hash" ] || check "runs: -p made no new file: $(tail -c 300 "$OUT/run-second.txt")"
(cd "$R" && cozy run "$here/project/copied.py" --file "image=$hash") > "$OUT/run-copy.txt" 2>&1 || check "runs: --file by hash: $(tail -c 300 "$OUT/run-copy.txt")"
J '.runs | length == 3' "$R/project.lock" || check "runs: three runs, and the lock lists $(jq -c '.runs | length' "$R/project.lock")"

step "a project: run ls in it lists its runs, and --all every one"
(cd "$R" && cozy --json run ls) > "$OUT/runs-in.json" 2> "$OUT/runs-in.err" || check "runs: run ls in it: $(tail -c 300 "$OUT/runs-in.err")"
J --arg p "$real" '(.runs | length == 3) and all(.runs[]; .project == $p)' "$OUT/runs-in.json" || check "runs: run ls in it: $(jq -c '[.runs[] | {number, project}]' "$OUT/runs-in.json")"
(cd "$R" && cozy --json run ls --all --limit 500) > "$OUT/runs-all.json" 2> "$OUT/runs-all.err" || check "runs: run ls --all: $(tail -c 300 "$OUT/runs-all.err")"
J --arg p "$real" 'any(.runs[]; .project != $p)' "$OUT/runs-all.json" || check "runs: run ls --all lists only this project"

step "a project: approve takes any file of its, by hash"
cozy project -p "$R" approve kyogre "$hash" > "$OUT/approve.txt" 2>&1 || check "runs: approve by hash: $(tail -c 300 "$OUT/approve.txt")"
grep -q "^    image: $hash\$" "$R/project.yaml" || check "runs: approve wrote: $(grep -n 'image' "$R/project.yaml")"
J '.assets[] | select(.name == "kyogre") | .pinned and (.status | startswith("pinned"))' <<< "$(status "$R")" || check "runs: kyogre: $(row "$R" kyogre .)"
J '.run_files == 2' <<< "$(status "$R")" || check "runs: a pinned file still counts as loose: $(status "$R" | jq -c .run_files)"

step "a project: reclaim takes a run's file once nothing names it"
lines=$(cozy --json project -p "$R" reclaim | jq .files); [ "$lines" -ge 2 ] || check "runs: reclaim lists $lines files"
ls "$R/files/$other"* > /dev/null 2>&1 || check "runs: reclaim with no --yes deleted"
cozy project -p "$R" reclaim --yes > "$OUT/reclaim.txt" 2>&1 || check "runs: reclaim --yes: $(tail -c 300 "$OUT/reclaim.txt")"
! ls "$R/files/$other"* > /dev/null 2>&1 || check "runs: reclaim --yes left a file nothing names"
ls "$R/files/$hash"* > /dev/null 2>&1 || check "runs: reclaim --yes took the pinned file"
J --arg h "$hash" '.runs | length == 1 and (.[0].sha256 | ltrimstr("sha256:") | startswith($h))' "$R/project.lock" || check "runs: rows left: $(jq -c .runs "$R/project.lock")"
J '.run_files == 0' <<< "$(status "$R")" || check "runs: status after reclaim: $(status "$R" | jq -c .run_files)"

step "a project: a script named like a module Python already has runs"
local name
for name in copy json; do
  cat > "$OUT/$name.py" <<'PY'
# /// script
# requires-python = ">=3.12"
# dependencies = ["cozy-runtime>=0.38", "msgspec>=0.19"]
# ///
"""Named like a module Python already has."""

import msgspec
from cozy_runtime.author import App

app = App()


class Ask(msgspec.Struct):
    text: str


class Said(msgspec.Struct):
    back: str


@app.entrypoint(accelerator=False)
def echo(payload: Ask) -> Said:
    return Said(back=payload.text[::-1])
PY
  (cd "$R" && cozy --json run "$OUT/$name.py/echo" text=abc --on local) > "$OUT/named-$name.ndjson" 2> "$OUT/named-$name.err" || check "runs: $name.py: $(tail -c 300 "$OUT/named-$name.err")"
  tail -1 "$OUT/named-$name.ndjson" | J '.run.result.back == "cba"' || check "runs: $name.py did not answer: $(tail -1 "$OUT/named-$name.ndjson" | jq -c .run.reason)"
done
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
    render: {with: $here/project/draw.py}
    kind: image
    description: A test card.    # a note on it
    aspect_ratio: "16:9"

  flat: {kind: image, description: A flat one., render: {with: $here/project/draw.py}}
  copied:
    render: {with: $here/project/copied.py}
    kind: image
    image: <card>
  poster:
    render: {with: $here/project/draw.py}
    kind: image
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
render "$T" card --missing || check "pins: render a pinned card: $(tail -c 300 "$OUT/render.txt")"
[ "$(made "$T" card)" = "$version" ] || check "pins: a text edit re-rendered a pinned card"
changed=$(row "$T" card .status)
[ "$changed" = '"pinned: version 1; its inputs changed: description"' ] || check "pins: status of an edited pin is $changed"
render "$T" card || check "pins: naming a pinned card: $(tail -c 300 "$OUT/render.txt")"
[ "$(made "$T" card)" = "$((version + 1))" ] || check "pins: naming it did not make a take to look at"
[ "$(row "$T" card .approved)" = 1 ] || check "pins: naming it moved the pin"
sed -i 's/description: Another card\./description: A test card./' "$T/project.yaml"
[ "$(row "$T" poster .inputs)" = "$(row "$T" poster .inputs)" ] || true

step "a project: unpin removes the line"
cozy project -p "$T" unpin card > "$OUT/unpin.txt" 2>&1 || check "pins: unpin card: $(tail -c 300 "$OUT/unpin.txt")"
diff "$before" "$T/project.yaml" > "$OUT/unpin.diff" || check "pins: unpin did not give back the file: $(cat "$OUT/unpin.diff")"
J '.assets[] | select(.name == "card") | (.pinned | not) and .approved == null' <<< "$(status "$T")" || check "pins: unpinned, status is $(row "$T" card .)"

step "a project: a pin over an input keeps it, and a line of its own"
render "$T" copied > /dev/null 2>&1 || true
cozy project -p "$T" approve copied 1 > "$OUT/approve.txt" 2>&1 || check "pins: approve copied 1: $(tail -c 300 "$OUT/approve.txt")"
grep -Eq '^    image: [0-9a-f"]+  # was <card>$' "$T/project.yaml" || check "pins: a pin over <card> is not one line keeping it: $(grep -n 'image:' "$T/project.yaml")"
cozy project -p "$T" unpin copied > "$OUT/unpin.txt" 2>&1 || check "pins: unpin copied: $(tail -c 300 "$OUT/unpin.txt")"
diff "$before" "$T/project.yaml" > "$OUT/unpin.diff" || check "pins: unpin copied did not give back the file: $(cat "$OUT/unpin.diff")"
cozy project -p "$T" approve flat 1 > "$OUT/approve.txt" 2>&1 || check "pins: approve an asset on one line: $(tail -c 300 "$OUT/approve.txt")"
grep -Eq '^  flat: \{image: [0-9a-f"]+, kind: image, description: A flat one\., render: \{with: .*/draw\.py\}\}$' "$T/project.yaml" || check "pins: flat is $(grep -n 'flat:' "$T/project.yaml")"
cozy project -p "$T" unpin flat > "$OUT/unpin.txt" 2>&1 || check "pins: unpin flat: $(tail -c 300 "$OUT/unpin.txt")"
diff "$before" "$T/project.yaml" > "$OUT/unpin.diff" || check "pins: unpin flat did not give back the file: $(cat "$OUT/unpin.diff")"

step "a project: re-approving art brings what was made from it back with no run"
local copies; copies=$(made "$T" copied)
cozy project -p "$T" approve card 1 > /dev/null 2>&1 || check "pins: approve card 1 again"
render "$T" copied || check "pins: render copied: $(tail -c 300 "$OUT/render.txt")"
cozy project -p "$T" approve card 2 > /dev/null 2>&1 || check "pins: approve card 2"
render "$T" copied --missing || check "pins: render copied on 2: $(tail -c 300 "$OUT/render.txt")"
copies=$(made "$T" copied)
cozy project -p "$T" approve card 1 > /dev/null 2>&1 || check "pins: approve card 1 again"
render "$T" copied --missing || check "pins: render copied on 1 again: $(tail -c 300 "$OUT/render.txt")"
[ "$(made "$T" copied)" = "$copies" ] || check "pins: re-approving art ran what reads it: $(made "$T" copied), not $copies"

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
    render: {with: $here/project/draw.py}
    kind: image
    image: cafe0000
    description: It renders without that.
  nothing:
    image: ./path.png
  no_package:
    kind: image
    description: Nothing makes me.
  no_kind:
    render: {with: $here/project/draw.py}
    description: An image, then.
  no_adapter:
    render: {with: $here/project/draw.py}
    kind: audio
    description: Nor me either.
YAML
J '.assets[] | select(.name == "teal") | .pinned and (.with == null) and (.approved == null)' <<< "$(status "$C")" || check "pins: a constant: $(row "$C" teal .)"
J '(.assets[] | select(.name == "singer") | (.files | keys) == ["image"]) and (.warnings | any(contains("singer'"'"'s audio: an asset has one main file")))' <<< "$(status "$C")" || check "pins: a second file: $(row "$C" singer .)"
J '.assets[] | select(.name == "teal") | .kind == "image"' <<< "$(status "$C")" || check "pins: a constant's kind is not its file's: $(row "$C" teal .)"
J '.warnings | any(contains("no file'"'"'s hash starts with cafe0000")) and any(contains("is not a hash"))' <<< "$(status "$C")" || check "pins: a bad pin is not warned of: $(status "$C" | jq -c .warnings)"
render "$C" teal || check "pins: render a constant: $(tail -c 300 "$OUT/render.txt")"
says pins "is a constant"
code=0; cozy project -p "$C" unpin teal > "$OUT/unpin.txt" 2>&1 || code=$?
[ "$code" != 0 ] || check "pins: unpinning a constant took its one line"
render "$C" wrong || check "pins: wrong's bad pin stopped it: $(tail -c 300 "$OUT/render.txt")"
[ "$(made "$C" wrong)" = 1 ] || check "pins: wrong was not made without its bad pin"

step "a project: an asset with no package, no kind, or no adapter says so, and the rest goes on"
J '.warnings | any(contains("no_package has no `render.with`, and no `defaults.image.with`"))' <<< "$(status "$C")" || check "pins: an asset with nothing to make is not said: $(status "$C" | jq -c .warnings)"
J '.assets[] | select(.name == "no_kind") | .kind == "image"' <<< "$(status "$C")" || check "pins: an asset with no kind is not an image: $(row "$C" 'no_kind' .)"
sed -i 's/It renders without that\./Rendered again./' "$C/project.yaml"
render "$C" all --missing || check "pins: render all with what can't be made: $(tail -c 400 "$OUT/render.txt")"
says pins "no_adapter: "
says pins "has no adapter for kind audio: it can't be made"
[ "$(made "$C" wrong)" = 2 ] || check "pins: what could be made was not: wrong has $(made "$C" wrong)"
}

# An entity is identity and links, no work. A link names an asset, or holds a hash, shorthand for a constant asset.
entities() {
step "a project: an entity's links and sub-entities, and what a read of each gets"
local E=$OUT/entities pic voice code
mkdir -p "$E"; still "$E/teal.png" teal
printf 'version: 2\nassets: {}\n' > "$E/project.yaml"
pic=$(prefix "$E" "$E/teal.png")
ffmpeg -v error -y -f lavfi -i sine=frequency=220:duration=1 "$E/voice.wav"
voice=$(prefix "$E" "$E/voice.wav")
cat > "$E/project.yaml" <<YAML
version: 2
defaults:
  all:
    style: Nothing reads this.
  image:
    with: $here/project/draw.py
entities:
  hero:
    kind: character
    description: A brave hero.
    image: <hero_sheet>
    voice: <hero_voice>
    pose: <hero_pose>
    render: {with: nothing}
    badge: $pic
    motto: Not a link.
    lost: <no_such_asset>
    primordial:
      description: The hero of old.
      image: <old_sheet>
      mark: $pic
      deeper: {image: <hero_sheet>}
  hero_voice:
    description: A name both sections use.
assets:
  meeting:
    kind: video
    render: {with: $here/project/shots.py}
    summary: <hero> meets <hero.primordial>, in <hero.pose>.
  odd:
    kind: character
    image: $pic
  hero_sheet:
    kind: image
    description: <hero>, on one sheet.
  hero_voice:
    kind: audio
    audio: $voice
  hero_pose:
    kind: image
    description: A pose of its own.
  old_sheet:
    kind: image
    description: <hero.primordial>, from <hero_sheet>.
YAML
said() { J --arg s "$1" '.warnings | any(contains($s))' <<< "$(status "$E")" || check "entities: no warning of '$1': $(status "$E" | jq -c .warnings)"; }
said "hero's render: an entity holds no way of making anything"
said "defaults.all: the only keys are image, video and audio"
said "odd's kind: an asset is an image, a video or audio"
said "hero's motto: an entity links to an asset"
said "hero's lost: <no_such_asset> is nothing in the project: add an asset"
said "hero.primordial's deeper: sub-entities go one level"
said "hero_voice is an asset and an entity"
said "hero_sheet mentions hero, which is made from it: that mention stays plain text"
said "old_sheet mentions hero.primordial, which is made from it: that mention stays plain text"
J '[.warnings[] | select(contains("hero_sheet mentions hero,"))] | length == 1' <<< "$(status "$E")" || check "entities: more than one warning of hero_sheet's mention"
J '(.entities | map(.name)) == ["hero"] and (.entities[0] | .kind == "character" and (.links | keys) == ["badge", "image", "lost", "pose", "voice"] and (.subs | map(.name)) == ["hero.primordial"])' <<< "$(status "$E")" || check "entities: status lists $(status "$E" | jq -c .entities)"
J '[.assets[].name] | index("hero") == null and index("hero.primordial") == null' <<< "$(status "$E")" || check "entities: an entity is listed as an asset"
render "$E" hero || check "entities: render hero: $(tail -c 400 "$OUT/render.txt")"
J '[.assets | keys[]] | sort == ["hero pose", "hero sheet", "old sheet"]' "$E/project.lock" || check "entities: render hero made $(jq -c '.assets | keys' "$E/project.lock")"
J '(.assets["hero sheet"].versions[0].reads // []) | length == 0' "$E/project.lock" || check "entities: an asset an entity links to read it: $(jq -c '.assets["hero sheet"].versions[0].reads' "$E/project.lock")"
J '[.assets["old sheet"].versions[0].reads[].name] == ["hero_sheet"]' "$E/project.lock" || check "entities: old sheet reads $(jq -c '.assets["old sheet"].versions[0].reads' "$E/project.lock")"
render "$E" meeting || check "entities: render meeting: $(tail -c 400 "$OUT/render.txt")"
J '.assets.meeting.versions[0].reads | map({(.name): (.files | keys)}) | add == {"hero": ["image", "voice"], "hero.primordial": ["image", "voice"], "hero.pose": ["image"]}' "$E/project.lock" || check "entities: meeting reads $(jq -c '.assets.meeting.versions[0].reads' "$E/project.lock")"
J '.assets.meeting.versions[0].reads | map(select(.name == "hero")) | .[0].files.image != null' "$E/project.lock" || check "entities: <hero> has no picture"

step "a project: a name can't hold spaces or commas"
local N=$OUT/names
mkdir -p "$N"
printf 'version: 2\nassets:\n  kyogre sheet:\n    kind: image\n' > "$N/project.yaml"
code=0; cozy project -p "$N" status > "$OUT/name.txt" 2>&1 || code=$?
{ [ "$code" != 0 ] && grep -qF "\`kyogre sheet\`: names can't hold spaces or commas; use \`kyogre_sheet\`" "$OUT/name.txt"; } || check "names: a space in a name: exit $code: $(tail -c 300 "$OUT/name.txt")"
printf 'version: 2\nentities:\n  a,b:\n    kind: image\n' > "$N/project.yaml"
code=0; cozy project -p "$N" status > "$OUT/name.txt" 2>&1 || code=$?
{ [ "$code" != 0 ] && grep -qF "use \`a_b\`" "$OUT/name.txt"; } || check "names: a comma in a name: exit $code: $(tail -c 300 "$OUT/name.txt")"
printf 'version: 2\nassets:\n  a:\n    kind: image\n    description: Prose <a b> here.\n' > "$N/project.yaml"
J '.warnings | any(contains("<a b>")) | not' <<< "$(status "$N")" || check "names: <a b> is just text, and was said: $(status "$N" | jq -c .warnings)"
code=0; cozy project -p "$N" rename a "b c" > "$OUT/name.txt" 2>&1 || code=$?
{ [ "$code" != 0 ] && grep -q "names can't hold" "$OUT/name.txt"; } || check "names: rename took a space: exit $code: $(tail -c 300 "$OUT/name.txt")"

step "a project: a hash in an entity's link is a constant asset, and approve rewrites its line"
local red; still "$E/red.png" red; red=$(prefix "$E" "$E/red.png")
J '[.assets[] | select(.name == "hero.badge" or .name == "hero.primordial.mark") | .pinned and .kind == "image"] == [true, true]' <<< "$(status "$E")" || check "entities: hash links: $(status "$E" | jq -c '[.assets[].name]')"
J '.entities[0].links.badge == "hero.badge" and .entities[0].subs[0].links.mark == "hero.primordial.mark"' <<< "$(status "$E")" || check "entities: the links: $(status "$E" | jq -c .entities)"
cozy project -p "$E" approve hero.badge "$red" > "$OUT/approve.txt" 2>&1 || check "entities: approve hero.badge: $(tail -c 300 "$OUT/approve.txt")"
grep -q "^    badge: $red\$" "$E/project.yaml" || check "entities: approve did not rewrite the link: $(grep -n badge "$E/project.yaml")"
cozy project -p "$E" approve hero.primordial.mark "$red" > "$OUT/approve.txt" 2>&1 || check "entities: approve hero.primordial.mark: $(tail -c 300 "$OUT/approve.txt")"
grep -q "^      mark: $red\$" "$E/project.yaml" || check "entities: approve did not rewrite the sub-entity's link: $(grep -n mark "$E/project.yaml")"

step "a project: a package's own tags are text, and a name that names nothing is said"
local D=$OUT/dialogue look
mkdir -p "$D"; plate "$D/look.mp4" testsrc2 440 800
printf 'version: 2\nassets: {}\n' > "$D/project.yaml"
look=$(prefix "$D" "$D/look.mp4")
cat > "$D/project.yaml" <<YAML
version: 2
assets:
  look:
    video: $look
  kyogre:
    video: $look
  talk:
    kind: video
    render: {with: $here/project/shots.py}
    summary: <look> says <D><English>Hello<d> and <d><ENGLISH>Hi</d> to <kyogee>, <Klingon>, <Picture 1> and <a b> too.
YAML
render "$D" talk || check "text: render talk: $(tail -c 300 "$OUT/render.txt")"
says text "<kyogee>: did you mean <kyogre>?"
says text "talk mentions <Klingon>, which is nothing in the project"
! grep -qiE 'mentions <(d|english)>|<(d|english)>: did you mean' "$OUT/render.txt" || check "text: render warned of a tag: $(tail -c 400 "$OUT/render.txt")"
J '.warnings | any(contains("<kyogee>: did you mean <kyogre>?")) and any(contains("<Klingon>"))' <<< "$(status "$D")" || check "text: a name that names nothing was not said: $(status "$D" | jq -c .warnings)"
J '[.warnings[] | select(contains("<d>") or contains("<D>") or contains("</d>") or contains("nglish") or contains("NGLISH") or contains("<a b>") or contains("Picture"))] == []' <<< "$(status "$D")" || check "text: a tag was warned of: $(status "$D" | jq -c .warnings)"
J '[.assets.talk.versions[0].reads[].name] == ["look"]' "$D/project.lock" || check "text: talk reads $(jq -c '.assets.talk.versions[0].reads' "$D/project.lock")"
grep -qF '<D><English>Hello<d> and <d><ENGLISH>Hi</d>' "$D/$(jq -r '.files[.assets.talk.versions[0].files.given].path' "$D/project.lock")" || check "text: the tags did not reach the adapter unchanged"
sed -i "/^  look:\$/i\\  d:\\n    video: $look" "$D/project.yaml"
J '.warnings | any(contains("d is also a tag of"))' <<< "$(status "$D")" || check "text: a name that is its package's tag was not said: $(status "$D" | jq -c .warnings)"
[ "$(row "$D" talk .status)" = '"current"' ] || check "text: a thing named as a tag was read: $(row "$D" talk .status)"
sed -i "/^  look:\$/i\\  ENGLISH:\\n    video: $look" "$D/project.yaml"
sed -i 's/<Klingon>/<Englsh>/' "$D/project.yaml"
J '(.warnings | any(contains("english is also a tag of"))) and (.warnings | any(contains("talk mentions <Englsh>, which is nothing")))' <<< "$(status "$D")" || check "text: a tag in another case, or a typo of one, was said wrong: $(status "$D" | jq -c .warnings)"

step "a project: a cycle of mentions, or through an entity, is said, and what is in it still renders"
local Y=$OUT/cycle
mkdir -p "$Y"
cat > "$Y/project.yaml" <<YAML
version: 2
defaults:
  image:
    with: $here/project/draw.py
assets:
  one:
    description: From <two>.
  two:
    description: From <one>.
YAML
J '[.warnings[] | select(contains("which is made from it: that mention stays plain text"))] | length == 1' <<< "$(status "$Y")" || check "cycle: not said once: $(status "$Y" | jq -c .warnings)"
cat >> "$Y/project.yaml" <<YAML
  long:
    description: From <hub>.
  back:
    description: From <long>.
entities:
  hub:
    kind: character
    description: Through an entity.
    image: <back>
YAML
J '.warnings | any(contains("mentions hub, which is made from it")) or any(contains("mentions long, which is made from it")) or any(contains("mentions back, which is made from it"))' <<< "$(status "$Y")" || check "cycle: through an entity not said: $(status "$Y" | jq -c .warnings)"
render "$Y" all || check "cycle: render all: $(tail -c 300 "$OUT/render.txt")"
{ [ "$(made "$Y" one)" = 1 ] && [ "$(made "$Y" two)" = 1 ] && [ "$(made "$Y" long)" = 1 ] && [ "$(made "$Y" back)" = 1 ]; } || check "cycle: made $(jq -c '.assets | map_values(.versions | length)' "$Y/project.lock")"

step "a project: an entity's words are in what reads it, and an entity holds no version"
sed -i 's/description: A brave hero\./description: A braver hero./' "$E/project.yaml"
J '[.stale[]?.name] | index("meeting") != null' <<< "$(status "$E")" || check "entities: a description edit left meeting current: $(status "$E" | jq -c .stale)"
code=0; cozy project -p "$E" approve hero 1 > "$OUT/approve.txt" 2>&1 || code=$?
{ [ "$code" != 0 ] && grep -q "is an entity" "$OUT/approve.txt"; } || check "entities: approve of an entity: exit $code: $(tail -c 300 "$OUT/approve.txt")"
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
    render: {with: $here/project/draw.py}
    kind: image
    description: The old one.
  reader:
    render: {with: $here/project/draw.py}
    kind: image
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
render "$T" all --missing || check "orphans: render all renamed"
shared=$(jq -r '.assets.new.versions[0].files.image' "$T/project.lock")
# Another asset's version names the same file: it stays. The rest of the orphan goes.
cat >> "$T/project.yaml" <<YAML
  doomed:
    render: {with: $here/project/draw.py}
    kind: image
    description: Going.
  kept:
    render: {with: $here/project/draw.py}
    kind: image
    description: Stays.
YAML
render "$T" all --missing || check "orphans: render all with doomed"
gone_file=$(jq -r '.assets.doomed.versions[0].files.image' "$T/project.lock")
jq --arg s "$gone_file" '.assets.kept.versions[0].writes = {"a frame": {"image": $s}}' "$T/project.lock" > "$T/l" && mv "$T/l" "$T/project.lock"
sed -i '/^  doomed:$/,/^    description: Going\.$/d' "$T/project.yaml"
cat >> "$T/project.yaml" <<YAML
  second:
    render: {with: $here/project/draw.py}
    kind: image
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
    kind: video
    render: {with: $here/project/shots.py}
    seconds: 15
YAML
cozy project -p "$T" render waiting --on local > "$OUT/slow.txt" 2>&1 &
local going=$!
for _ in $(seq 60); do J '.busy' <<< "$(status "$T")" && break; sleep 0.5; done
code=0; cozy project -p "$T" reclaim --yes > "$OUT/busy.txt" 2>&1 || code=$?
{ [ "$code" != 0 ] && grep -q 'rendering' "$OUT/busy.txt"; } || check "orphans: reclaim --yes while rendering (exit $code): $(tail -c 300 "$OUT/busy.txt")"
wait "$going" || check "orphans: the slow render: $(tail -c 300 "$OUT/slow.txt")"
}

# A second render of a project already rendering is told at once what is going, and follows it as the first does.
joining() {
step "a project: a second render joins the one going, and hears its run at once"
local T=$OUT/joining first second n heard
mkdir -p "$T"
cat > "$T/project.yaml" <<YAML
version: 2
assets:
  waiting:
    kind: video
    render: {with: $here/project/shots.py}
    seconds: 15
YAML
cozy project -p "$T" render waiting --on local > "$OUT/join-first.txt" 2>&1 &
first=$!
n=''
for _ in $(seq 240); do n=$(row "$T" waiting .status | sed -n 's/.*making (run \([0-9]*\)).*/\1/p'); [ -n "$n" ] && break; sleep 0.25; done
[ -n "$n" ] || check "joining: the first render never showed a run: $(tail -c 300 "$OUT/join-first.txt")"
cozy project -p "$T" render waiting --missing --on local > "$OUT/join-second.txt" 2>&1 &
second=$!
heard=''
for _ in $(seq 12); do
  grep -q 'already going in joining, and this adds nothing to it: joining it' "$OUT/join-second.txt" && grep -q '^waiting: take 1 running' "$OUT/join-second.txt" && { heard=1; break; }
  sleep 0.25
done
[ -n "$heard" ] || check "joining: the second render said nothing within 3 s, while run $n went: $(cat "$OUT/join-second.txt")"
wait "$first" || check "joining: the first render: $(tail -c 300 "$OUT/join-first.txt")"
wait "$second" || check "joining: the second render: $(tail -c 300 "$OUT/join-second.txt")"
[ "$(made "$T" waiting)" = 1 ] || check "joining: two renders of one ask made $(made "$T" waiting)"
}

# Renders queued from one shell, seen, one canceled, and the rest land. A canceled `render all` doesn't send again what it canceled.
queueing() {
step "a project: three renders queued from one shell, one canceled, the rest land"
local T=$OUT/queueing
mkdir -p "$T"
cat > "$T/project.yaml" <<YAML
version: 2
defaults:
  image:
    with: $here/project/draw.py
assets:
  first:
    kind: video
    render: {with: $here/project/shots.py}
    seconds: 8
  second:
    description: From <first>.
  third:
    description: Also from <first>.
YAML
cozy project -p "$T" render first --queue --on local > "$OUT/queue-1.txt" 2>&1 || check "queue: first: $(tail -c 300 "$OUT/queue-1.txt")"
cozy project -p "$T" render second --queue --on local > "$OUT/queue-2.txt" 2>&1 || check "queue: second: $(tail -c 300 "$OUT/queue-2.txt")"
cozy project -p "$T" render third --queue --on local > "$OUT/queue-3.txt" 2>&1 || check "queue: third: $(tail -c 300 "$OUT/queue-3.txt")"
grep -q 'queued first' "$OUT/queue-1.txt" || check "queue: first said $(cat "$OUT/queue-1.txt")"
grep -q 'queued third, after .*first' "$OUT/queue-3.txt" || check "queue: third said $(cat "$OUT/queue-3.txt")"
J '[.queue[].name] == ["first", "second", "third"]' <<< "$(status "$T")" || check "queue: status shows $(status "$T" | jq -c .queue)"
cozy project -p "$T" status > "$OUT/queue-status.txt" 2>&1
grep -q '^queued: .*second, third' "$OUT/queue-status.txt" || check "queue: status says $(cat "$OUT/queue-status.txt")"
cozy project -p "$T" cancel third > "$OUT/queue-cancel.txt" 2>&1 || check "queue: cancel: $(tail -c 300 "$OUT/queue-cancel.txt")"
grep -q 'dropped third' "$OUT/queue-cancel.txt" || check "queue: cancel said $(cat "$OUT/queue-cancel.txt")"
J '[.queue[].name] | index("third") == null' <<< "$(status "$T")" || check "queue: third is still queued: $(status "$T" | jq -c .queue)"
for _ in $(seq 240); do J '.busy | not' <<< "$(status "$T")" && break; sleep 0.5; done
{ [ "$(made "$T" first)" = 1 ] && [ "$(made "$T" second)" = 1 ] && [ "$(made "$T" third)" = 0 ]; } || check "queue: made $(jq -c '.assets | map_values(.versions | length)' "$T/project.lock")"
step "a project: a save that can't be read refuses a new render, and nothing else"
local B=$OUT/queue-broken code
mkdir -p "$B"
cat > "$B/project.yaml" <<YAML
version: 2
defaults:
  image:
    with: $here/project/draw.py
assets:
  slow:
    kind: video
    render: {with: $here/project/shots.py}
    seconds: 6
  after:
    description: From <slow>.
  also:
    description: Also from <slow>.
YAML
cozy project -p "$B" render after also --queue --on local > "$OUT/queue-broken.txt" 2>&1 || check "broken: render after: $(tail -c 300 "$OUT/queue-broken.txt")"
cp "$B/project.yaml" "$OUT/queue-broken.yaml"
printf 'assets: [\n' >> "$B/project.yaml"
J '.busy and ([.queue[].name] | index("after") != null) and (.warnings | any(contains("a new one is refused until this reads")))' <<< "$(status "$B")" || check "broken: status says $(status "$B" | jq -c '{busy, queue, warnings}')"
code=0; cozy project -p "$B" render after --on local > "$OUT/queue-broken-refused.txt" 2>&1 || code=$?
[ "$code" != 0 ] || check "broken: a new render read a file that doesn't"
cozy project -p "$B" cancel also > "$OUT/queue-broken-cancel.txt" 2>&1 || check "broken: cancel: $(tail -c 300 "$OUT/queue-broken-cancel.txt")"
grep -q 'dropped also' "$OUT/queue-broken-cancel.txt" || check "broken: cancel said $(cat "$OUT/queue-broken-cancel.txt")"
for _ in $(seq 240); do [ "$(made "$B" after)" = 1 ] && J '.busy | not' <<< "$(cozy --json project -p "$B" status 2>/dev/null || echo '{"busy":false}')" && break; sleep 0.5; done
{ [ "$(made "$B" slow)" = 1 ] && [ "$(made "$B" after)" = 1 ] && [ "$(made "$B" also)" = 0 ]; } || check "broken: with the file broken, made $(jq -c '.assets | map_values(.versions | length)' "$B/project.lock")"
cp "$OUT/queue-broken.yaml" "$B/project.yaml"

step "a project: a render uses the file as it was asked, so an edit waits for the next"
local W=$OUT/queue-edit going prompt
mkdir -p "$W"
cat > "$W/project.yaml" <<YAML
version: 2
defaults:
  image:
    with: $here/project/draw.py
assets:
  gate:
    kind: video
    render: {with: $here/project/shots.py}
    seconds: 6
  words:
    description: Old words.
  reader:
    description: From <words> after <gate>.
YAML
cozy project -p "$W" render all --on local > "$OUT/queue-edit.txt" 2>&1 &
going=$!
for _ in $(seq 120); do [ "$(status "$W" | jq '[.queue[]? | select(.name == "gate" and .run != null)] | length')" = 1 ] && break; sleep 0.25; done
sed -i 's/description: Old words\./description: New words./' "$W/project.yaml"
wait "$going" || check "edit: the render: $(tail -c 400 "$OUT/queue-edit.txt")"
! grep -q 'cannot render' "$OUT/queue-edit.txt" || check "edit: a pass said it couldn't: $(grep 'cannot render' "$OUT/queue-edit.txt")"
{ [ "$(made "$W" words)" = 1 ] && [ "$(made "$W" reader)" = 1 ]; } || check "edit: the pass made $(jq -c '.assets | map_values(.versions | length)' "$W/project.lock")"
prompt() { jq -r --arg n "$2" '.assets[$n].versions[-1].invocations[0].input.prompt' "$1/project.lock"; }
[ "$(prompt "$W" words)" = 'Old words.' ] || check "edit: the pass used $(prompt "$W" words)"
case "$(row "$W" words .status)" in *'out of date'*) ;; *) check "edit: words is $(row "$W" words .status)" ;; esac
render "$W" reader || check "edit: the next render: $(tail -c 400 "$OUT/render.txt")"
{ [ "$(made "$W" words)" = 2 ] && [ "$(made "$W" reader)" = 2 ] && [ "$(prompt "$W" words)" = 'New words.' ]; } || check "edit: the next render made $(jq -c '.assets | map_values(.versions | length)' "$W/project.lock")"
J --arg w "$(jq -r '.assets.words.versions[-1].files.image' "$W/project.lock")" '.assets.reader.versions[-1].reads[] | select(.name == "words") | .files.image == $w' "$W/project.lock" || check "edit: the reader did not read the new file"

step "a project: two queued renders with an edit between use their own text"
local Q=$OUT/queue-two
mkdir -p "$Q"
cat > "$Q/project.yaml" <<YAML
version: 2
defaults:
  image:
    with: $here/project/draw.py
assets:
  gate:
    kind: video
    render: {with: $here/project/shots.py}
    seconds: 4
  note:
    description: Text one, after <gate>.
YAML
cozy project -p "$Q" render note --queue --on local > "$OUT/queue-two-1.txt" 2>&1 || check "two: first: $(tail -c 300 "$OUT/queue-two-1.txt")"
sed -i 's/Text one/Text two/' "$Q/project.yaml"
cozy project -p "$Q" render note --queue --on local > "$OUT/queue-two-2.txt" 2>&1 || check "two: second: $(tail -c 300 "$OUT/queue-two-2.txt")"
for _ in $(seq 240); do J '.busy | not' <<< "$(status "$Q")" && break; sleep 0.5; done
J '[.assets.note.versions[].invocations[0].input.prompt] | (length == 2) and any(startswith("Text one")) and any(startswith("Text two"))' "$Q/project.lock" || check "two: note was made from $(jq -c '[.assets.note.versions[].invocations[0].input.prompt]' "$Q/project.lock")"
J '.walk | not' "$Q/project.lock" || check "two: a finished render left its text in the lock: $(jq -c '.walk | keys' "$Q/project.lock")"

[ -n "${ALONE:-}" ] && return 0
step "a project: a canceled render all sends nothing again"
local A=$OUT/queue-all
mkdir -p "$A"
cat > "$A/project.yaml" <<YAML
version: 2
defaults:
  image:
    with: $here/project/draw.py
assets:
  long:
    kind: video
    render: {with: $here/project/shots.py}
    seconds: 120
  after:
    description: From <long>.
YAML
cozy project -p "$A" render all --queue --on local > "$OUT/queue-all.txt" 2>&1 || check "queue: render all: $(tail -c 300 "$OUT/queue-all.txt")"
for _ in $(seq 120); do [ "$(status "$A" | jq '[.queue[] | select(.run != null)] | length')" = 1 ] && break; sleep 0.5; done
cozy project -p "$A" cancel all > "$OUT/queue-all-cancel.txt" 2>&1 || check "queue: cancel all: $(tail -c 300 "$OUT/queue-all-cancel.txt")"
{ grep -q 'canceled run' "$OUT/queue-all-cancel.txt" && grep -q 'dropped after' "$OUT/queue-all-cancel.txt"; } || check "queue: cancel all said $(cat "$OUT/queue-all-cancel.txt")"
for _ in $(seq 120); do J '(.busy | not) and (.queue // [] | length == 0)' <<< "$(status "$A")" && break; sleep 0.5; done
J '(.busy | not) and (.queue // [] | length == 0)' <<< "$(status "$A")" || check "queue: after cancel all: $(status "$A" | jq -c '{busy, queue}')"
{ [ "$(made "$A" long)" = 0 ] && [ "$(made "$A" after)" = 0 ]; } || check "queue: cancel all made $(jq -c '.assets | map_values(.versions | length)' "$A/project.lock")"
}

# Every render makes a new version, but of what is pinned and not named. Only approve or unpin moves a pin.
every_render() {
step "a project: render all twice, a pinned asset named, and --missing"
local T=$OUT/every-render pin line
mkdir -p "$T"
cat > "$T/project.yaml" <<YAML
version: 2
defaults:
  image:
    with: $here/project/draw.py
assets:
  fixed:
    description: Pinned.
  card:
    description: A card.
  poster:
    description: <card> as a poster.
YAML
render "$T" fixed || check "every render: fixed: $(tail -c 300 "$OUT/render.txt")"
cozy project -p "$T" approve fixed 1 > "$OUT/approve.txt" 2>&1 || check "every render: approve: $(tail -c 300 "$OUT/approve.txt")"
line=$(grep -n '^    image: ' "$T/project.yaml")
render "$T" all || check "every render: render all: $(tail -c 300 "$OUT/render.txt")"
says every-render "fixed is pinned: skipped. Name it to make a new version, or unpin it"
render "$T" all || check "every render: render all again: $(tail -c 300 "$OUT/render.txt")"
{ [ "$(made "$T" card)" = 2 ] && [ "$(made "$T" poster)" = 2 ] && [ "$(made "$T" fixed)" = 1 ]; } || check "every render: two passes made $(jq -c '.assets | map_values(.versions | length)' "$T/project.lock")"
J --arg c "$(jq -r '.assets.card.versions[-1].files.image' "$T/project.lock")" '.assets.poster.versions[-1].reads[] | select(.name == "card") | .files.image == $c' "$T/project.lock" || check "every render: the poster did not read the newest card"
J '[.assets.card.versions[].seed] | unique | length == 2' "$T/project.lock" || check "every render: a pass did not draw a new seed"
render "$T" fixed || check "every render: naming the pinned: $(tail -c 300 "$OUT/render.txt")"
{ [ "$(made "$T" fixed)" = 2 ] && [ "$(grep -n '^    image: ' "$T/project.yaml")" = "$line" ] && [ "$(row "$T" fixed .approved)" = 1 ]; } || check "every render: naming the pinned moved its pin: $(grep -n 'image:' "$T/project.yaml"), $(row "$T" fixed .approved)"
render "$T" all --missing || check "every render: --missing: $(tail -c 300 "$OUT/render.txt")"
{ [ "$(made "$T" card)" = 2 ] && [ "$(made "$T" poster)" = 2 ] && [ "$(made "$T" fixed)" = 2 ]; } || check "every render: --missing made $(jq -c '.assets | map_values(.versions | length)' "$T/project.lock")"
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
  clash:
    kind: video
    render: {with: $here/project/shots.py}
    summary: Two lights meet on <look>.
  look:
    video: $look
YAML
given() { cat "$T/$(jq -r --arg s "${1:-clash}" '.files[.assets[$s].versions[-1].files.given].path' "$T/project.lock")"; }
render "$T" clash || check "keeps: render: $(tail -c 300 "$OUT/render.txt")"
J '.keep == null and (.prompt | contains("clash") and contains("look=")) and .seed != null' <<< "$(given)" || check "keeps: the adapter's words did not reach the function: $(given)"
# Its untyped bytes stay on the machine: the lock says which, nothing lands, and nothing counts them as missing.
local held; held=$(jq -r '.assets.clash.versions[-1].files.latents' "$T/project.lock")
J --arg h "$held" '.files[$h] | .machine == "local" and .length == 32' "$T/project.lock" || check "keeps: the lock does not say who holds the latents: $(jq -c --arg h "$held" '.files[$h]' "$T/project.lock")"
[ ! -e "$T/files/${held#sha256:}" ] || check "keeps: the latents landed in files/"
J '.absent == 0' <<< "$(status "$T")" || check "keeps: status counts what stays on its machine as absent: $(status "$T" | jq -c .absent)"
code=0; render "$T" clash --take 9 || code=$?
{ [ "$code" = 2 ] && says keeps "has no take 9"; } || check "keeps: --take 9 exit $code: $(tail -c 300 "$OUT/render.txt")"
render "$T" clash --take 1 --in 3 || check "keeps: --take 1 --in 3: $(tail -c 300 "$OUT/render.txt")"
says keeps "continue_from.latents is read on local, which holds it"
[ ! -e "$T/files/${held#sha256:}" ] || check "keeps: --in fetched what its machine holds"
J '.keep == 3' <<< "$(given)" || check "keeps: --in 3 is not op.keep: $(given)"
J '.assets.clash.versions[-1] | .kept == {"take": 1, "until_s": 3.04} and (.seed != null)' "$T/project.lock" || check "keeps: the version does not record what it keeps: $(jq -c '.assets.clash.versions[-1].kept' "$T/project.lock")"
says keeps "take 2 is take 1 to 3.04 s"
render "$T" clash --in 2 || check "keeps: --in 2: $(tail -c 300 "$OUT/render.txt")"
J '.assets.clash.versions[-1].kept.take == 2' "$T/project.lock" || check "keeps: --in alone keeps the take in use: $(jq -c '.assets.clash.versions[-1].kept' "$T/project.lock")"
render "$T" all --missing --in 2 || check "keeps: all --in: $(tail -c 300 "$OUT/render.txt")"
says keeps "--take and --in keep the start of one asset"

step "a project: a run's untyped bytes stay on its machine unless they're all it made"
(cd "$T" && cozy --json run "$here/project/shots.py/shoot" prompt=kept --file "plate.video=$T/look.mp4" --on local) > "$OUT/kept.ndjson" 2> "$OUT/kept.err" || check "stays: the run did not end: $(tail -c 300 "$OUT/kept.err")"
tail -1 "$OUT/kept.ndjson" | jq .run > "$OUT/kept.json"
J '.output[] | select(.name == "latents") | .stays and .path == null and .sha256 != null' "$OUT/kept.json" || check "stays: --json: $(jq -c '.output' "$OUT/kept.json")"
J '.output[] | select(.name == "video") | .path != null and (.stays | not)' "$OUT/kept.json" || check "stays: its video did not land: $(jq -c '.output' "$OUT/kept.json")"
held=$(jq -r '.output[] | select(.name == "latents") | .sha256' "$OUT/kept.json")
J --arg h "$held" '.files[$h].machine == "local"' "$T/project.lock" || check "stays: a plain run's latents aren't in the lock"
local at; at=$(jq -r '.address + " " + .token' "$HOME/.cozy/calcifer/calcifer.lock.holder")
[ "$(curl -sS -H "Authorization: Bearer ${at#* }" -o /dev/null -w '%{http_code}' "http://${at%% *}/v1/runs/$(jq -r .number "$OUT/kept.json")/outputs/latents")" = 409 ] || check "stays: the output route did not refuse"
(cd "$T" && cozy --json run "$here/project/shots.py/only_untyped" prompt=alone --on local) > "$OUT/alone.ndjson" 2> "$OUT/alone.err" || check "stays: an untyped-only run did not end: $(tail -c 300 "$OUT/alone.err")"
tail -1 "$OUT/alone.ndjson" | J '.run.output[0] | (.stays | not) and .path != null' || check "stays: an untyped-only run did not come here: $(tail -1 "$OUT/alone.ndjson" | jq -c '.run.output')"
cozy project -p "$T" reclaim > "$OUT/reclaim-stays.txt" 2>&1 || check "stays: reclaim: $(tail -c 300 "$OUT/reclaim-stays.txt")"
}

# An image a beat writes: a screenshot at `at` into the beat. What the shot reads after that beat goes to its function, in its key; what
# it doesn't is a free frame, outside its key. A mention names exactly the asset it names.
written() {
step "a project: written written by beats"
local T=$OUT/series look take frames
mkdir -p "$T"; plate "$T/look.mp4" testsrc2 440 800 8
printf 'version: 2\nassets: {}\n' > "$T/project.yaml"
look=$(prefix "$T" "$T/look.mp4")
cat > "$T/project.yaml" <<YAML
version: 2
assets:
  first:
    kind: video
    render: {with: $here/project/shots.py}
    summary: Dusk on <look> and <coast>.
  long_take:
    kind: video
    render: {with: $here/project/shots.py}
    summary: Walking on <look>.
    beats:
      - duration: 4
        description: She runs on <coast>.
        writes:
          - image: coast.1
            at: 1
          - image: coast.2
            at: 3
      - duration: 4
        description: She stands on <coast.1>, then <coast.2>.
        writes:
          - image: harbor.1
            at: 1
  next:
    kind: video
    render: {with: $here/project/shots.py}
    summary: Dawn on <look> and <coast.2>, by <harbor.1>.
  look:
    video: $look
  coast:
    render: {with: $here/project/draw.py}
    kind: image
    description: A coast.
  poster:
    render: {with: $here/project/draw.py}
    kind: image
    description: <coast.1> as a poster.
YAML
J '[.assets[] | select(.name == "coast.1" or .name == "coast.2") | .kind] == ["image", "image"]' <<< "$(status "$T")" || check "series: revisions are not assets: $(status "$T" | jq -c '[.assets[].name]')"
render "$T" all || check "series: render all: $(tail -c 400 "$OUT/render.txt")"
given() { cat "$T/$(jq -r --arg s "$1" '.files[.assets[$s].versions[-1].files.given].path' "$T/project.lock")"; }
J '.writes == ["coast.1", "coast.2"] and .at == [1, 3]' <<< "$(given 'long take')" || check "series: long take's writes are not its beats', on the shot's clock: $(given 'long take')"
J '.prompt | contains("coast.2=") and contains("look=")' <<< "$(given next)" || check "series: next does not read <coast.2>: $(given next)"
take=$(jq -c '.assets["long take"].versions[-1].writes | keys' "$T/project.lock")
[ "$take" = '["coast.1","coast.2"]' ] || check "series: long take's take holds $take"
[ "$(made "$T" coast.1)" = null ] || [ "$(made "$T" coast.1)" = 0 ] || check "series: a sent revision has versions of its own"
[ "$(row "$T" coast.1 .status)" = '"written by long_take 1"' ] || check "series: coast.1 is $(row "$T" coast.1 .status)"
J --arg want "$(jq -r '.assets["long take"].versions[-1].writes["coast.2"].image' "$T/project.lock")" '.assets["next"].versions[-1].reads[] | select(.name == "coast.2") | .files.image == $want' "$T/project.lock" \
  || check "series: next's <coast.2> is not what long take wrote: $(jq -c '.assets["next"].versions[-1].reads' "$T/project.lock")"
J --arg want "$(jq -r '.assets["long take"].versions[-1].writes["coast.1"].image' "$T/project.lock")" '.assets.poster.versions[-1].reads[] | select(.name == "coast.1") | .files.image == $want' "$T/project.lock" \
  || check "series: poster's <coast.1> is not that revision"
J '.assets["first"].versions[-1].reads | map(.name) | index("coast")' "$T/project.lock" || check "series: first does not read coast"
[ "$(jq -r '.assets["first"].versions[-1].reads[] | select(.name == "coast") | .files.image' "$T/project.lock")" = "$(jq -r '.assets.coast.versions[-1].files.image' "$T/project.lock")" ] || check "series: first's <coast> is not coast itself"

step "a project: a free frame, moved, re-renders nothing"
J '.assets["harbor.1"].versions[-1] | .made_by.with == "cozy/frame" and (.files.image != null)' "$T/project.lock" || check "series: harbor.1 is not a free frame: $(jq -c '.assets["harbor.1"].versions[-1].made_by' "$T/project.lock")"
frames=$(runs "$T")
local was=$(row "$T" 'long_take' .inputs)
sed -i '/image: harbor.1/{n;s/at: 1/at: 2/}' "$T/project.yaml"
[ "$(row "$T" 'long_take' .inputs)" = "$was" ] || check "series: moving a free write moved its shot's key"
render "$T" 'harbor.1' || check "series: render a free frame: $(tail -c 300 "$OUT/render.txt")"
says series "harbor.1: version"
J '.assets["harbor.1"].versions | length == 1' "$T/project.lock" || check "series: a tool kept its old version: $(jq -c '.assets["harbor.1"].versions | length' "$T/project.lock")"
J '.assets["harbor.1"].versions[-1].digests.at != null' "$T/project.lock" || check "series: a frame's key does not hold its time"
[ "$(runs "$T")" = "$frames" ] || check "series: moving a free frame ran something"

step "a project: pinning a sent write puts its shot out of date; a write past its beat warns"
local pic; still "$T/pic.png" navy; pic=$(prefix "$T" "$T/pic.png")
sed -i "/^  poster:\$/i\\  coast.2:\\n    image: $pic" "$T/project.yaml"
case "$(row "$T" 'long_take' .status)" in *'out of date'*) ;; *) check "series: a pinned sent write did not put its shot out of date: $(row "$T" 'long_take' .status)" ;; esac
J '.assets[] | select(.name == "coast.2") | .pinned' <<< "$(status "$T")" || check "series: coast.2 is not pinned"
sed -i '/^  coast.2:$/,+1d' "$T/project.yaml"
sed -i '0,/at: 1$/s//at: 40/' "$T/project.yaml"
J '.warnings | any(contains("at 40 s into a beat 4 s long"))' <<< "$(status "$T")" || check "series: a write past its beat did not warn: $(status "$T" | jq -c .warnings)"
sed -i '0,/image: harbor.1/s//&\n            description: Not a field of a write./' "$T/project.yaml"
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
  moves:
    kind: video
    render: {with: $here/project/shots.py}
    bare: true
    plate: <look>
    summary: It walks.
    beats:
      - duration: 4
        description: The wall turns.
        writes:
          - image: wall.1
            at: 3
      - duration: 2
        description: It stands by <wall.1>.
  look:
    video: $look
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
defaults:
  audio:
    with: $here/project/shots.py
    a: 1
    b: from defaults
assets:
  same:
    kind: video
    render: {with: $here/project/shots.py}
    no_run: true
    summary: Already <look>.
  look:
    video: $look
  checked:
    render: {with: $here/project/shots.py}
    kind: image
    summary: Some words.
    no_such_field: Nothing reads this.
  both:
    render: {with: $here/project/shots.py}
    kind: video
    two_calls: true
    summary: One then the other.
  told:
    kind: audio
    render:
      b: its own
      c: 3
  told_alone:
    render:
      with: $here/project/shots.py
      c: 3
    kind: audio
YAML
render "$T" same || check "adapters: no run: $(tail -c 400 "$OUT/render.txt")"
J '.assets.same.versions[0] | (.runs // [] | length == 0) and .files.video != null' "$T/project.lock" || check "adapters: None is not what it reads, with no run: $(jq -c '.assets.same.versions' "$T/project.lock")"
[ "$(jq -r .assets.same.versions[0].files.video "$T/project.lock")" = "$(row "$T" look '.files.video.sha256' | tr -d '"')" ] || check "adapters: None's files are not what it reads"
render "$T" same || check "adapters: no run, again"
says adapters "has nothing to make"
[ "$(made "$T" same)" = 1 ] || check "adapters: a second version of what is its reads"
render "$T" checked || check "adapters: typed: $(tail -c 400 "$OUT/render.txt")"
says adapters "reads no field no_such_field"
step "a project: options go to the adapter, the asset's over its defaults', and one naming its own with takes none"
render "$T" told told_alone || check "adapters: options: $(tail -c 400 "$OUT/render.txt")"
heard() { cat "$T/$(jq -r --arg s "$1" '.files[.assets[$s].versions[-1].files.given].path' "$T/project.lock")"; }
[ "$(heard told | jq -c .)" = '{"a":1,"b":"its own","c":3}' ] || check "adapters: told's options: $(heard told)"
[ "$(heard 'told alone' | jq -c .)" = '{"c":3}' ] || check "adapters: told alone took options from defaults: $(heard 'told alone')"
render "$T" both || check "adapters: two calls: $(tail -c 400 "$OUT/render.txt")"
J '.assets.both.versions | length == 1 and (.[0] | (.runs | length == 2) and (.invocations | length == 2) and .files.first != null and .files.second != null)' "$T/project.lock" || check "adapters: two calls are not one version: $(jq -c '.assets.both.versions' "$T/project.lock")"
[ "$(cat "$T/$(jq -r '.files[.assets.both.versions[0].files.second].path' "$T/project.lock")")" = "$(jq -r '.assets.both.versions[0].files.first' "$T/project.lock")" ] || check "adapters: the second call was not given the first's output"
}

# Runs that fail on purpose, on a machine with no history to keep clean: never alone.
failures() {
step "a project: a piece ffprobe can't read fails its cut, and no film stands short of it"
local U=$OUT/unreadable look code=0
mkdir -p "$U"; plate "$U/look.mp4" testsrc2 440 800 4
printf 'version: 2\nassets: {}\n' > "$U/project.yaml"
look=$(prefix "$U" "$U/look.mp4")
cat > "$U/project.yaml" <<YAML
version: 2
assets:
  look:
    video: $look
  good:
    kind: video
    render: {with: $here/project/shots.py}
    fresh: true
    summary: Dusk on <look>.
  bad:
    kind: video
    render: {with: $here/project/shots.py}
    noise: true
    summary: Dusk on <look>.
  film:
    kind: video
    render: {with: cozy}
    pieces:
      - <good>
      - <bad>
YAML
cozy project -p "$U" render all --on local > "$OUT/render.txt" 2>&1 || code=$?
[ "$code" != 0 ] || check "unreadable: render exited 0"
says unreadable "film cannot render: bad: "
says unreadable "holds no video ffprobe reads"
[ ! -e "$U/film.mp4" ] || check "unreadable: a film was made without bad"
step "a project: a failed run is no version, and its seed is kept for the next try"
local T=$OUT/failing look seed
mkdir -p "$T"; plate "$T/look.mp4" testsrc2 440 800
printf 'version: 2\nassets: {}\n' > "$T/project.yaml"
look=$(prefix "$T" "$T/look.mp4")
cat > "$T/project.yaml" <<YAML
version: 2
assets:
  no:
    kind: video
    render: {with: $here/project/shots.py}
    refuse: Not this one.
    summary: <look>.
  reader:
    kind: video
    render: {with: $here/project/shots.py}
    summary: <no> and <look>.
  look:
    video: $look
YAML
code=0; render "$T" reader || code=$?
{ [ "$code" != 0 ] && says failures "Not this one."; } || check "failures: Refuse (exit $code): $(tail -c 400 "$OUT/render.txt")"
[ "$(made "$T" no)" = null ] || [ "$(made "$T" no)" = 0 ] || check "failures: a refusal made a version"
says failures "reader cannot render: no failed in this render"
stops
failed_once
absent_piece
rename_waits
[ -d "${PACKAGES:-}/minimax-h3" ] || return 0
step "a project: H3's adapter refuses to keep another package's take"
local H=$OUT/refused; mkdir -p "$H"; plate "$H/look.mp4" testsrc2 440 800
printf 'version: 2\nassets: {}\n' > "$H/project.yaml"
look=$(prefix "$H" "$H/look.mp4")
cat > "$H/project.yaml" <<YAML
version: 2
assets:
  clash:
    kind: video
    render: {with: $here/project/shots.py}
    summary: <look>.
    beats:
      - duration: 6
  look:
    video: $look
YAML
render "$H" clash || check "refused: the first take: $(tail -c 300 "$OUT/render.txt")"
sed -i "s#with: .*shots.py#with: $(cd "$PACKAGES" && pwd)/minimax-h3#" "$H/project.yaml"
code=0; render "$H" clash --take 1 --in 5 || code=$?
{ [ "$code" != 0 ] && says refused "has no latents this package can read"; } || check "refused: another package's take (exit $code): $(tail -c 300 "$OUT/render.txt")"
}

# A film short of its pieces, as a lock from 0.18.5 can hold one, isn't its state: any render cuts it whole, with its pieces as written.
# Naming a cut cuts it again.
short_film_heals() {
step "a project: a short film in the lock repairs itself, and naming a cut cuts it"
local T=$OUT/healing a b short version
mkdir -p "$T"
plate "$T/a.mp4" testsrc2 440 800; plate "$T/b.mp4" smptebars 660 800; plate "$T/short.mp4" testsrc2 440 800 2
printf 'version: 2\nassets: {}\n' > "$T/project.yaml"
a=$(prefix "$T" "$T/a.mp4"); b=$(prefix "$T" "$T/b.mp4"); short=$(prefix "$T" "$T/short.mp4")
cat > "$T/project.yaml" <<YAML
version: 2
assets:
  a: {video: $a}
  b: {video: $b}
  short_film: {video: $short}
  edit:
    kind: video
    render: {with: cozy}
    pieces:
      - <a>
      - <b>
YAML
render "$T" edit || check "healing: first cut: $(tail -c 400 "$OUT/render.txt")"
J '.cuts[0].frames == 288' <<< "$(status "$T")" || check "healing: the whole film: $(status "$T" | jq -c .cuts)"
short=$(jq -r --arg p "$short" '.files | keys[] | select(contains($p))' "$T/project.lock")
jq --arg s "$short" '.assets.edit.versions[-1].files.video = $s' "$T/project.lock" > "$T/lock.tmp" && mv "$T/lock.tmp" "$T/project.lock"
rm -f "$T/edit.mp4"; cp "$T/short.mp4" "$T/edit.mp4"
[ "$(row "$T" edit .standing)" = '"out_of_date"' ] || check "healing: a short film stands as its state: $(row "$T" edit .status)"
render "$T" all --missing || check "healing: render: $(tail -c 400 "$OUT/render.txt")"
J '.cuts[0].frames == 288' <<< "$(status "$T")" || check "healing: not whole after a render: $(status "$T" | jq -c .cuts)"
[ "$(row "$T" edit .standing)" = '"current"' ] || check "healing: not current: $(row "$T" edit .status)"
n=$(ffprobe -v error -count_frames -select_streams v:0 -show_entries stream=nb_read_frames -of csv=p=0 "$T/edit.mp4"); [ "$n" = 288 ] || check "healing: edit.mp4 is really $n frames"
version=$(jq '.assets.edit.versions[-1].number' "$T/project.lock")
render "$T" edit || check "healing: naming it: $(tail -c 400 "$OUT/render.txt")"
says healing "(unchanged)"
[ "$(jq '.assets.edit.versions[-1].number' "$T/project.lock")" = "$version" ] || check "healing: the same bytes made a new version: $(tail -c 400 "$OUT/render.txt")"
}

# Takes cut in the walk that lands them, new bytes nothing has measured. First in the project part: on a new computer this is also
# what fetches Calcifer's ffprobe.
cut_of_takes() {
step "a project: a cut holds every take landed in its walk"
local T=$OUT/landed a b n
mkdir -p "$T/plates"
plate "$T/plates/a.mp4" testsrc2 440 800 4; plate "$T/plates/b.mp4" testsrc 550 800 4
printf 'version: 2\nassets: {}\n' > "$T/project.yaml"
a=$(prefix "$T" "$T/plates/a.mp4") b=$(prefix "$T" "$T/plates/b.mp4")
cat > "$T/project.yaml" <<YAML
version: 2
assets:
  plate_a:
    video: $a
  plate_b:
    video: $b
  one:
    kind: video
    render: {with: $here/project/shots.py}
    fresh: true
    summary: Dusk on <plate_a>.
  two:
    kind: video
    render: {with: $here/project/shots.py}
    fresh: true
    summary: Dawn on <plate_b>.
  film:
    kind: video
    render: {with: cozy}
    fps: 24
    pieces:
      - <one>
      - <two>
YAML
filmed() {
  n=$(ffprobe -v error -count_frames -select_streams v:0 -show_entries stream=nb_read_frames -of csv=p=0 "$T/film.mp4" 2>/dev/null) || n=
  [ "$n" = 192 ] || check "landed: $1: film.mp4 is $n frames, not 192: $(grep -F 'left out' "$OUT/render.txt" | cut -c1-200)"
  ! grep -qF 'left out' "$OUT/render.txt" || check "landed: $1: a piece was left out: $(grep -F 'left out' "$OUT/render.txt" | cut -c1-200)"
  J --arg want "$(jq -r '.assets.two.versions[-1].files.video' "$T/project.lock")" '.assets.film.versions[-1].reads[] | select(.name == "two") | .files.video == $want' "$T/project.lock" \
    || check "landed: $1: film does not read two's newest take"
}
render "$T" all || check "landed: render all: $(tail -c 400 "$OUT/render.txt")"
filmed "render all"
render "$T" two || check "landed: render two: $(tail -c 400 "$OUT/render.txt")"
filmed "a new take of the last piece"
}

# Eight shots a machine fails alike: the second failure stops what is sent to it, and the render ends on how far it got.
stops() {
step "a project: runs failing alike stop at the second, and the last line says how far it got"
local T=$OUT/stops code=0 name failed
mkdir -p "$T"
printf 'version: 2\nassets:\n' > "$T/project.yaml"
for name in a b c d e f g h; do
  printf '  %s:\n    kind: video\n    render: {with: %s/project/shots.py}\n    crash: the machine is broken\n' "$name" "$here" >> "$T/project.yaml"
done
render "$T" all || code=$?
[ "$code" != 0 ] || check "stops: render exited 0: $(tail -c 400 "$OUT/render.txt")"
says stops "runs failed the same way, so this render sends it nothing more"
says stops "rendered 0 of 8; "
says stops "cozy project render all --missing --on local"
failed=$(cozy --json run ls -p "$T" --limit 0 --state failed | jq '.runs | length')
[ "$failed" -lt 8 ] || check "stops: $failed of 8 runs ran: $(tail -c 400 "$OUT/render.txt")"
}

# A shot that fails on a new project, read by another: the walk doesn't send it again, one row says so, and what reads it says why. A
# render the person asks again is a new attempt.
failed_once() {
step "a project: a failed take isn't sent again by the walk; asked again, it is"
local T=$OUT/failed-once look code=0 tried
mkdir -p "$T"; plate "$T/look.mp4" testsrc2 440 800 2
printf 'version: 2\nassets: {}\n' > "$T/project.yaml"
look=$(prefix "$T" "$T/look.mp4")
cat > "$T/project.yaml" <<YAML
version: 2
assets:
  look:
    video: $look
  broken:
    kind: video
    render: {with: $here/project/shots.py}
    summary: <look>.
    crash: the card ran out of memory
  reader:
    kind: video
    render: {with: $here/project/shots.py}
    summary: <look> after <broken>.
YAML
tried() { cozy --json run ls -p "$T" --limit 0 --state failed | jq '[.runs[] | select(.target | endswith("/crash"))] | length'; }
render "$T" all || code=$?
[ "$code" != 0 ] || check "failed once: render exited 0: $(tail -c 400 "$OUT/render.txt")"
[ "$(tried)" = 1 ] || check "failed once: the walk sent it $(tried) times: $(tail -c 600 "$OUT/render.txt")"
J '[.board.jobs[] | select(.asset == "broken")] | length == 1 and .[0].state == "failed" and .[0].number == 1' <<< "$(status "$T")" || check "failed once: its rows: $(status "$T" | jq -c '.board.jobs | map({asset, state, number})')"
J '[.board.jobs[] | select(.asset == "reader")] | length == 1 and .[0].state == "failed"' <<< "$(status "$T")" || check "failed once: what reads it: $(status "$T" | jq -c '.board.jobs | map({asset, state})')"
says "failed once" "reader cannot render: broken failed in this render"
says "failed once" "rendered 0 of 2; 2 failed"
! grep -q 'no take yet' "$OUT/render.txt" || check "failed once: it went on to send it: $(tail -c 400 "$OUT/render.txt")"
code=0; render "$T" broken || code=$?
{ [ "$code" != 0 ] && [ "$(tried)" = 2 ]; } || check "failed once: asked again, exit $code, $(tried) runs"
J '[.board.jobs[] | select(.asset == "broken")] | length == 1 and .[0].number == 2' <<< "$(status "$T")" || check "failed once: asked again, its rows: $(status "$T" | jq -c '.board.jobs | map({asset, state, number})')"
}

# A piece gone from this computer, and at no machine or Hub, is left out with a warning (Paul, 11-04): the film says so, the one before stays a
# version, and once the piece is back the next render cuts it whole. So with a piece that has no version yet.
frames_of() { ffprobe -v error -count_frames -select_streams v:0 -show_entries stream=nb_read_frames -of csv=p=0 "$1"; }
absent_piece() {
step "a project: a piece that can't be had is left out, said, and cut back in when it's here"
local T=$OUT/absent a b first stored
mkdir -p "$T"
plate "$T/a.mp4" testsrc2 440 800; plate "$T/b.mp4" smptebars 660 800
printf 'version: 2\nassets: {}\n' > "$T/project.yaml"
a=$(prefix "$T" "$T/a.mp4"); b=$(prefix "$T" "$T/b.mp4")
cat > "$T/project.yaml" <<YAML
version: 2
assets:
  a: {video: $a}
  b: {video: $b}
  film:
    kind: video
    render: {with: cozy}
    pieces: [<a>, <b>]
YAML
render "$T" film || check "absent: first cut: $(tail -c 400 "$OUT/render.txt")"
first=$(sha "$T/film.mp4")
rm -f "$T"/files/"$b"*
sed -i 's/pieces: \[<a>, <b>\]/pieces: [<a>, <b>, <a>]/' "$T/project.yaml"
render "$T" film || check "absent: a missing piece failed the render: $(tail -c 400 "$OUT/render.txt")"
says absent "film.mp4 is cut without b"
says absent "(without b: not on this computer)"
[ "$(frames_of "$T/film.mp4")" = 288 ] || check "absent: the film is $(frames_of "$T/film.mp4") frames, not the 288 of what it could have"
J '.cuts[0].without == ["b: not on this computer"]' <<< "$(status "$T")" || check "absent: status: $(status "$T" | jq -c .cuts)"
cozy project -p "$T" status 2>&1 | grep -qF '(without b: not on this computer)' || check "absent: status doesn't say what it went without"
[ "$(made "$T" film)" = 2 ] || check "absent: the short film is no new version: $(made "$T" film)"
ls "$T/files/$first"* > /dev/null 2>&1 || check "absent: the longer film went"
prefix "$T" "$T/b.mp4" > /dev/null
render "$T" all --missing || check "absent: with it back: $(tail -c 400 "$OUT/render.txt")"
[ "$(frames_of "$T/film.mp4")" = 432 ] || check "absent: with b back the film is $(frames_of "$T/film.mp4") frames, not 432: $(tail -c 400 "$OUT/render.txt")"
J '(.cuts[0].without // []) == []' <<< "$(status "$T")" || check "absent: status with it back: $(status "$T" | jq -c .cuts)"

step "a project: a piece whose bytes aren't its hash is left out"
stored=$(jq -r --arg a "$a" '.files | to_entries[] | select(.key | contains($a)) | .value.path' "$T/project.lock")
chmod u+w "$T/$stored"; printf 'changed' >> "$T/$stored"
render "$T" film || check "absent: a changed piece failed the render: $(tail -c 400 "$OUT/render.txt")"
says absent "(without a: its bytes changed)"
[ "$(frames_of "$T/film.mp4")" = 144 ] || check "absent: a changed piece was cut: $(frames_of "$T/film.mp4") frames"

step "a project: a piece with no version yet is left out, and its landing cuts it in"
local P=$OUT/arriving
mkdir -p "$P"
plate "$P/a.mp4" testsrc2 440 800
printf 'version: 2\nassets: {}\n' > "$P/project.yaml"
a=$(prefix "$P" "$P/a.mp4")
cat > "$P/project.yaml" <<YAML
version: 2
assets:
  a: {video: $a}
  later:
    kind: video
    render: {with: $here/project/shots.py}
    summary: Dusk on <a>.
  film:
    kind: video
    render: {with: cozy}
    pieces: [<a>, <later>]
YAML
render "$P" film || check "arriving: $(tail -c 400 "$OUT/render.txt")"
says arriving "(without later: no version yet)"
[ "$(frames_of "$P/film.mp4")" = 144 ] || check "arriving: the film is $(frames_of "$P/film.mp4") frames"
render "$P" later || check "arriving: later: $(tail -c 400 "$OUT/render.txt")"
[ "$(frames_of "$P/film.mp4")" = 288 ] || check "arriving: the film didn't cut it in: $(frames_of "$P/film.mp4") frames: $(tail -c 400 "$OUT/render.txt")"
! grep -qF 'without later' "$OUT/render.txt" || check "arriving: still says without: $(tail -c 400 "$OUT/render.txt")"
}

# Status and the closing line say what the film holds, not what its pieces add up to; a piece with no version yet is warned of.
film_is_its_file() {
step "a project: a film's length is its file's, and a piece with no version warns"
local T=$OUT/truth a
mkdir -p "$T"
plate "$T/a.mp4" testsrc2 440 800
plate "$T/short.mp4" testsrc2 440 800 1
printf 'version: 2\nassets: {}\n' > "$T/project.yaml"
a=$(prefix "$T" "$T/a.mp4")
cat > "$T/project.yaml" <<YAML
version: 2
assets:
  a: {video: $a}
  later:
    kind: video
    render: {with: $here/project/shots.py}
    no_run: true
  film:
    kind: video
    render: {with: cozy}
    pieces: [<a>, <later>]
YAML
render "$T" film || check "truth: cut: $(tail -c 400 "$OUT/render.txt")"
says truth "warning"
says truth "(without later: no version yet)"
J '.cuts[0].frames == 144' <<< "$(status "$T")" || check "truth: status of the film: $(status "$T" | jq -c .cuts)"
[ "$(stat -c %h "$T/film.mp4")" = 1 ] || check "truth: film.mp4 is a hard link: $(stat -c %h "$T/film.mp4") names"
local stored
stored=$(jq -r '.files[.assets.film.versions[-1].files.video].path' "$T/project.lock")
[ "$(id -u)" = 0 ] || [ ! -w "$T/$stored" ] || check "truth: $stored can be written"
cp "$T/short.mp4" "$T/film.mp4"
J '.cuts[0].frames == 24' <<< "$(status "$T")" || check "truth: status doesn't read the file: $(status "$T" | jq -c .cuts)"
[ "$(sha "$T/$stored")" = "$(basename "${stored%.*}")" ] || check "truth: writing over film.mp4 changed $stored"
}

# A cut's versions are all kept and listed; approving an older one brings it and its timeline to the root, and unpinning goes back.
cut_versions() {
step "a project: a cut keeps its versions, and the film follows the state"
local T=$OUT/edits a b one first_timeline two
mkdir -p "$T"
plate "$T/a.mp4" testsrc2 440 800; plate "$T/b.mp4" smptebars 660 800
printf 'version: 2\nassets: {}\n' > "$T/project.yaml"
a=$(prefix "$T" "$T/a.mp4"); b=$(prefix "$T" "$T/b.mp4")
cat > "$T/project.yaml" <<YAML
version: 2
assets:
  a: {video: $a}
  b: {video: $b}
  edit:
    kind: video
    render: {with: cozy}
    pieces:
      - <a>
YAML
render "$T" edit || check "edits: first cut: $(tail -c 400 "$OUT/render.txt")"
one=$(sha "$T/edit.mp4"); first_timeline=$(sha "$T/edit.otio")
sed -i 's/^      - <a>$/      - <a>\n      - <b>/' "$T/project.yaml"
render "$T" edit || check "edits: second cut: $(tail -c 400 "$OUT/render.txt")"
two=$(sha "$T/edit.mp4")
[ "$one" != "$two" ] || check "edits: the second cut is the first"
J '.assets[] | select(.name == "edit") | (.prefixes | length) == 2 and (.prefixes[0].from == ["a"]) and (.prefixes[1].from == ["a", "b"])' <<< "$(status "$T")" || check "edits: status: $(row "$T" edit .prefixes)"
ls "$T/files/$one"* > /dev/null 2>&1 || check "edits: the first film is gone from files/"
render "$T" edit || check "edits: naming it: $(tail -c 400 "$OUT/render.txt")"
says edits "(unchanged)"
[ "$(made "$T" edit)" = 2 ] || check "edits: the same pieces made a version: $(made "$T" edit)"
cozy project -p "$T" approve edit "$(jq '.assets.edit.versions[0].number' "$T/project.lock")" > "$OUT/edits-approve.txt" 2>&1 || check "edits: approve: $(tail -c 300 "$OUT/edits-approve.txt")"
[ "$(sha "$T/edit.mp4")" = "$one" ] || check "edits: edit.mp4 isn't the approved film: $(tail -c 300 "$OUT/edits-approve.txt")"
[ "$(sha "$T/edit.otio")" = "$first_timeline" ] || check "edits: edit.otio isn't the approved film's"
cozy project -p "$T" unpin edit > "$OUT/edits-unpin.txt" 2>&1 || check "edits: unpin: $(tail -c 300 "$OUT/edits-unpin.txt")"
[ "$(sha "$T/edit.mp4")" = "$two" ] || check "edits: edit.mp4 didn't go back to the state: $(tail -c 300 "$OUT/edits-unpin.txt")"
[ "$(made "$T" edit)" = 2 ] || check "edits: choosing made a version: $(made "$T" edit)"
}

# A rename asked while a render goes happens as it ends.
rename_waits() {
step "a project: a rename during a render waits for it"
local T=$OUT/renaming going n
mkdir -p "$T"
cat > "$T/project.yaml" <<YAML
version: 2
assets:
  old:
    kind: video
    render: {with: $here/project/shots.py}
    seconds: 0
  busy:
    kind: video
    render: {with: $here/project/shots.py}
    seconds: 12
YAML
render "$T" old || check "rename: first render: $(tail -c 400 "$OUT/render.txt")"
cozy project -p "$T" render busy --on local > "$OUT/rename-busy.txt" 2>&1 &
going=$!
n=''
for _ in $(seq 120); do n=$(row "$T" busy .status | sed -n 's/.*making (run \([0-9]*\)).*/\1/p'); [ -n "$n" ] && break; sleep 0.25; done
[ -n "$n" ] || check "rename: the render never showed a run: $(tail -c 300 "$OUT/rename-busy.txt")"
sed -i 's/^  old:/  new:/' "$T/project.yaml"
cozy project -p "$T" rename old new > "$OUT/rename.txt" 2>&1 || check "rename: refused during a render: $(tail -c 300 "$OUT/rename.txt")"
grep -q 'when the render going ends' "$OUT/rename.txt" || check "rename: does not say it waits: $(tail -c 300 "$OUT/rename.txt")"
wait "$going" || check "rename: the render: $(tail -c 300 "$OUT/rename-busy.txt")"
[ "$(made "$T" new)" = 1 ] || check "rename: new has $(made "$T" new) versions after the render ended"
}

# Three assets failing alike, a package that isn't there, so no run is made.
said_once() {
step "a project: a failure several assets share is said once"
local T=$OUT/once code=0
mkdir -p "$T"
cat > "$T/project.yaml" <<YAML
version: 2
assets:
  one:
    kind: image
    render: {with: $T/missing.py}
    description: A coast.
  two:
    kind: image
    render: {with: $T/missing.py}
    description: A harbor.
  three:
    kind: image
    render: {with: $T/missing.py}
    description: A wall.
YAML
render "$T" all || code=$?
[ "$code" != 0 ] || check "once: render exited 0"
[ "$(grep -v render_incomplete "$OUT/render.txt" | grep -c 'pyproject.toml')" = 1 ] || check "once: not said once: $(tail -c 400 "$OUT/render.txt")"
says once "rendered 0 of 3; 3 failed"
says once "cozy project render all --missing --on local"
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
    render: {with: cozy}
    kind: video
    fps: 24
    pieces:
      - {video: <a>, start: 7, end: 151}
      - {video: <b>, start: 13, end: 140}
      - {video: <a>, start: 48, end: 96}
      - {video: <b>, start: 150, end: 161}
  trailer:
    render: {with: cozy}
    kind: video
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
render "$T" all --missing || check "cuts: render all again: $(tail -c 300 "$OUT/render.txt")"
says cuts "(unchanged)"
[ "$(stat -c %Y%N "$T/edit.mp4" 2>/dev/null || stat -c %Y "$T/edit.mp4")" = "$made_at" ] || check "cuts: same takes, cut again"
J '.assets.edit.versions | length == 1' "$T/project.lock" || check "cuts: a cut kept its old version"
cp "$T/edit.otio" "$OUT/edit.otio.was"; rm -f "$T/edit.otio"
render "$T" all --missing || check "cuts: with its .otio deleted: $(tail -c 300 "$OUT/render.txt")"
cmp -s "$T/edit.otio" "$OUT/edit.otio.was" || check "cuts: a deleted .otio did not come back the same"
[ "$(stat -c %Y%N "$T/edit.mp4" 2>/dev/null || stat -c %Y "$T/edit.mp4")" = "$made_at" ] || check "cuts: a deleted .otio cut again"

step "a project: a take at 25 fps, retimed; a take that isn't a piece any more"
sed -i "s/^    video: $B\$/    video: $R/" "$T/project.yaml"
render "$T" all --missing || check "cuts: with r: $(tail -c 300 "$OUT/render.txt")"
n=$(ffprobe -v error -count_frames -select_streams v:0 -show_entries stream=nb_read_frames,r_frame_rate -of csv=p=0 "$T/edit.mp4" 2>/dev/null) || n=
[ "$n" = "24/1,330" ] || check "cuts: the retimed edit.mp4 is $n, not 330 frames at 24/1"
says cuts "it retimes b (25 fps) to 24 fps"
"${FR[@]}" "$T/edit.mp4" 24 "$T/plates/a.mp4:7:151" "$T/plates/r.mp4:13:140" "$T/plates/a.mp4:48:96" "$T/plates/r.mp4:150:161" > "$OUT/frames-2.txt" 2>&1 || true
tail -1 "$OUT/frames-2.txt" | grep -q 'frames PASS' || check "cuts: the retimed cut's frames are not their sources': $(grep FAIL "$OUT/frames-2.txt" | head -3 | paste -sd' ')"
sed -i "s/^    video: $R\$/    video: $B/" "$T/project.yaml"
render "$T" all --missing || check "cuts: b back: $(tail -c 300 "$OUT/render.txt")"
[ "$(sha "$T/edit.mp4")" = "$first" ] || check "cuts: the first edit again is another file"

step "a project: approve recuts here, and a tool reads a newest out of date, and says so"
local S=$OUT/recut; mkdir -p "$S"; plate "$S/look.mp4" testsrc2 440 800 3
printf 'version: 2\nassets: {}\n' > "$S/project.yaml"
A=$(prefix "$S" "$S/look.mp4")
cat > "$S/project.yaml" <<YAML
version: 2
assets:
  shot:
    kind: video
    render: {with: $here/project/shots.py}
    bare: true
    plate: <look>
    summary: One.
  look:
    video: $A
  edit:
    render: {with: cozy}
    kind: video
    pieces:
      - <shot>
YAML
render "$S" all || check "recut: render all: $(tail -c 300 "$OUT/render.txt")"
[ -s "$S/edit.mp4" ] || check "recut: no edit.mp4"
sed -i 's/summary: One\./summary: Two./; s/^    pieces:$/    fps: 30\n&/' "$S/project.yaml"
render "$S" look --missing || check "recut: render look"
says recut "which is out of date"
cozy project -p "$S" approve shot 1 > "$OUT/approve.txt" 2>&1 || check "recut: approve: $(tail -c 300 "$OUT/approve.txt")"
[ -s "$S/edit.mp4" ] || check "recut: approve lost edit.mp4"
rm "$S/edit.mp4"
render "$S" look --missing || check "recut: render look again"
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
    render: {with: cozy}
    kind: video
    aspect_ratio: "16:9"
    pieces:
      - {video: <a>, start: 0, end: 48}
      - {video: <b>, start: 24, end: 48}
  whole:
    render: {with: cozy}
    kind: video
    aspect_ratio: "16:9"
    pieces:
      - <a>
      - <b>
YAML
render "$W" all || check "rounded: render all: $(tail -c 300 "$OUT/render.txt")"
size() { ffprobe -v error -select_streams v:0 -show_entries stream=width,height -of csv=p=0 "$1"; }
[ "$(size "$W/edit.mp4")" = 1344,768 ] || check "rounded: edit.mp4 is $(size "$W/edit.mp4"), not its takes' 1344,768"
md5s() { ffmpeg -v error -i "$1" -map 0:v -f framemd5 - | grep -v '^#' | awk '{print $NF}'; }
[ "$( (md5s "$W/a.mp4" | sed -n 1,48p; md5s "$W/b.mp4" | sed -n 25,48p) | md5sum)" = "$(md5s "$W/edit.mp4" | md5sum)" ] || check "rounded: edit.mp4's frames are not its takes'"
[ "$( (md5s "$W/a.mp4"; md5s "$W/b.mp4") | md5sum)" = "$(md5s "$W/whole.mp4" | md5sum)" ] || check "rounded: whole.mp4's frames are not its whole takes'"
! ls -1 "$CACHE" | grep -qE "^($(sha "$W/a.mp4" | cut -c1-16)|$(sha "$W/b.mp4" | cut -c1-16))-" || check "rounded: an edge of a whole take was encoded"
sed -i "s/^    video: $B\$/    video: $R/" "$W/project.yaml"
render "$W" all --missing || check "rounded: with an 854x480 plate: $(tail -c 300 "$OUT/render.txt")"
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
    render: {with: cozy}
    kind: video
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
cozy project -p "$U" import "$U/kyogre.png" > "$OUT/import-first.txt" 2>&1 || check "import: a file: $(tail -c 300 "$OUT/import-first.txt")"
cozy project -p "$U" import "$U/kyogre.png" > "$OUT/import-again.txt" 2>&1 || check "import: again: $(tail -c 300 "$OUT/import-again.txt")"
grep -q '^already imported ' "$OUT/import-again.txt" || check "import: a second import does not say so: $(cat "$OUT/import-again.txt")"

step "a project: import --remove-background"
local clear runs_before line cleaned original
ffmpeg -v error -y -f lavfi -i "color=c=black@0.0:size=64x64,format=rgba" -frames:v 1 "$U/clear.png"
runs_before=$(jq '.runs // [] | length' "$U/project.lock")
cozy project -p "$U" import "$U/clear.png" --remove-background > "$OUT/import-clear.txt" 2> "$OUT/import-clear.err" || check "import: already transparent: $(tail -c 300 "$OUT/import-clear.err")"
grep -q 'already transparent' "$OUT/import-clear.err" || check "import: already transparent is not said: $(cat "$OUT/import-clear.err")"
[ "$(wc -l < "$OUT/import-clear.txt")" = 1 ] && grep -q "^image: $(sha "$U/clear.png" | cut -c1-12)\$" "$OUT/import-clear.txt" || check "import: already transparent printed: $(cat "$OUT/import-clear.txt")"
cozy project -p "$U" import "$U/kyogre.png" --remove-background --on no_such_machine > "$OUT/import-none.txt" 2> "$OUT/import-none.err" || check "import: no machine: $(tail -c 300 "$OUT/import-none.err")"
grep -q 'a plain copy is kept' "$OUT/import-none.err" || check "import: no machine is not said: $(cat "$OUT/import-none.err")"
grep -q "^image: $(sha "$U/kyogre.png" | cut -c1-12)\$" "$OUT/import-none.txt" || check "import: no machine printed: $(cat "$OUT/import-none.txt")"
[ "$(jq '.runs // [] | length' "$U/project.lock")" = "$runs_before" ] || check "import: a run with no machine, or of a transparent file"
if [ -n "${MODEL:-}" ]; then
  still "$U/blue.png" blue
  original=$(sha "$U/blue.png")
  cozy project -p "$U" import "$U/blue.png" --remove-background > "$OUT/import-clean.txt" 2> "$OUT/import-clean.err" || check "import: remove the background: $(tail -c 300 "$OUT/import-clean.err")"
  [ "$(wc -l < "$OUT/import-clean.txt")" = 1 ] || check "import: more than one line: $(cat "$OUT/import-clean.txt")"
  line=$(cat "$OUT/import-clean.txt"); cleaned=${line#image: }
  { [ "${#cleaned}" = 12 ] && [ "$cleaned" != "${original:0:12}" ] && ls "$U/files/$cleaned"* > /dev/null 2>&1; } || check "import: the cleaned file: $line"
  ! ls "$U/files/$original"* > /dev/null 2>&1 || check "import: the original was kept"
fi
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
yaml_routes "$P" "$ID"
}

# project.yaml over HTTP: read it, save over what was read, and never over a save made since.
yaml_routes() {
step "a project: project.yaml read and saved over HTTP"
local P=$1 U=http://$ADDR/v1/projects/$2/yaml et next
cp "$P/project.yaml" "$OUT/yaml-kept.yaml"
api -D "$OUT/yaml.hdr" -o "$OUT/yaml.got" "$U" || true
et=$(grep -i '^etag:' "$OUT/yaml.hdr" | cut -d' ' -f2 | tr -d '\r')
{ head -1 "$OUT/yaml.hdr" | grep -q ' 200' && cmp -s "$OUT/yaml.got" "$P/project.yaml" && [ -n "$et" ]; } || check "GET yaml: $(tr -d '\r' < "$OUT/yaml.hdr" | paste -sd' ')"
[ "$(api -o /dev/null -w '%{http_code}' -H "If-None-Match: $et" "$U")" = 304 ] || check "If-None-Match on yaml is not 304"
[ "$(api -o "$OUT/yaml-put.json" -w '%{http_code}' -X PUT --data-binary @"$OUT/yaml.got" "$U")" = 400 ] || check "a save with no If-Match was not refused 400"
{ printf '# A person wrote this in another editor.\n'; cat "$P/project.yaml"; } > "$OUT/yaml-theirs.yaml"
cp "$OUT/yaml-theirs.yaml" "$P/project.yaml"
{ printf '# Saved over HTTP.\n'; cat "$OUT/yaml.got"; } > "$OUT/yaml-mine.yaml"
[ "$(api -o "$OUT/yaml-put.json" -w '%{http_code}' -X PUT -H "If-Match: $et" --data-binary @"$OUT/yaml-mine.yaml" "$U")" = 409 ] || check "a save over a file changed since it was read was not 409: $(cat "$OUT/yaml-put.json")"
J '.error.code == "project_changed"' "$OUT/yaml-put.json" || check "the stale save's refusal: $(cat "$OUT/yaml-put.json")"
cmp -s "$P/project.yaml" "$OUT/yaml-theirs.yaml" || check "a stale save changed the file"
et=$(api -I "$U" | grep -i '^etag:' | cut -d' ' -f2 | tr -d '\r')
printf 'version: 2\nassets: [\n' > "$OUT/yaml-broken.yaml"
[ "$(api -o "$OUT/yaml-put.json" -w '%{http_code}' -X PUT -H "If-Match: $et" --data-binary @"$OUT/yaml-broken.yaml" "$U")" = 400 ] || check "text that isn't a project was not refused 400: $(cat "$OUT/yaml-put.json")"
cmp -s "$P/project.yaml" "$OUT/yaml-theirs.yaml" || check "a refused save changed the file"
{ printf '# Saved over HTTP.\n'; cat "$OUT/yaml-theirs.yaml"; } > "$OUT/yaml-mine.yaml"
[ "$(api -D "$OUT/yaml-put.hdr" -o /dev/null -w '%{http_code}' -X PUT -H "If-Match: $et" --data-binary @"$OUT/yaml-mine.yaml" "$U")" = 204 ] || check "a good save was not 204"
cmp -s "$P/project.yaml" "$OUT/yaml-mine.yaml" || check "the saved file isn't the bytes sent"
next=$(grep -i '^etag:' "$OUT/yaml-put.hdr" | cut -d' ' -f2 | tr -d '\r')
[ "$next" = "\"$(sha "$P/project.yaml")\"" ] || check "the save's ETag isn't its digest: $next"
cp "$OUT/yaml-kept.yaml" "$P/project.yaml"
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
