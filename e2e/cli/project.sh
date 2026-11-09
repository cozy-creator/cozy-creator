# e2e/cli/project.sh: a project on this computer through `cozy project` and Calcifer's HTTP surface (cl-758), the part cli/run.sh
# calls `projects`, with the local machine up. No model runs: pictures come from a weightless local function (project/draw.py), shots
# from project/shots.py, and plates from ffmpeg, so every path but a model's is real: the folder, the files, the lock, the walk, the
# call, the cut, the API a web UI uses and a browser on it.
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
sha() { { sha256sum "$1" 2> /dev/null || shasum -a 256 "$1"; } | cut -c1-64; }
J() { jq -e "$@" > /dev/null; }
# edited <file> <perl s///>: in place, on every OS: a Mac's sed takes neither -i alone nor a newline in what it writes.
edited() { perl -pi.was -e "$2" "$1"; rm -f "$1.was"; }
# yaml_in <project>: stdin as the project: its project.yaml while it's a folder no Calcifer has opened (its first open imports it, cl-803),
# else read in by `cozy project import`, one edit, as a hand's edit of its export.
yaml_in() {
  if [ -f "$1/project.db" ]; then
    cat > "$1.in.yaml" && cozy project -p "$1" import "$1.in.yaml" > "$OUT/yaml-in.txt" 2>&1 || check "import into $1: $(tail -c 300 "$OUT/yaml-in.txt")"
  else
    cat > "$1/project.yaml"
  fi
}
# hand <project> <perl s///>: its export edited and read back, as a hand edits a project now.
hand() { { cozy project -p "$1" export > "$1.hand.yaml" && edited "$1.hand.yaml" "$2" && yaml_in "$1" < "$1.hand.yaml"; } || check "a hand's edit of $1 failed"; }
# added <project>: stdin's boxes (lines under `assets:`) added by hand to its export, read back.
added() {
  { cozy project -p "$1" export | perl -0pe 's/^assets: \{\}\n?\z/assets:\n/m; $_ .= "assets:\n" unless /^assets:/m' > "$1.hand.yaml" && cat >> "$1.hand.yaml" && yaml_in "$1" < "$1.hand.yaml"; } || check "a hand's addition to $1 failed"
}
# db_sql <project> <sql>: statements run on its project.db past Calcifer, as another program would.
db_sql() {
  python3 - "$1/project.db" "$2" <<'PY'
import sqlite3, sys
conn = sqlite3.connect(sys.argv[1], isolation_level=None, timeout=30)
conn.executescript(sys.argv[2])
PY
}
# seq_of <project>: its revision, one more each commit.
seq_of() { sql1 "$1" "SELECT value FROM meta WHERE key = 'seq'"; }
# owed <project>: how many runs its walk has pending.
owed() { sql1 "$1" "SELECT count(*) FROM walk, json_each(walk.record, '\$.pending')"; }
# approved_in <project> <name>: the line its export writes for that box's `approved`.
approved_in() { cozy project -p "$1" export | awk -v box="  $2:" '$0 == box { in_it = 1; next } in_it && /^    approved: / { print; exit } /^  [^ ]/ { in_it = 0 }'; }
# dropped <project> <name>…: those boxes taken out of its export by hand, read back.
dropped() {
  local P=$1; shift
  { cozy project -p "$P" export | NAMES="$*" perl -ne 'BEGIN { %n = map { $_ => 1 } split / /, $ENV{NAMES} } if (/^  ([^ :]+):/ && $n{$1}) { $in = 1; next } $in = 0 if $in && /^ {0,2}\S/; print unless $in' > "$P.hand.yaml" && yaml_in "$P" < "$P.hand.yaml"; } || check "a hand's deletion from $P failed"
}
# db_replace <project> <from> <to>: each row of its versions with `from` in it says `to`, written past Calcifer, as a damaged file was.
db_replace() {
  python3 - "$1/project.db" "$2" "$3" <<'PY'
import sqlite3, sys
conn = sqlite3.connect(sys.argv[1])
conn.execute("UPDATE versions SET record = replace(record, ?, ?)", (sys.argv[2], sys.argv[3]))
conn.commit()
PY
}
# prefix <project> <file>: `cozy project import`, and what its export writes for it.
prefix() { (cd "$1" && cozy --json project -p "$1" import "$2" | jq -r '.sha256 | sub("sha256:"; "") | .[:12]') || check "import $2 failed"; }
# api <curl args…>: Calcifer, with the bearer of its lock record.
api() { curl -sS -H "Authorization: Bearer $TOKEN" "$@"; }
# ask <json>: POST /v1/runs under a new key, its answer on stdout; a local function's install is followed, then asked again under that key.
ask() {
  local key=idem-$RANDOM$RANDOM$RANDOM answer
  while :; do
    answer=$(api -X POST -H "Idempotency-Key: $key" -d "$1" "http://$ADDR/v1/runs")
    [ "$(jq -r '.preparing // false' <<< "$answer")" = true ] || break
    cozy --json run show "$(jq -r .run.number <<< "$answer")" > /dev/null 2>&1 || true
  done
  printf '%s\n' "$answer"
}
# ended <number>: the run at its end with its outputs landed, whole.
ended() { cozy --json run show "$1" > /dev/null 2>&1 || true; cozy --json --full run show "$1" 2> /dev/null; }
# status <project> [file]: its status as JSON.
# status <project>: what status says. Why it can't be read goes to the log, and an answer the CLI can't read is a failure of its own.
status() {
  cozy --json --full project -p "$1" status 2> "$OUT/status.err" && return 0
  local code=$?
  echo "status of $1: $(tail -c 300 "$OUT/status.err")" >&2
  grep -q answer_unreadable "$OUT/status.err" && check "status of $1: $(tail -c 300 "$OUT/status.err")" >&2
  return "$code"
}
# row <project> <asset> <jq>: that of its row in status.
row() { status "$1" | jq -c --arg n "$2" ".assets[] | select(.name == \$n) | $3"; }
# made <project> <asset>: how many versions the lock keeps.
made() { local n; n=$(held "$1" "$2" 2> /dev/null | jq '.versions | length'); echo "${n:-0}"; }
# held <project> <asset>: what `cozy project status <asset>` says of it, `.asset` and `.versions`. Why it can't goes to the log.
held() { cozy --json --full project -p "$1" status "$2" 2> "$OUT/held.err" || echo "status $2 of $1: $(tail -c 200 "$OUT/held.err")" >&2; }
# take_file <project> <asset> <role> [version index]: that version's file by its hash.
take_file() { held "$1" "$2" | jq -r --arg r "$3" --argjson v "${4:--1}" '.versions[$v].files[$r].sha256 // empty'; }
# counts <project>: how many versions each asset has.
counts() { status "$1" | jq -c '[.assets[] | {(.name): (.versions // 0)}] | add'; }
# runs <project>: every version's last run its lock records.
runs() { status "$1" | jq '[.assets[].prefixes[]?.run // empty] | length'; }
# asked <project> <asset> [version index]: what that version's last run was asked.
asked() { cozy --json --full run show "$(held "$1" "$2" | jq -r --argjson v "${3:--1}" '.versions[$v].runs[-1]')" --once | jq -c .request; }
# prompt <project> <asset>: the instructions its newest version's run was told.
prompt() { asked "$1" "$2" | jq -r .input.instructions.text; }
# read_of <project> <asset>: the names what it reads go by, as status says.
read_of() { row "$1" "$2" '[.reads[]?.name]'; }
# file_at <project> <sha256>: where that file is.
file_at() { ls "$1/files/${2#sha256:}"* 2> /dev/null | head -1; }
# render <project> <args…>: on this computer, its words in $OUT/render.txt.
# As a person renders. No fixture here has weights, so one that starts downloading them is a fault, stopped at once: a runner has no business
# fetching them, and on a rental the bytes bill. One whose output stops growing for 15 minutes has stalled, `step`'s rule in release-gate/sessions/lib.sh.
render() {
  local P=$1 pid last=-1 since=$SECONDS size why=''; shift
  : > "$OUT/render.txt"
  cozy project -p "$P" render "$@" ${LOCAL:+--on local} >> "$OUT/render.txt" 2>&1 & pid=$!
  while [ -z "$why" ] && kill -0 "$pid" 2> /dev/null; do
    sleep 0.2
    grep -q 'downloading .* weights' "$OUT/render.txt" && why='it started downloading weights'
    size=$(wc -c < "$OUT/render.txt")
    if [ "$size" != "$last" ]; then last=$size since=$SECONDS; fi
    [ $((SECONDS - since)) -lt 900 ] || why='its output stopped for 15 min'
  done
  if [ -n "$why" ]; then
    check "render $*: $why: $(tail -c 300 "$OUT/render.txt")"
    kill "$pid" 2> /dev/null || true
    cozy project -p "$P" cancel all > /dev/null 2>&1 || true
    wait "$pid" 2> /dev/null || true
    return 1
  fi
  wait "$pid"
}
says() { grep -qF -- "$2" "$OUT/render.txt" || check "$1: render does not say '$2': $(tail -c 400 "$OUT/render.txt")"; }

# What a project does with its own files on a computer with no machine (macOS, Windows: `cozy project render --on local` answers
# local_unavailable), the part cli/run.sh calls `project_files`. A film is a version nothing had to run for, so lock writes, renames and
# versions all go through here. A take from a function can't: draw.py and shots.py need a machine, so the render, walk, take and
# queue cells stay Linux's. After web_serves, whose Calcifer has fetched the ffmpeg these use.
project_files() {
step "a project's files, with no machine"
local own; own=$(find "$HOME/.cozy/tools/ffmpeg" -name 'ffprobe*' 2> /dev/null | head -1)
[ -n "$own" ] || { check "a project's files: Calcifer fetched no ffmpeg"; return; }
local PATH="$(dirname "$own"):$PATH"
hard_cut
first_open
upgrade_interrupted
short_film_heals
cut_versions
free_versions
held_open
step "a project: what needs a machine says so where there is none"
local M=$OUT/no-machine code=0
mkdir -p "$M"; cp "$here/project/draw.py" "$M/"
printf 'version: 4\nassets:\n  made: {tool: ./draw.py/draw, instructions: Needs a machine.}\n' | yaml_in "$M"
render "$M" made || code=$?
{ [ "$code" != 0 ] && grep -q "local runs aren't available" "$OUT/render.txt"; } || check "no machine (exit $code): $(tail -c 300 "$OUT/render.txt")"
}

# A paid take landing while another program holds project.db's write lock past Calcifer's wait (a SQLite browser's open transaction): it
# stays owed, says so, and lands once, in order, when it's let go, with nothing run again.
lock_refused() {
[ "${RUNNER_OS:-}" = Linux ] && [ -z "${ALONE:-}" ] || return 0
step "a project: a take landing waits out project.db held by another program, and lands once"
local T=$OUT/lock-refused rendering run code=0 ended holder
mkdir -p "$T"
yaml_in "$T" <<YAML
version: 4
assets:
  first: {tool: $here/project/draw.py/draw, instructions: Starts the project.}
  take:
    tool: $here/project/shots.py/takes
    seconds: 3
YAML
render "$T" first || check "lock refused: first: $(tail -c 300 "$OUT/render.txt")"
# A tool with a seed: -n 2 is two runs whether or not its function was described by the time they went (slow, with none, is one).
render "$T" take -n 2 & rendering=$!
for _ in $(seq 240); do [ "$(owed "$T")" = 2 ] && break; sleep 0.25; done
[ "$(owed "$T")" = 2 ] || check "lock refused: the takes never went: $(owed "$T") owed"
python3 - "$T/project.db" "$T.held" <<'PY' & holder=$!
import os, sqlite3, sys, time
conn = sqlite3.connect(sys.argv[1], isolation_level=None)
conn.execute("BEGIN IMMEDIATE")
open(sys.argv[2], "w").close()
while not os.path.exists(sys.argv[2] + ".go"):
    time.sleep(0.1)
conn.execute("ROLLBACK")
PY
until [ -e "$T.held" ] || ! kill -0 "$holder" 2> /dev/null; do sleep 0.1; done
[ -e "$T.held" ] || check "lock refused: nothing held project.db"
for _ in $(seq 600); do grep -q 'Not saved yet' "$OUT/render.txt" && break; sleep 0.25; done
grep -q 'Not saved yet' "$OUT/render.txt" || check "lock refused: nothing says it isn't saved: $(tail -c 300 "$OUT/render.txt")"
grep -q 'project.db is open in another program' "$OUT/render.txt" || check "lock refused: it doesn't say what holds it: $(tail -c 300 "$OUT/render.txt")"
for _ in $(seq 240); do
  ended=$(cozy --json run ls --limit 0 | jq --arg s "$(status "$T" | jq -r .id)" '[.runs[] | select(.project_id == $s and .status == "completed")] | length')
  [ "$ended" = 3 ] && break; sleep 0.25
done
[ "$ended" = 3 ] || check "lock refused: $ended of 3 runs ended"
[ "$(made "$T" take)" = 0 ] || check "lock refused: a take landed while project.db was held: $(made "$T" take)"
J '[.warnings[]?.message] | any(contains("Not saved yet"))' <<< "$(status "$T")" || check "lock refused: status doesn't say so: $(status "$T" | jq -c .warnings)"
touch "$T.held.go"; wait "$holder" || true
wait "$rendering" || code=$?
[ "$code" = 0 ] || check "lock refused: the render (exit $code): $(tail -c 300 "$OUT/render.txt")"
J '[.versions[].number] == [1, 2] and all(.versions[]; (.runs | length) == 1)' <<< "$(held "$T" take)" || check "lock refused: its versions: $(held "$T" take | jq -c '[.versions[] | {number, runs}]')"
[ "$(cozy --json run ls --limit 0 | jq --arg s "$(status "$T" | jq -r .id)" '[.runs[] | select(.project_id == $s)] | length')" = 3 ] || check "lock refused: something ran again"
[ "$(grep -c 'Not saved yet' "$OUT/render.txt")" = 1 ] || check "lock refused: said $(grep -c 'Not saved yet' "$OUT/render.txt") times"
! J '[.warnings[]?.message] | any(contains("Not saved yet"))' <<< "$(status "$T")" || check "lock refused: it still says so"
rm -f "$T.held" "$T.held.go"
}

# A film's versions, as `discards`, `renames_read` and `lost_lock` hold a take's, with nothing to run.
free_versions() {
step "a project: a film's versions discarded and put back, renamed and reclaimed"
local T=$OUT/free-versions a b first second count code
mkdir -p "$T"
plate "$T/a.mp4" testsrc2 440 800 2; plate "$T/b.mp4" smptebars 660 800 2
printf 'version: 4\nassets: {}\n' | yaml_in "$T"
a=$(prefix "$T" "$T/a.mp4"); b=$(prefix "$T" "$T/b.mp4")
# film_of <clip>…: the project as a film of those.
film_of() {
  { printf 'version: 4\nassets:\n  a: {approved: %s}\n  b: {approved: %s}\n  film:\n    tool: cozy/edit\n    tracks:\n      - clips:\n' "$a" "$b"
    printf '          - <%s>\n' "$@"; } | yaml_in "$T"
}
film_of a; render "$T" film || check "free versions: first: $(tail -c 300 "$OUT/render.txt")"
film_of a b; render "$T" film || check "free versions: second: $(tail -c 300 "$OUT/render.txt")"
film_of b a; render "$T" film || check "free versions: third: $(tail -c 300 "$OUT/render.txt")"
[ "$(made "$T" film)" = 3 ] || check "free versions: it made $(made "$T" film)"
first=$(take_file "$T" film video 0); second=$(take_file "$T" film video 1)
cozy project -p "$T" delete film --version 2 > /dev/null || check "free versions: delete the middle"
J '[.versions[].number] == [1, 3] and .discarded == [2]' <<< "$(held "$T" film)" || check "free versions: after the middle: $(held "$T" film | jq -c '{versions: [.versions[].number], discarded}')"
[ -n "$(file_at "$T" "$second")" ] || check "free versions: its file went at once"
[ "$(sql1 "$T" "SELECT count(*) FROM versions WHERE key = 'film' AND discarded = 1")" = 1 ] || check "free versions: project.db has no record of it"
cozy project -p "$T" restore film --version 2 > /dev/null || check "free versions: restore"
J '[.versions[].number] == [1, 2, 3] and (.discarded // []) == []' <<< "$(held "$T" film)" || check "free versions: after Undo: $(held "$T" film | jq -c '{versions: [.versions[].number], discarded}')"

cozy project -p "$T" rename film final > "$OUT/free-rename.txt" 2>&1 || check "free versions: rename: $(tail -c 300 "$OUT/free-rename.txt")"
J '.asset.standing == "current" and (.versions | length) == 3' <<< "$(held "$T" final)" || check "free versions: renamed: $(held "$T" final | jq -c .asset)"
[ -f "$T/final.mp4" ] || check "free versions: the film didn't follow its name: $(ls "$T" | tr '\n' ' ')"

cozy project -p "$T" delete final --version 1 > /dev/null || check "free versions: delete the first"
[ -n "$(file_at "$T" "$first")" ] || check "free versions: delete took the file"
cozy project -p "$T" reclaim --yes > "$OUT/free-reclaim.txt" 2>&1 || check "free versions: reclaim: $(tail -c 300 "$OUT/free-reclaim.txt")"
[ -z "$(file_at "$T" "$first")" ] || check "free versions: reclaim left what nothing names"
J '[.versions[].number] == [2, 3]' <<< "$(held "$T" final)" || check "free versions: after reclaim: $(held "$T" final | jq -c '[.versions[].number]')"
}

# Windows only: a film written over while another program has it open, as a player or a scanner has it, with no FILE_SHARE_DELETE, so a
# rename over it is refused while it's held. The write has to land anyway, or say why: never quietly go. project.db is Calcifer's alone
# (web/held.js holds its write lock).
held_open() {
[ "${RUNNER_OS:-}" = Windows ] || return 0
step "a project on Windows: a film over a file another program holds open"
local T=$OUT/held-open a b code hold
mkdir -p "$T"
plate "$T/a.mp4" testsrc2 440 800 2; plate "$T/b.mp4" smptebars 660 800 2
printf 'version: 4\nassets: {}\n' | yaml_in "$T"
a=$(prefix "$T" "$T/a.mp4"); b=$(prefix "$T" "$T/b.mp4")
yaml_of() {
  { printf 'version: 4\nassets:\n  a: {approved: %s}\n  b: {approved: %s}\n  film:\n    tool: cozy/edit\n    tracks:\n      - clips:\n' "$a" "$b"
    printf '          - <%s>\n' "$@"; } | yaml_in "$T"
}
yaml_of a; render "$T" film || check "held open: first: $(tail -c 300 "$OUT/render.txt")"
cozy project -p "$T" set film 'at=40,40,320' > /dev/null || check "held open: placing it"
# holding <file> [ms]: it's open from when this returns, for 2.5 s or [ms], in $hold.
holding() {
  local ready=$OUT/held-ready; rm -f "$ready"
  powershell -NoProfile -Command "\$f = [IO.File]::Open('$(cygpath -w "$1")', 'Open', 'Read', 'Read'); New-Item -ItemType File '$(cygpath -w "$ready")' | Out-Null; Start-Sleep -Milliseconds ${2:-2500}; \$f.Close()" & hold=$!
  until [ -e "$ready" ] || ! kill -0 "$hold" 2> /dev/null; do sleep 0.1; done
  [ -e "$ready" ] || check "held open: nothing held $1"
}
yaml_of b
holding "$T/film.mp4"
code=0; render "$T" film || code=$?
wait "$hold"
{ [ "$code" = 0 ] && [ "$(made "$T" film)" = 2 ]; } || check "held open: film.mp4 (exit $code, $(made "$T" film) versions): $(tail -c 300 "$OUT/render.txt")"

step "a project on Windows: a file held past the wait costs nothing, and says so"
local was
# As a player holds a film someone's watching: the version lands, and the film is updated when it's let go.
yaml_of a b; was=$(sha "$T/film.mp4")
holding "$T/film.mp4" 9000
code=0; render "$T" film || code=$?
{ [ "$code" = 0 ] && [ "$(made "$T" film)" = 3 ]; } || check "held long: a render lost its version (exit $code, $(made "$T" film) versions): $(tail -c 300 "$OUT/render.txt")"
says "held long" "film.mp4 is open in another program"
[ "$(grep -c 'is open in another program' "$OUT/render.txt")" = 1 ] || check "held long: said more than once: $(grep -c 'is open in another program' "$OUT/render.txt")"
[ "$(sha "$T/film.mp4")" = "$was" ] || check "held long: the film changed while held"
wait "$hold"
render "$T" film || check "held long: the render after: $(tail -c 300 "$OUT/render.txt")"
[ "sha256:$(sha "$T/film.mp4")" = "$(take_file "$T" film video)" ] || check "held long: the film isn't the state once it's let go"
}

# The project and page cells' fixtures are tools (`@app.tool`, cozy-runtime >=0.43). A machine with no Runtime of its own takes PyPI's
# newest, which before 0.43 is published refuses them: said once, here, and the cells don't go on.
tools_read() {
step "a project: the fixtures' tools read on this machine"
(cd "$OUT/project" && cozy run -p "$OUT/project" "$here/project/draw.py/draw" instructions=read --on local) > "$OUT/tools-read.txt" 2>&1 \
  || fail "the fixtures' tools don't read here: the machine's Runtime (${CAND_RUNTIME:-none of its own, so the newest on PyPI}) needs cozy-runtime's tools (rc/tools), as --runtime: $(tail -c 300 "$OUT/tools-read.txt")"
}

projects() {
tools_read
step "a project: new, named, a folder of it, -p"
local P=$OUT/garden code ID calcifer
# Calcifer takes no arguments: any says what it is, and starts no daemon.
calcifer=$(jq -r .pid < "$HOME/.cozy/calcifer/calcifer.lock.holder")
[ "$(cozy calcifer --help | grep -c 'takes no arguments')" = 1 ] || check "cozy calcifer --help doesn't say what it is"
code=0; cozy calcifer no-such-word > /dev/null 2>&1 || code=$?
[ "$code" = 1 ] || check "cozy calcifer no-such-word exited $code, not 1"
[ "$(jq -r .pid < "$HOME/.cozy/calcifer/calcifer.lock.holder")" = "$calcifer" ] || check "cozy calcifer --help moved Calcifer from $calcifer to $(jq -r .pid < "$HOME/.cozy/calcifer/calcifer.lock.holder")"
cozy project new "$P" > "$OUT/project-new.txt" 2>&1 || check "project new failed: $(tail -c 300 "$OUT/project-new.txt")"
[ "$(ls -A "$P" 2>/dev/null | grep -vxE 'project\.db-(wal|shm)')" = project.db ] || check "project new made $(ls -A "$P" 2>/dev/null | tr '\n' ' '), not only project.db"
grep -qE 'cozy project -p [^ ]+ render all' "$OUT/project-new.txt" || check "project new does not say what's next: $(tail -c 300 "$OUT/project-new.txt")"
cozy project -p "$P" status > "$OUT/project-new-status.txt" 2>&1 || check "status of a new project failed: $(tail -c 300 "$OUT/project-new-status.txt")"
for name in kyogre groudon coast awaken erupt clash edit; do grep -q "$name" "$OUT/project-new-status.txt" || check "a new project's status names no $name"; done
mkdir -p "$P/sub"
# The folder you're in names no project (cl-819): Calcifer may be on another computer. A folder of Calcifer's in it does.
(cd "$P/sub" && cozy --json project status) > "$OUT/project-cwd.json" 2> /dev/null && check "status named a project by the folder it ran in"
J '.error.code == "not_in_a_project"' "$OUT/project-cwd.json" || check "status in a project's folder, none named: $(head -c 300 "$OUT/project-cwd.json")"
cozy --json project -p "$P/sub" status > "$OUT/project-status.json" 2> "$OUT/project-status.err" || check "status of a folder in it failed: $(tail -c 300 "$OUT/project-status.err")"
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
upgrades
first_open
version_text
old_landing
reclaim_unheld
upgrade_interrupted
tombstones
lost_lock
lock_refused
discards
free_versions
text_assets
memo
pins
project_runs
path_free
entities
orphans
joining
walking
walk_restarted
no_double_spend
render_all_n
queueing
every_render
frames
[ "$(uname)" != Linux ] || no_stall
direct_inputs
tool_functions
binder_types
three_pictures
composed_tool
memo_retry
cancel_children
run_as_render
latents_stay
pickup_take
run_no_on
by_the_cli
cuts "$P" "$ID"
imports
[ -n "${ALONE:-}" ] || failures
http_surface "$P" "$ID"
event_stream
live_views
[ -n "${ALONE:-}" ] || browser_session "$P" "$ID"
project_index "$P" "$ID"
}

# The projects Calcifer knows: listed, a moved folder missing, DELETE forgetting it and never its folder.
project_index() {
step "a project: the index, a moved folder missing, DELETE forgets"
local P=$1 ID=$2
cozy --json project ls > "$OUT/project-ls-1.json" || check "project ls failed"
J --arg p "$P" '.projects | any(.path == $p and (.missing | not))' "$OUT/project-ls-1.json" || check "project ls does not list $P"
mv "$P" "$OUT/garden-moved"
cozy --json project ls > "$OUT/project-ls-2.json" || check "project ls after the move failed"
J --arg i "$ID" '.projects | any(.id == $i and .missing)' "$OUT/project-ls-2.json" || check "a moved project is not missing"
[ "$(api -o /dev/null -w '%{http_code}' -X DELETE "http://$ADDR/v1/projects/$ID")" = 204 ] || check "DELETE /v1/projects/$ID did not answer 204"
cozy --json project ls > "$OUT/project-ls-3.json" || true
{ J --arg a "$ID" '.projects | all(.id != $a)' "$OUT/project-ls-3.json" && [ -f "$OUT/garden-moved/project.db" ]; } || check "DELETE did not forget the project, or touched its folder"
forget_all
}

# Several runs at once, off a terminal and on one: every line names its asset, a preview is no take, a warning says whose it is, once,
# and what `--json` gives is as it was.
by_asset() {
step "a project: runs at once, each line starting with its asset"
local T=$OUT/by-asset look name shot
mkdir -p "$T"; plate "$T/look.mp4" testsrc2 440 800 2
printf 'version: 4\nassets: {}\n' | yaml_in "$T"
look=$(prefix "$T" "$T/look.mp4")
yaml_in "$T" <<YAML
version: 4
assets:
  look:
    approved: $look
  amy:
    tool: $here/project/draw.py/draw
    instructions: A picture.
  sophia:
    tool: $here/project/draw.py/draw
    instructions: Another.
  long:
    tool: $here/project/shots.py/scenes
    plate: <look>
    summary: <look> and <amy>.
    segments: 2
    each: 3
  short:
    tool: $here/project/shots.py/scenes
    plate: <look>
    summary: <look> and <sophia>.
    segments: 1
    each: 2
  cut:
    tool: cozy/edit
    tracks:
      - clips:
          - <long>
          - <short>
YAML
render "$T" all || check "by asset: render all: $(tail -c 400 "$OUT/render.txt")"
cp "$OUT/render.txt" "$OUT/by-asset.txt"
for name in amy sophia long short cut; do
  grep -qE "^$name: (take|version) [0-9]+" "$OUT/by-asset.txt" || check "by asset: no line says $name landed: $(cat "$OUT/by-asset.txt")"
done
# A shot's number is the order it went in, and these two go side by side.
shot=$(status "$T" | jq -r '.board.jobs[] | select(.asset == "long") | .shot') || true
says "by asset" "long: shot $shot/2 : segment 1/2 ready: files/"
says "by asset" "long: shot $shot/2 : segment 2/2, denoise"
says "by asset" "short: shot $((3 - shot))/2 : segment 1/1, denoise"
# Nothing of a run's own, and every line its asset's, the project's first line and the last aside.
! grep -qE '^run [0-9]+ · |^  on |^  inputs: |^  call |^(image|video)  ' "$OUT/by-asset.txt" || check "by asset: a run's own lines: $(cat "$OUT/by-asset.txt")"
grep -vE '^(amy|sophia|long|short|cut): |^by-asset( on [a-z]+)?: following|^rendered ' "$OUT/by-asset.txt" > "$OUT/by-asset-other.txt" || true
[ ! -s "$OUT/by-asset-other.txt" ] || check "by asset: a line with no asset: $(cat "$OUT/by-asset-other.txt")"
[ "$(grep -c 'lens is dusty' "$OUT/by-asset.txt")" = 1 ] || check "by asset: a warning not said once, or not at all: $(grep 'lens is dusty' "$OUT/by-asset.txt")"
grep -qE '^(long|short): warning: the fixture' "$OUT/by-asset.txt" || check "by asset: a warning names no asset: $(grep 'lens is dusty' "$OUT/by-asset.txt")"
J '.board.jobs | map(select(.asset == "long")) | .[0] | .state == "done" and .noun == "take" and .number == 1 and (.path | startswith("files/"))' <<< "$(status "$T")" || check "by asset: status's board: $(status "$T" | jq -c .board)"
# Shots are what makes a video by a package: a still and a cut count for nothing.
J '.board.shots == 2 and ([.board.jobs[] | {(.asset): .shot}] | add | [.amy, .sophia, .cut] == [null, null, null] and ([.long, .short] | sort) == [1, 2])' <<< "$(status "$T")" || check "by asset: its shots: $(status "$T" | jq -c '{shots: .board.shots, jobs: [.board.jobs[] | {asset, shot}]}')"

step "a project: --json is each run's own events"
cozy --json project -p "$T" render short --on local > "$OUT/by-asset.ndjson" 2> "$OUT/by-asset-json.err" || check "by asset: --json: $(tail -c 300 "$OUT/by-asset-json.err")"
for type in run.completed output_item.landed run.log; do
  jq -se --arg t "$type" 'any(.type == $t)' "$OUT/by-asset.ndjson" > /dev/null || check "by asset: --json has no $type: $(cut -c1-120 "$OUT/by-asset.ndjson" | head -20)"
done

step "a project: --plain on a terminal is one line per event"
printf 'project\x1frender\x1f--plain\x1f\x1e' >> "$OUT/argv.log"  # script runs the binary itself
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

# Another format, of the file or of the lock, is said and left as it is: nothing writes over it.
hard_cut() {
step "a project: format 1 is refused, untouched"
local O=$OUT/old code before
mkdir -p "$O"
printf 'version: 1\nentities:\n  - {name: a, kind: character}\n' | yaml_in "$O"
code=0; cozy project -p "$O" status > "$OUT/old-yaml.txt" 2>&1 || code=$?
{ [ "$code" != 0 ] && grep -q 'format 1' "$OUT/old-yaml.txt"; } || check "a format 1 project.yaml was read (exit $code): $(tail -c 200 "$OUT/old-yaml.txt")"
printf 'version: 4\nassets:\n  a:\n    description: A.\n' | yaml_in "$O"
printf '{"nodes": {"a": {"kind": "character", "versions": [{"number": 1}]}}}\n' > "$O/project.lock"
before=$(sha "$O/project.lock")
code=0; cozy project -p "$O" status > "$OUT/old-lock.txt" 2>&1 || code=$?
{ [ "$code" != 0 ] && grep -q 'format 1' "$OUT/old-lock.txt" && [ "$(sha "$O/project.lock")" = "$before" ]; } || check "a format 1 project.lock was read or written (exit $code): $(tail -c 200 "$OUT/old-lock.txt")"
# What a page opening to the newest project would refuse.
rm -rf "$O"
}

# Format 2 through 3 to 4, here, with no Calcifer: nothing goes out of date and nothing runs. A project made for real, its files and versions
# then said back as format 2 would, an entity and a form over its sheets, a still a shot wrote, and a mention of what nothing wrote.
upgrades() {
step "a project: a format 2 folder opened moves to format 4 and into project.db, its files kept as .v2"
local U=$OUT/upgrade draw=$here/project/draw.py before art wall clip
mkdir -p "$U"; still "$U/art.png" teal; plate "$U/clip.mp4" testsrc 440 200 8
printf 'version: 4\nassets: {}\n' | yaml_in "$U"
art=$(prefix "$U" "$U/art.png")
prefix "$U" "$U/clip.mp4" > /dev/null; clip=$(sha "$U/clip.mp4")
yaml_in "$U" <<YAML
version: 4
assets:
  art: {approved: $art}
  hero_sheet: {tool: $draw/draw, instructions: From <art>.}
  old_sheet: {tool: $draw/draw, instructions: From <hero_sheet>.}
  wall: {tool: $draw/draw, instructions: A wall.}
  poster: {tool: $draw/draw, instructions: Before an edit.}
YAML
render "$U" all || check "upgrade: making it: $(tail -c 300 "$OUT/render.txt")"
before=$(runs "$U")
cozy project -p "$U" approve wall 1 > /dev/null 2>&1 || check "upgrade: approve wall"
wall=$(take_file "$U" wall image 0); wall=${wall#sha256:}; wall=${wall:0:12}
# What a format 2 lock said of the same, read while this cozy can read it.
local was_lock
old_row() { held "$U" "$1" | jq --arg n "$1" '{name: $n, kind: "image", versions: [.versions[] | {number, seed, runs, inputs: "old-key-\(.number)", made_by: {with: "local/draw/draw"}, files: (.files | map_values(.sha256))}]}'; }
old_files() { held "$U" "$1" | jq '[.versions[].files[] | {(.sha256): {path, sha256}}] | add'; }
was_lock=$(jq -n --argjson a "$(old_row hero_sheet)" --argjson b "$(old_row old_sheet)" --argjson c "$(old_row wall)" --argjson d "$(old_row poster)" --arg clip "sha256:$clip" \
  --argjson files "$(jq -n --argjson a "$(old_files hero_sheet)" --argjson b "$(old_files old_sheet)" --argjson c "$(old_files wall)" --argjson d "$(old_files poster)" '$a + $b + $c + $d')" \
  '{format: 2, files: ($files + {($clip): {path: "files/\($clip[7:]).mp4", sha256: $clip}}), assets: {"hero sheet": $a, "old sheet": $b, wall: $c, poster: $d,
    "table.1": {name: "table.1", kind: "image", versions: [{number: 7, inputs: "old-frame", files: {image: $c.versions[0].files.image}, made_by: {with: "cozy/frame"}}]},
    take: {name: "take", kind: "video", numbered: 4, versions: [{number: 4, inputs: "old-take", files: {video: $clip}, made_by: {with: "local/minimax-h3/generate"}}]},
    "frame.1": {name: "frame.1", kind: "image", numbered: 4, versions: [{number: 4, inputs: "old-frame-4", files: {image: $a.versions[0].files.image}, made_by: {with: "cozy/frame"}}]}}}') || check "upgrade: what it holds: $(tail -c 300 "$OUT/held.err")"
# What a format 2 file said of the same, in a folder of its own, with its files: what its first open moves.
local V=$OUT/upgrade-v2
mkdir -p "$V/files"; cp -l "$U"/files/* "$V/files/"
cat > "$V/project.yaml" <<YAML
# A hand's note.
version: 2
defaults:
  image:
    with: $draw
    style: Plain.
entities:
  hero:
    kind: character
    description: A brave hero.
    image: <hero_sheet>
    old:
      description: The hero of old.
      image: <old_sheet>
assets:
  # Its art.
  art:
    image: $art  #ken
  hero_sheet:
    render: From <art>.
  old_sheet:
    render: From <hero_sheet>.
  wall:
    image: $wall
    render: A wall.
  poster:
    render: After an edit, with <table.1> and <coast.1>.
  clear:
    render: {with: /nowhere/qwen-image-2, prompt: A cat., transparent: true}
  shot:
    kind: video
    render: {with: cozy}
    pieces:
      - <hero.old>
  # A frame a beat wrote, its one version numbered past 1.
  frame.1:
  take:
    kind: video
    render: {with: /nowhere/minimax-h3}
    beats:
      - duration: 4
        description: A wave.
      - duration: 4
        description: It breaks.
        writes: [{image: frame.1, at: 1.5}]
YAML
printf '%s\n' "$was_lock" > "$V/project.lock"
cp "$V/project.yaml" "$OUT/upgrade-v2.yaml"; cp "$V/project.lock" "$OUT/upgrade-v2.lock"
# What its first open says, once, is on the open's answer: the CLI prints it.
cozy project -p "$V" status > "$OUT/upgraded.txt" 2>&1 || check "upgrade: it said $(tail -c 400 "$OUT/upgraded.txt")"
cozy --json --full project -p "$V" status > "$OUT/upgraded-status.json" 2> "$OUT/upgraded.err" || check "upgrade: it said $(tail -c 400 "$OUT/upgraded.err")"
cmp -s "$V/project.yaml.v2" "$OUT/upgrade-v2.yaml" && cmp -s "$V/project.lock.v2" "$OUT/upgrade-v2.lock" || check "upgrade: the old files aren't kept beside the new"
{ [ -f "$V/project.db" ] && [ ! -e "$V/project.yaml" ] && [ ! -e "$V/project.lock" ] && [ -f "$V/project.yaml.v3" ]; } || check "upgrade: 2 to 3 to 4, into project.db: $(ls -A "$V" | tr '\n' ' ')"
cozy project -p "$V" export > "$OUT/upgraded.yaml"
{ [ "$(made "$V" hero)" = 1 ] && [ "$(made "$V" hero_old)" = 1 ] && J '.versions[0].number == 7' <<< "$(held "$V" table_1)"; } || check "upgrade: its versions: $(counts "$V")"
J '.assets[] | select(.name == "wall") | .approved == 1' "$OUT/upgraded-status.json" || check "upgrade: wall holds no version: $(row "$V" wall .)"
J '.assets[] | select(.name == "clear") | .fields.instructions == "This is an RGBA image with transparency. A cat.\n\nThe image has alpha channel and the background is transparent." and (.fields | has("transparent") | not)' "$OUT/upgraded-status.json" || check "upgrade: Qwen's transparent: $(row "$V" clear .fields)"
J '.assets[] | select(.name == "hero") | (.fields | has("asset_type") | not) and .fields.description == "A brave hero." and .fields.instructions == "From <art>." and .fields.style == "<style>"' "$OUT/upgraded-status.json" || check "upgrade: hero: $(row "$V" hero .fields)"
J '.assets[] | select(.name == "hero_old") | .fields.description == "The hero of old." and .fields.instructions == "From <hero>." and (.fields | has("asset_type") | not)' "$OUT/upgraded-status.json" || check "upgrade: hero_old: $(row "$V" hero_old .fields)"
J '.assets[] | select(.name == "poster") | .versions == 1 and .fields.instructions == "After an edit, with <table_1> and <coast.1>."' "$OUT/upgraded-status.json" || check "upgrade: poster: $(row "$V" poster .)"
{ grep -q 'cozy/edit' "$OUT/upgraded.yaml" && grep -q '<hero_old>' "$OUT/upgraded.yaml" && ! grep -qE '^ *(kind|with|render|pieces|writes):|^entities|^defaults' "$OUT/upgraded.yaml"; } || check "upgrade: what it says: $(cat "$OUT/upgraded.yaml")"
J '.assets[] | select(.name == "frame_1") | (.pinned | not) and .approved == null and .versions == 1 and .tool == "cozy/frame" and .fields.video == "<take>" and .fields.at == 5.5' "$OUT/upgraded-status.json" || check "upgrade: a frame a beat wrote isn't a frame of its shot, unapproved: $(row "$V" frame_1 .)"
[ "$(grep -c 'poster mentions <coast.1>' "$OUT/upgraded.txt")" = 1 ] || check "upgrade: a mention of nothing wasn't said: $(cat "$OUT/upgraded.txt")"
{ [ "$(grep -c 'hero, hero_old are now a picture reference like any other' "$OUT/upgraded.txt")" = 1 ] && [ "$(grep -c 'hero.old is hero_old' "$OUT/upgraded.txt")" = 1 ]; } || check "upgrade: what format 4 reads another way wasn't said once: $(cat "$OUT/upgraded.txt")"
cozy project -p "$V" status > /dev/null 2>&1 || check "upgrade: opened again"
[ ! -e "$V/project.yaml" ] || check "upgrade: a second open left a project.yaml"

step "a project: a format 2 folder opened: \`defaults\` go, a style and a soundscape are text assets, each mentioned where it was used"
local W=$OUT/upgrade-defaults
mkdir -p "$W"
cat > "$W/project.yaml" <<YAML
version: 2
defaults:
  image: {with: /nowhere/qwen-image-2, style: Plain, aspect_ratio: "16:9"}
  video: {with: /nowhere/minimax-h3, style: Plain and bold., soundscape: Rain., music: N/A, aspect_ratio: "16:9", turbo: true}
assets:
  clear: {render: {prompt: A cat., transparent: true}}
  dull: {render: {prompt: A dog., aspect_ratio: "3:2"}}
  shot:
    kind: video
    summary: <clear> walks.
    beats: [{duration: 5, description: It rains.}]
  plain_shot:
    kind: video
    soundscape: Quiet.
    beats: [{duration: 5, description: It stops.}]
YAML
# What its first open says, once, is on the open's answer: the CLI prints it.
cozy project -p "$W" status > "$OUT/upgraded-defaults.txt" 2>&1 || check "upgrade: defaults: $(tail -c 300 "$OUT/upgraded-defaults.txt")"
status "$W" > "$OUT/upgraded-defaults-status.json" || check "upgrade: defaults: $(tail -c 300 "$OUT/status.err")"
of() { jq -c --arg n "$1" '.assets[] | select(.name == $n) | .fields' "$OUT/upgraded-defaults-status.json"; }
J '.text == "Plain"' <<< "$(of style)" || check "upgrade: style: $(of style)"
J '.text == "<style> and bold."' <<< "$(of video_style)" || check "upgrade: video_style: $(of video_style)"
J '.text == "Rain."' <<< "$(of soundscape)" || check "upgrade: soundscape: $(of soundscape)"
J '.instructions == "This is an RGBA image with transparency. Style: <style>.\n\nA cat.\n\nThe image has alpha channel and the background is transparent." and .aspect_ratio == "16:9"' <<< "$(of clear)" || check "upgrade: clear: $(of clear)"
J '.instructions == "Style: <style>.\n\nA dog." and .aspect_ratio == "3:2"' <<< "$(of dull)" || check "upgrade: an asset's own value went: $(of dull)"
J '.summary == "<video_style>\n<clear> walks." and .soundscape == "<soundscape>" and (has("music") | not) and (has("style") | not) and .aspect_ratio == "16:9"' <<< "$(of shot)" || check "upgrade: shot: $(of shot)"
J '.summary == "<video_style>" and .soundscape == "Quiet."' <<< "$(of plain_shot)" || check "upgrade: a shot with no summary: $(of plain_shot)"
! grep -q '^defaults' <(cozy project -p "$W" export) || check "upgrade: defaults stayed: $(cozy project -p "$W" export)"
[ "$(grep -c 'H3 shots now carry the style in their summary' "$OUT/upgraded-defaults.txt")" = 1 ] || check "upgrade: the move wasn't said once: $(cat "$OUT/upgraded-defaults.txt")"

step "a project: a format 2 folder opened: names that fold to one are said, by name, and nothing is written"
local N=$OUT/upgrade-names
mkdir -p "$N"
printf 'version: 2\nassets:\n  Amy_one:\n    description: One.\n  amy-ONE:\n    description: Two.\n' > "$N/project.yaml"
cp "$N/project.yaml" "$OUT/upgrade-names.yaml"
code=0; cozy project -p "$N" status > "$OUT/upgrade-names.txt" 2>&1 || code=$?
{ [ "$code" != 0 ] && grep -q 'Amy_one and amy-ONE are one name' "$OUT/upgrade-names.txt" && cmp -s "$N/project.yaml" "$OUT/upgrade-names.yaml" && [ ! -e "$N/project.db" ]; } || check "upgrade: names that are one (exit $code): $(tail -c 300 "$OUT/upgrade-names.txt")"
rm -rf "$N"
}

# A text asset: its words go where it's mentioned, once, in Doc, and the key follows them.
text_assets() {
step "a project: a text asset's words go where it's mentioned, in any case, and what they mention is read"
local T=$OUT/text draw=$here/project/draw.py was
mkdir -p "$T"
yaml_in "$T" <<YAML
version: 4
assets:
  style:
    text: |
      Soft light.
  mood:
    text: <style> And <card>.
  card: {tool: $draw/draw, instructions: A card.}
  poster: {tool: $draw/draw, instructions: <STYLE> A poster.}
  sheet: {tool: $draw/draw, instructions: "<Mood> A sheet."}
  ring_a: {text: A <ring_b>}
  ring_b: {text: B <ring_a>}
  ringed: {tool: $draw/draw, instructions: <ring_a>}
  lost: {tool: $draw/draw, instructions: Without <nothing_here>.}
YAML
J '.assets[] | select(.name == "style") | .kind == "text" and .standing == "current" and .fields.text == "Soft light." and .versions == 0' <<< "$(status "$T")" || check "text: style: $(row "$T" style .)"
render "$T" poster sheet ringed || check "text: render: $(tail -c 400 "$OUT/render.txt")"
[ "$(prompt "$T" poster)" = 'Soft light. A poster.' ] || check "text: poster was told: $(prompt "$T" poster)"
grep -q '^Soft light\. And ' <<< "$(prompt "$T" sheet)" || check "text: sheet was told: $(prompt "$T" sheet)"
grep -qF "$(take_file "$T" card image)" <<< "$(asked "$T" sheet)" || check "text: a mention inside a text isn't a read: $(asked "$T" sheet)"
[ "$(row "$T" poster .fields.instructions)" = '"<STYLE> A poster."' ] || check "text: what's shown isn't what's written: $(row "$T" poster .fields)"
J '[.[].name] | index("card") != null and index("Mood") == null and index("mood") != null and index("style") != null' <<< "$(row "$T" sheet .reads)" || check "text: reads: $(row "$T" sheet .reads)"
J '[.warnings[].message] | any(contains("mentions itself, through what it mentions"))' <<< "$(status "$T")" || check "text: a cycle isn't said: $(status "$T" | jq -c .warnings)"
grep -q '<ring_a>' <<< "$(prompt "$T" ringed)" || check "text: a cycle didn't stay text: $(prompt "$T" ringed)"
render "$T" lost > /dev/null 2>&1 || true
says text "<nothing_here> is no longer in the project"

step "a project: editing a text marks what reaches it, an approved asset stays approved, and a deleted one is gone, not changed"
cozy project -p "$T" approve poster 1 > /dev/null 2>&1 || check "text: approve poster"
was=$(made "$T" poster)
hand "$T" 's/Soft light\./Hard light./'
J '.assets[] | select(.name == "sheet") | .standing == "out_of_date"' <<< "$(status "$T")" || check "text: an edit didn't reach sheet, through mood: $(row "$T" sheet .)"
J '.assets[] | select(.name == "poster") | .standing == "out_of_date" and .pinned and (.status | contains("<style>"))' <<< "$(status "$T")" || check "text: poster: $(row "$T" poster .)"
hand "$T" 's/Hard light\./Soft light./'
J '.assets[] | select(.name == "sheet") | .standing == "current"' <<< "$(status "$T")" || check "text: words back didn't bring it back: $(row "$T" sheet .)"
dropped "$T" ring_a ring_b ringed lost style
J '.assets[] | select(.name == "poster") | .standing == "current" and .pinned' <<< "$(status "$T")" || check "text: a deleted text changed what it was in: $(row "$T" poster .)"
[ "$(made "$T" poster)" = "$was" ] || check "text: something ran"

step "a project: a name is one name in any case, a text can't be a tag, and a typed name that's taken is refused"
local N=$OUT/text-names id code
mkdir -p "$N"
printf 'version: 4\nassets:\n  Amy:\n    description: One.\n  amy:\n    description: Two.\n' | yaml_in "$N"
code=0; cozy project -p "$N" status > "$OUT/one-name.txt" 2>&1 || code=$?
{ [ "$code" != 0 ] && grep -q 'Amy, on line 3, and amy, on line 5, are one name' "$OUT/one-name.txt"; } || check "names: two keys of one name (exit $code): $(tail -c 300 "$OUT/one-name.txt")"
printf 'version: 4\nassets: {}\n' | yaml_in "$N"
[ "$(cozy project -p "$N" add text note text='Some words.')" = note ] || check "names: add text with no name"
[ "$(cozy project -p "$N" add text Note_2 text='More words.')" = Note_2 ] || check "names: add text with a name"
code=0; cozy project -p "$N" add text NOTE text=Again. > "$OUT/typed.txt" 2>&1 || code=$?
{ [ "$code" != 0 ] && grep -q 'note is in the project already' "$OUT/typed.txt"; } || check "names: a typed name that's taken (exit $code): $(tail -c 300 "$OUT/typed.txt")"
[ "$(cozy project -p "$N" add text text='Picked for it.')" = text ] || check "names: a picked name"
[ "$(cozy project -p "$N" add text text='Picked again.')" = text_2 ] || check "names: a picked name is numbered"
J '.assets[] | select(.name == "note") | .kind == "text" and .fields.text == "Some words."' <<< "$(status "$N")" || check "names: a text from the CLI: $(row "$N" note .)"
step "a project: a text asset can't take a package's tag"
local R=$OUT/text-tags
mkdir -p "$R"
yaml_in "$R" <<YAML
version: 4
assets:
  look: {tool: $here/project/shots.py/shoot, refuse: Only its tags are read here.}
  D: {text: A tag.}
YAML
render "$R" look > /dev/null 2>&1 || true
J '[.warnings[].message] | any(contains("so a text asset can'"'"'t go by it: cozy project rename D"))' <<< "$(status "$R")" || check "tags: not said: $(status "$R" | jq -c .warnings)"
}

# The first thing to touch a project of format 2 moves it, as `cozy project upgrade` with no status would.
first_open() {
step "a project: format 2 upgrades itself on first open, says so once, and keeps .v2 beside it"
local T=$OUT/first-open code answer before lock=$HOME/.cozy/calcifer/calcifer.lock.holder
ADDR=$(jq -r .address "$lock") TOKEN=$(jq -r .token "$lock")
mkdir -p "$T"
printf 'version: 2\nassets:\n  a:\n    description: A.\n' | yaml_in "$T"
printf '{"format": 2, "assets": {}, "files": {}}\n' > "$T/project.lock"
cp "$T/project.yaml" "$OUT/first-open.yaml"; cp "$T/project.lock" "$OUT/first-open.lock"
cozy project -p "$T" status > "$OUT/first-open.txt" 2>&1 || check "first open: status: $(tail -c 300 "$OUT/first-open.txt")"
grep -qF 'project.yaml.v2' "$OUT/first-open.txt" || check "first open: not said: $(tail -c 300 "$OUT/first-open.txt")"
grep -q '^version: 4$' <(cozy project -p "$T" export) || check "first open: it didn't upgrade: $(cozy project -p "$T" export)"
{ cmp -s "$T/project.yaml.v2" "$OUT/first-open.yaml" && cmp -s "$T/project.lock.v2" "$OUT/first-open.lock"; } || check "first open: an original went"
cozy project -p "$T" status > "$OUT/first-open-2.txt" 2>&1 || check "first open: status again: $(tail -c 300 "$OUT/first-open-2.txt")"
! grep -qF 'project.yaml.v2' "$OUT/first-open-2.txt" || check "first open: said twice: $(cat "$OUT/first-open-2.txt")"
local P=$OUT/first-open-page
mkdir -p "$P"
printf 'version: 2\nassets:\n  a:\n    description: A.\n' | yaml_in "$P"
answer=$(api -X POST -d "$(jq -n --arg p "$P" '{project: $p}')" "http://$ADDR/v1/projects")
J '.said | any(contains("project.yaml.v2"))' <<< "$answer" || check "first open: the route doesn't say so: $answer"

step "a project: what can't convert is refused with why, and left as it is"
local C=$OUT/first-open-stuck
mkdir -p "$C"
printf 'version: 2\nassets:\n  Amy_one:\n    description: One.\n  amy-ONE:\n    description: Two.\n' | yaml_in "$C"
before=$(sha "$C/project.yaml")
code=0; cozy project -p "$C" status > "$OUT/first-open-stuck.txt" 2>&1 || code=$?
{ [ "$code" != 0 ] && grep -q 'Amy_one' "$OUT/first-open-stuck.txt" && [ "$(sha "$C/project.yaml")" = "$before" ] && [ ! -e "$C/project.yaml.v2" ]; } || check "first open: names that are one (exit $code): $(tail -c 300 "$OUT/first-open-stuck.txt")"
rm -rf "$C"

step "a project: format 2 opened keeps what was out of date, and says each mention of what a beat wrote"
local S=$OUT/first-open-states draw=$here/project/draw.py art plate poster f
mkdir -p "$S/files"
still "$S/art.png" teal; still "$S/plate.png" navy; still "$S/poster.png" olive
art=$(sha "$S/art.png") plate=$(sha "$S/plate.png") poster=$(sha "$S/poster.png")
for f in art plate poster; do mv "$S/$f.png" "$S/files/$(sha "$S/$f.png").png"; done
# As 0.19 left it: coast's plate approved, poster's one version from before keys (out of date), and two stills clash's beats wrote, never made.
yaml_in "$S" <<YAML
version: 2
entities:
  coast:
    kind: environment
    image: <coast_plate>
assets:
  art:
    image: ${art:0:12}
  coast_plate:
    image: ${plate:0:12}
    render: {with: $draw, prompt: A coast.}
  poster:
    render: {with: $draw, prompt: From <art>.}
  clash:
    kind: video
    render: {with: /nowhere/minimax-h3}
    beats:
      - description: On <coast>.
        writes: [{image: coast.1}]
      - description: Then on <coast.1>.
        writes: [{image: coast.2}]
      - description: And on <coast.2>.
YAML
jq -n --arg art "sha256:$art" --arg plate "sha256:$plate" --arg poster "sha256:$poster" '{format: 2,
  files: ([$art, $plate, $poster] | map({(.): {path: "files/\(.[7:]).png", sha256: .}}) | add),
  assets: {"coast plate": {name: "coast_plate", kind: "image", numbered: 1, versions: [{number: 1, inputs: "", files: {image: $plate}}]},
    poster: {name: "poster", kind: "image", numbered: 1, versions: [{number: 1, inputs: "", files: {image: $poster}}]}}}' > "$S/project.lock"
cozy project -p "$S" status > "$OUT/first-open-states.txt" 2>&1 || check "first open: states: $(tail -c 300 "$OUT/first-open-states.txt")"
status "$S" > "$OUT/first-open-states.json"
J '.assets[] | select(.name == "poster") | .standing == "out_of_date" and .versions == 1' "$OUT/first-open-states.json" || check "first open: what was out of date isn't: $(row "$S" poster .)"
J '.assets[] | select(.name == "coast") | .standing == "current" and .approved == 1' "$OUT/first-open-states.json" || check "first open: coast: $(row "$S" coast .)"
J '.assets[] | select(.name == "art") | .standing == "current"' "$OUT/first-open-states.json" || check "first open: art: $(row "$S" art .)"
grep -qE "clash, line [0-9]+: <coast\.1> was what a beat of clash wrote, and now reads as coast's version 1" "$OUT/first-open-states.txt" || check "first open: <coast.1> not said: $(cat "$OUT/first-open-states.txt")"
grep -qE "clash, line [0-9]+: <coast\.2> was what a beat of clash wrote, and now reads as coast's version 2, which it hasn't" "$OUT/first-open-states.txt" || check "first open: <coast.2> not said: $(cat "$OUT/first-open-states.txt")"
}

# A version keeps the words that made it, only where they changed, whatever becomes of the one that held them.
version_text() {
step "a project: a version keeps its text, stored only where it changed"
local T=$OUT/version-text
mkdir -p "$T"
yaml_in "$T" <<YAML
version: 4
assets:
  hero: {tool: $here/project/draw.py/draw, instructions: A red door.}
YAML
render "$T" hero -n 2 || check "version text: render: $(tail -c 300 "$OUT/render.txt")"
hand "$T" 's/A red door/A blue door/'
render "$T" hero || check "version text: again: $(tail -c 300 "$OUT/render.txt")"
J '[.versions[].text] == ["A red door.", "A red door.", "A blue door."]' <<< "$(held "$T" hero)" || check "version text: $(held "$T" hero | jq -c '[.versions[].text]')"
local texts; texts=$(python3 -c 'import sqlite3, sys; print(sqlite3.connect(sys.argv[1]).execute("SELECT count(*) FROM versions WHERE record LIKE ?", (sys.argv[2],)).fetchone()[0])' "$T/project.db" '%"text":"A %')
[ "$texts" = 2 ] || check "version text: stored where it didn't change: $texts rows hold words"
cozy project -p "$T" delete hero --version 1 > /dev/null || check "version text: discard"
cozy project -p "$T" reclaim --yes > /dev/null 2>&1 || check "version text: reclaim"
J '[.versions[].text] == ["A red door.", "A blue door."]' <<< "$(held "$T" hero)" || check "version text: what held it went: $(held "$T" hero | jq -c '[.versions[].text]')"
}

# Paid-for bytes reach `files/` whatever the project is. A project of format 2, its run in flight over the upgrade, and one that can't convert.
old_landing() {
step "a project: a run landing in a project of format 2 upgrades it, lists its output, and says so"
local T=$OUT/old-landing n
mkdir -p "$T"
printf 'version: 2\nassets:\n  a:\n    description: A.\n' | yaml_in "$T"
printf '{"format": 2, "assets": {}, "files": {}}\n' > "$T/project.lock"
(cd "$T" && cozy --json run -p "$T" "$here/project/draw.py" instructions=kept) > "$OUT/old-landing.ndjson" 2> "$OUT/old-landing.err" || check "old landing: run: $(tail -c 300 "$OUT/old-landing.err")"
n=$(run_number "$OUT/old-landing.ndjson")
[ "$(ls "$T/files" 2> /dev/null | wc -l)" = 1 ] || check "old landing: nothing in files/: $(ls -a "$T" "$T/.runs" 2>&1 | tr '\n' ' ')"
J '[.warnings[]?.message] | any(contains("project.yaml.v2"))' <<< "$(cozy --json run show "$n" --once)" || check "old landing: no warning says it upgraded: $(cozy --json run show "$n" --once | jq -c .warnings)"
{ grep -q '^version: 4$' <(cozy project -p "$T" export) && [ -f "$T/project.db" ] && [ ! -e "$T/project.yaml" ]; } || check "old landing: it didn't upgrade into project.db: $(ls -A "$T" | tr '\n' ' ')"
J '.run_files == 1' <<< "$(status "$T")" || check "old landing: its output isn't listed: $(status "$T" | jq -c .run_files)"

step "a project: a run in flight lands, and is listed"
local going
(cd "$T" && cozy --json run -p "$T" "$here/project/shots.py/slow" seconds=8) > "$OUT/old-landing-slow.ndjson" 2>&1 & going=$!
wait "$going" || check "old landing: the slow run: $(tail -c 300 "$OUT/old-landing-slow.ndjson")"
[ "$(ls "$T/files" | wc -l)" = 2 ] || check "old landing: its output isn't in files/: $(ls "$T/files" | tr '\n' ' ')"
J '.run_files >= 1' <<< "$(status "$T")" || check "old landing: it isn't listed: $(status "$T" | jq -c .run_files)"

step "a project: a project that can't convert still takes a run's bytes"
local C=$OUT/old-landing-stuck
mkdir -p "$C"
printf 'version: 2\nassets:\n  Amy_one:\n    description: One.\n  amy-ONE:\n    description: Two.\n' | yaml_in "$C"
code=0; cozy project -p "$C" status > "$OUT/old-landing-stuck.txt" 2>&1 || code=$?
[ "$code" != 0 ] || check "old landing: names that are one converted"
(cd "$C" && cozy --json run -p "$C" "$here/project/draw.py" instructions=stuck) > "$OUT/old-landing-stuck.ndjson" 2>&1 || check "old landing: a run in it: $(tail -c 300 "$OUT/old-landing-stuck.ndjson")"
# Kept by its bytes: the one file in files/ is the run's output, by its sha256.
local kept digest
cozy --json run show "$(run_number "$OUT/old-landing-stuck.ndjson")" --once > "$OUT/old-landing-stuck-run.json" 2>&1 || true
digest=$(jq -r '.output[0].sha256 // empty' "$OUT/old-landing-stuck-run.json" 2> /dev/null || true)
kept=$(ls "$C/files" 2> /dev/null || true)
{ [ "$(ls "$C/files" 2> /dev/null | wc -l)" = 1 ] && [ "sha256:$(sha "$C/files/$kept")" = "$digest" ]; } || check "old landing: a project that can't convert lost its output: $(ls -a "$C" 2>&1 | tr '\n' ' ')"
echo "old landing: a project that can't convert kept ${digest:-nothing} as files/${kept:-nothing}, sha256:$(sha "$C/files/$kept" 2> /dev/null || true): $(jq -c '[.warnings[]?.message]' "$OUT/old-landing-stuck-run.json" 2> /dev/null || true)"
rm -rf "$C"
}

# A first open stopped part way (cl-803): killed before project.db is in place, the files stand and the next open imports them again;
# after, what's left goes. Nothing here runs anything.
upgrade_interrupted() {
step "a project: a first open killed at any moment loses nothing, and finishes"
local S=$OUT/old-landing-killed lock=$HOME/.cozy/calcifer/calcifer.lock.holder stopped=0 tried=0 at pid began took part F
mkdir -p "$S"
local native=$S
[ "${RUNNER_OS:-}" != Windows ] || native=$(cygpath -m "$S")
node - "$native" <<'JS'
const { createHash } = require('crypto');
const { writeFileSync } = require('fs');
const assets = {}, yaml = ['version: 2', 'assets:'];
for (let n = 0; n < 600; n++) {
  const digest = `sha256:${createHash('sha256').update(String(n)).digest('hex')}`;
  assets[`a ${n}`] = { name: `a_${n}`, kind: 'image', numbered: 1, versions: [{ number: 1, inputs: 'old', files: { image: digest }, made_by: { with: 'local/draw/draw' } }] };
  yaml.push(`  a_${n}:`, '    render: {with: ./draw.py, prompt: One of many.}');
}
writeFileSync(`${process.argv[2]}/project.yaml`, `${yaml.join('\n')}\n`);
writeFileSync(`${process.argv[2]}/project.lock`, JSON.stringify({ format: 2, assets, files: {} }));
JS
cp "$S/project.yaml" "$OUT/killed.yaml"; cp "$S/project.lock" "$OUT/killed.lock"
began=$(node -p 'Date.now()')
status "$S" > /dev/null || check "killed: a clean first open"
took=$(($(node -p 'Date.now()') - began))
cozy project -p "$S" export > "$OUT/killed-done.yaml" || check "killed: the clean one's export"
# From its start to past its end, each a folder of its own: how long it takes moves with the disk, so no one stretch of it can be counted on.
for part in $(seq 1 12); do
  at=$((took * part / 10)) tried=$((tried + 1)) F=$S-$part
  mkdir -p "$F"; cp "$OUT/killed.yaml" "$F/project.yaml"; cp "$OUT/killed.lock" "$F/project.lock"
  cozy project -p "$F" status > /dev/null 2>&1 & pid=$!
  sleep "$(printf '%d.%03d' $((at / 1000)) $((at % 1000)))"
  if kill -0 "$pid" 2> /dev/null; then
    eval "$(web_stop "$lock")" 2> /dev/null || true
    stopped=$((stopped + 1))
  fi
  wait "$pid" 2> /dev/null || true
  if [ ! -f "$F/project.db" ]; then
    { [ ! -e "$F/project.yaml.v2" ] || cmp -s "$F/project.yaml.v2" "$OUT/killed.yaml"; } || check "killed at ${at} ms: project.yaml.v2 isn't the original"
    { [ ! -e "$F/project.lock.v2" ] || cmp -s "$F/project.lock.v2" "$OUT/killed.lock"; } || check "killed at ${at} ms: project.lock.v2 isn't the original"
  fi
  cozy project -p "$F" export > "$OUT/killed-again.yaml" 2>&1 || check "killed at ${at} ms: opened again: $(tail -c 300 "$OUT/killed-again.yaml")"
  cmp -s "$OUT/killed-again.yaml" "$OUT/killed-done.yaml" || check "killed at ${at} ms: it came out otherwise: $(diff "$OUT/killed-done.yaml" "$OUT/killed-again.yaml" | head -5)"
  { cmp -s "$F/project.yaml.v2" "$OUT/killed.yaml" && cmp -s "$F/project.lock.v2" "$OUT/killed.lock"; } || check "killed at ${at} ms: an original went"
  [ -z "$(cd "$F" && ls -A | grep -vxE 'files|\.runs|project\.db(-wal|-shm)?|project\.(yaml|lock)\.v[23]')" ] || check "killed at ${at} ms: left: $(ls -A "$F" | tr '\n' ' ')"
  rm -rf "$F"
done
echo "first open killed mid-way $stopped of $tried times, in $took ms"
[ "$stopped" -gt 0 ] || check "killed: no kill landed before it finished"
# The Calcifer after the kills, as the cells after this call it.
cozy project ls > /dev/null || check "killed: cozy doesn't answer after the kills"
ADDR=$(jq -r .address "$lock") TOKEN=$(jq -r .token "$lock")
}

# A name the project lost, and a render since, leaves its versions an orphan, and that name starts over.
tombstones() {
step "a project: a name the file lost is an orphan once a render has seen it, and that name starts over"
local N=$OUT/tombstones draw=$here/project/draw.py
mkdir -p "$N"
yaml_in "$N" <<YAML
version: 4
assets:
  once: {tool: $draw/draw, instructions: The first of that name.}
  other: {tool: $draw/draw, instructions: Another.}
YAML
render "$N" all || check "tombstones: render all: $(tail -c 300 "$OUT/render.txt")"
dropped "$N" once
render "$N" other || check "tombstones: render other: $(tail -c 300 "$OUT/render.txt")"
printf '  once: {tool: %s/draw, instructions: Something else entirely.}\n' "$draw" | added "$N"
{ [ "$(made "$N" once)" = 0 ] && J '.assets[] | select(.name == "once") | .standing == "missing"' <<< "$(status "$N")"; } || check "tombstones: a new asset took its name's versions: $(row "$N" once .)"
J '[.orphans[].name] == ["once"]' <<< "$(status "$N")" || check "tombstones: the old one is no orphan: $(status "$N" | jq -c .orphans)"
dropped "$N" once
printf '  again: {tool: %s/draw, instructions: The first of that name.}\n' "$draw" | added "$N"
cozy project -p "$N" rename once again > "$OUT/tombstones-rename.txt" 2>&1 || check "tombstones: rename: $(tail -c 300 "$OUT/tombstones-rename.txt")"
[ "$(made "$N" again)" = 1 ] || check "tombstones: rename didn't bring its versions: $(counts "$N")"
}

# With its versions lost (their rows gone from project.db, as a bad restore leaves it), what's in `files/` may be all that's left of a
# project's takes: `reclaim --yes` deletes none of it.
lost_lock() {
step "a project: with its versions lost, reclaim --yes deletes nothing, and says what happened"
local L=$OUT/lost-lock count code
mkdir -p "$L"
yaml_in "$L" <<YAML
version: 4
assets:
  made: {tool: $here/project/draw.py/draw, instructions: Paid for.}
YAML
render "$L" made -n 2 || check "lost lock: render: $(tail -c 300 "$OUT/render.txt")"
count=$(ls "$L/files" | wc -l)
[ "$count" -ge 2 ] || check "lost lock: it made $count files"
db_sql "$L" "VACUUM INTO '$OUT/lost-lock.db'"
db_sql "$L" "DELETE FROM versions; DELETE FROM run_files"
J '.deleted != true and (.next // []) == [] and (.lost | contains("lists no versions")) and .files >= 2' <<< "$(cozy --json project -p "$L" reclaim)" || check "lost lock: a dry reclaim: $(cozy --json project -p "$L" reclaim)"
code=0; cozy project -p "$L" reclaim --yes > "$OUT/lost-lock.txt" 2>&1 || code=$?
{ [ "$code" != 0 ] && grep -qF "lists no versions" "$OUT/lost-lock.txt" && grep -q 'put project.db back' "$OUT/lost-lock.txt"; } || check "lost lock: reclaim --yes (exit $code): $(tail -c 300 "$OUT/lost-lock.txt")"
[ "$(ls "$L/files" | wc -l)" = "$count" ] || check "lost lock: reclaim --yes deleted: $(ls "$L/files" | wc -l) of $count"
db_sql "$L" "ATTACH '$OUT/lost-lock.db' AS kept; INSERT INTO versions SELECT * FROM kept.versions; INSERT INTO run_files SELECT * FROM kept.run_files"
cozy project -p "$L" reclaim --yes > "$OUT/lost-lock.txt" 2>&1 || check "lost lock: reclaim with them back: $(tail -c 300 "$OUT/lost-lock.txt")"
[ "$(ls "$L/files" | wc -l)" = "$count" ] || check "lost lock: with them back, reclaim took what they name"

step "a project: with a version landed since its versions were lost, reclaim --yes takes nothing old"
db_sql "$L" "DELETE FROM versions; DELETE FROM run_files"
render "$L" made || check "lost lock: a version since: $(tail -c 300 "$OUT/render.txt")"
count=$(ls "$L/files" | wc -l)
J --argjson n "$((count - 1))" '.unrecorded == $n and (.lost // null) == null' <<< "$(cozy --json project -p "$L" reclaim)" || check "lost lock: what's listed: $(cozy --json project -p "$L" reclaim)"
cozy project -p "$L" reclaim --yes > "$OUT/lost-lock.txt" 2>&1 || check "lost lock: reclaim since: $(tail -c 300 "$OUT/lost-lock.txt")"
[ "$(ls "$L/files" | wc -l)" = "$count" ] || check "lost lock: reclaim --yes took what it had no record of: $(ls "$L/files" | wc -l) of $count"

step "a project: a deleted asset is an orphan, its name starts clean, and restore or reclaim settles it"
local D=$OUT/deleted line
mkdir -p "$D"
yaml_in "$D" <<YAML
version: 4
assets:
  kept: {tool: $here/project/draw.py/draw, instructions: One.}
YAML
render "$D" kept || check "deleted: render: $(tail -c 300 "$OUT/render.txt")"
# What it did, then what restores it, a line each on stdout.
line=$(cozy project -p "$D" delete kept) || check "deleted: delete"
line=${line##*$'\n'}
J '[.orphans[] | select(.name == "kept") | .versions] == [1]' <<< "$(status "$D")" || check "deleted: no orphan: $(status "$D" | jq -c .orphans)"
printf '  kept: {tool: %s/project/draw.py/draw, instructions: Something else.}\n' "$here" | added "$D"
[ "$(made "$D" kept)" = 0 ] || check "deleted: a new asset took its name's versions: $(made "$D" kept)"
dropped "$D" kept
eval "$line" > /dev/null || check "deleted: restore: $line"
[ "$(made "$D" kept)" = 1 ] || check "deleted: restore lost its version: $(made "$D" kept)"
cozy project -p "$D" delete kept > /dev/null || check "deleted: delete again"
cozy project -p "$D" reclaim --yes > "$OUT/deleted.txt" 2>&1 || check "deleted: reclaim: $(tail -c 300 "$OUT/deleted.txt")"
{ J '(.orphans // []) == []' <<< "$(status "$D")" && [ "$(ls "$D/files" | wc -l)" = 0 ]; } || check "deleted: reclaim left: $(ls "$D/files"), $(status "$D" | jq -c .orphans)"
}

# A take discarded: off its box at once, its number never given again, its files there until `reclaim`, and nothing made again.
discards() {
step "a project: a take discarded, and put back"
local T=$OUT/discards second third line back code
mkdir -p "$T"
yaml_in "$T" <<YAML
version: 4
assets:
  hero: {tool: $here/project/draw.py/draw, instructions: One of three.}
  scene: {tool: $here/project/draw.py/draw, instructions: <hero> in a doorway.}
YAML
render "$T" hero -n 3 || check "discards: render: $(tail -c 300 "$OUT/render.txt")"
cozy project -p "$T" approve hero 3 > /dev/null || check "discards: approve"
render "$T" scene || check "discards: render scene: $(tail -c 300 "$OUT/render.txt")"
second=$(take_file "$T" hero image 1); third=$(take_file "$T" hero image 2)
line=$(cozy project -p "$T" delete hero --version 2) || check "discards: delete the middle"
line=${line##*$'\n'}
[ "$line" = "cozy project -p $T restore hero --version 2" ] || check "discards: delete printed: $line"
J '[.versions[].number] == [1, 3] and .discarded == [2] and .versions[-1].approved' <<< "$(held "$T" hero)" || check "discards: after the middle: $(held "$T" hero | jq -c '{versions: [.versions[].number], discarded}')"
[ -n "$(file_at "$T" "$second")" ] || check "discards: its file went at once"
[ "$(sql1 "$T" "SELECT count(*) FROM versions WHERE key = 'hero' AND discarded = 1")" -ge 1 ] || check "discards: project.db has no record of it"
back=$(cozy --json project -p "$T" delete hero --version 3) || check "discards: delete the approved: $back"
J --arg back "cozy project -p $T restore hero --version 3 --approved" '.approved and .stands == 1 and .stale == ["scene"] and .next == [$back]' <<< "$back" || check "discards: the approved's answer: $back"
! grep -q 'approved:' <(cozy project -p "$T" export) || check "discards: approved: stayed"
J '[.versions[].number] == [1] and .discarded == [2, 3] and .asset.standing == "current"' <<< "$(held "$T" hero)" || check "discards: after the approved: $(held "$T" hero | jq -c '{versions: [.versions[].number], discarded, standing: .asset.standing}')"
J '.asset.standing == "out_of_date" and (.versions | length) == 1' <<< "$(held "$T" scene)" || check "discards: its reader: $(held "$T" scene | jq -c .asset)"
eval "$(jq -r '.next[0]' <<< "$back")" > /dev/null || check "discards: restore the approved"
eval "$line" > /dev/null || check "discards: restore the middle: $line"
J '[.versions[].number] == [1, 2, 3] and (.discarded // []) == [] and .versions[-1].approved' <<< "$(held "$T" hero)" || check "discards: Undo: $(held "$T" hero | jq -c '{versions: [.versions[].number], discarded}')"
grep -q 'approved:' <(cozy project -p "$T" export) || check "discards: Undo left no approved:"
J '.asset.standing == "current"' <<< "$(held "$T" scene)" || check "discards: its reader after Undo: $(held "$T" scene | jq -c .asset)"
render "$T" all --missing || check "discards: render what's missing: $(tail -c 300 "$OUT/render.txt")"
[ "$(made "$T" hero) $(made "$T" scene)" = "3 1" ] || check "discards: something ran again: $(counts "$T")"

step "a project: a discarded number is never given again, and reclaim clears only what nothing names"
cozy project -p "$T" delete hero --version 2 --version 3 > "$OUT/discards.txt" || check "discards: delete of two"
[ "$(grep -c ' restore hero --version ' "$OUT/discards.txt")" = 2 ] || check "discards: delete of two printed: $(cat "$OUT/discards.txt")"
cozy project -p "$T" import "${second:7:12}" kept > /dev/null || check "discards: import what another asset then names"
render "$T" hero -n 1 || check "discards: another: $(tail -c 300 "$OUT/render.txt")"
J '[.versions[].number] == [1, 4]' <<< "$(held "$T" hero)" || check "discards: a number given again: $(held "$T" hero | jq -c '[.versions[].number]')"
J '.files >= 1 and .next == ["cozy project reclaim --yes"]' <<< "$(cozy --json project -p "$T" reclaim)" || check "discards: reclaim lists: $(cozy --json project -p "$T" reclaim)"
[ -n "$(file_at "$T" "$third")" ] || check "discards: reclaim with no --yes deleted"
cozy project -p "$T" reclaim --yes > "$OUT/discards.txt" 2>&1 || check "discards: reclaim: $(tail -c 300 "$OUT/discards.txt")"
[ -z "$(file_at "$T" "$third")" ] || check "discards: reclaim left what nothing names"
[ -n "$(file_at "$T" "$second")" ] || check "discards: reclaim took what another asset names"
J '(.discarded // []) == [] and [.versions[].number] == [1, 4]' <<< "$(held "$T" hero)" || check "discards: after reclaim: $(held "$T" hero | jq -c '{versions: [.versions[].number], discarded}')"
code=0; cozy project -p "$T" restore hero --version 3 > "$OUT/discards.txt" 2>&1 || code=$?
{ [ "$code" != 0 ] && grep -q 'reclaim' "$OUT/discards.txt"; } || check "discards: restore after reclaim (exit $code): $(cat "$OUT/discards.txt")"
render "$T" hero -n 1 || check "discards: after reclaim: $(tail -c 300 "$OUT/render.txt")"
J '[.versions[].number] == [1, 4, 5]' <<< "$(held "$T" hero)" || check "discards: reclaim gave a number again: $(held "$T" hero | jq -c '[.versions[].number]')"

step "a project: a box with its last take discarded stays, with none, until it's put back"
printf '  lone: {tool: %s/project/draw.py/draw, instructions: Only one.}\n' "$here" | added "$T"
render "$T" lone || check "discards: lone: $(tail -c 300 "$OUT/render.txt")"
back=$(cozy --json project -p "$T" delete lone --version 1) || check "discards: the last: $back"
J '.stands == null' <<< "$back" || check "discards: the last's answer: $back"
J '.asset.standing == "missing" and .asset.versions == 0 and .discarded == [1]' <<< "$(held "$T" lone)" || check "discards: the box: $(held "$T" lone | jq -c .asset)"
eval "$(jq -r '.next[0]' <<< "$back")" > /dev/null || check "discards: restore the last"
J '.asset.standing == "current"' <<< "$(held "$T" lone)" || check "discards: the last back: $(held "$T" lone | jq -c .asset)"

step "a project: a take built on a discarded one stands, and a cut's clip keeps its file"
local C=$OUT/discards-cut look first
mkdir -p "$C"; plate "$C/look.mp4" testsrc2 440 800
printf 'version: 4\nassets: {}\n' | yaml_in "$C"
look=$(prefix "$C" "$C/look.mp4")
yaml_in "$C" <<YAML
version: 4
assets:
  clash:
    tool: $here/project/shots.py/shoot
    summary: Two lights meet on <look>.
  look:
    approved: $look
  film:
    tool: cozy/edit
    tracks:
      - clips:
          - <clash>
YAML
render "$C" all || check "discards: cut: $(tail -c 300 "$OUT/render.txt")"
first=$(take_file "$C" clash video 0)
render "$C" clash 'pickup.take=<clash.1>' pickup.out_frame=72 || check "discards: a pickup of take 1: $(tail -c 300 "$OUT/render.txt")"
cozy project -p "$C" delete clash --version 1 > /dev/null || check "discards: delete what's built on"
J '[.versions[].number] == [2] and .versions[0].pickup.take == "<clash.1>" and .asset.standing == "current"' <<< "$(held "$C" clash)" || check "discards: what's built on it: $(held "$C" clash | jq -c '{versions, standing: .asset.standing}')"
cozy project -p "$C" reclaim --yes > "$OUT/discards.txt" 2>&1 || check "discards: reclaim with a clip: $(tail -c 300 "$OUT/discards.txt")"
[ -n "$(file_at "$C" "$first")" ] || check "discards: reclaim took what a cut's clip names"

step "a project: a take still being made is canceled, not discarded"
local S=$OUT/discards-going
mkdir -p "$S"
yaml_in "$S" <<YAML
version: 4
assets:
  slow: {tool: $here/project/shots.py/slow, seconds: 30}
YAML
cozy project -p "$S" render slow --queue --on local > "$OUT/discards.txt" 2>&1 || check "discards: slow: $(tail -c 300 "$OUT/discards.txt")"
for _ in $(seq 100); do
  cozy project -p "$S" delete slow --version 1 > "$OUT/discards.txt" 2>&1 || true
  grep -q 'still being made' "$OUT/discards.txt" && break
  sleep 0.1
done
{ grep -q 'still being made' "$OUT/discards.txt" && grep -q 'cozy project cancel slow' "$OUT/discards.txt"; } || check "discards: a running take: $(cat "$OUT/discards.txt")"
cozy project -p "$S" cancel slow > /dev/null 2>&1 || check "discards: cancel"
for _ in $(seq 240); do J '.busy | not' <<< "$(status "$S")" && break; sleep 0.5; done
}

# The key: a run only when an asset has no state. What doesn't change it, what does, reverting, and what reads what.
memo() {
step "a project: render makes what has no state, and nothing else"
local T=$OUT/memo first was reads
mkdir -p "$T"
yaml_in "$T" <<YAML
version: 4
defaults:
  image:
    style: Plain.
assets:
  card:
    tool: $here/project/draw.py/draw
    instructions: A test card.
  poster:
    tool: $here/project/draw.py/draw
    instructions: <card> as a poster.
  copied:
    tool: $here/project/copied.py/copy
    image: <card>
YAML
render "$T" all || check "memo: render all: $(tail -c 400 "$OUT/render.txt")"
[ "$(made "$T" card) $(made "$T" poster) $(made "$T" copied)" = "1 1 1" ] || check "memo: not one version each: $(counts "$T")"
{ J '.versions[0].seed != null' <<< "$(held "$T" card)" && J '.versions[0].seed == null' <<< "$(held "$T" copied)"; } || check "memo: a seed was not drawn for draw, or was for clean, which takes none"
[ "$(sql1 "$T" "SELECT count(*) FROM lock_files WHERE record LIKE '%\"path\"%' OR record LIKE '%\"sha256\"%'")" = 0 ] || check "memo: a file's row holds a path or hash: $(sql "$T" 'SELECT * FROM lock_files')"
[ "$(ls "$T/files" | grep -Ec '^[0-9a-f]{64}')" -ge 3 ] || check "memo: the files aren't named by their hash: $(ls "$T/files")"
[ "$(read_of "$T" poster)" = '["card"]' ] || check "memo: poster doesn't read card: $(read_of "$T" poster)"
grep -qF "$(take_file "$T" card image 0)" <<< "$(asked "$T" poster)" || check "memo: poster's run wasn't given card's file: $(asked "$T" poster)"
first=$(runs "$T")

step "a project: nothing to do, and what changes no key"
render "$T" all --missing || check "memo: render all again: $(tail -c 300 "$OUT/render.txt")"
[ "$(runs "$T")" = "$first" ] || check "memo: a second render all --missing ran something"
was=$(row "$T" card .inputs)
{ printf '# a note\n'; cozy project -p "$T" export; } | yaml_in "$T"
hand "$T" 's/instructions: A test card\./instructions: "  A test card.  "/'
[ "$(row "$T" card .inputs)" = "$was" ] || check "memo: a comment, quotes or trailing spaces moved card's key"
hand "$T" 's/instructions: "  A test card.  "/instructions: A test card./'

step "a project: a text edit, and reverting returns the old output with no run"
local old_file; old_file=$(row "$T" card .path)
hand "$T" 's/instructions: A test card\./instructions: Another card./'
[ "$(row "$T" card .status)" = '"out of date: its inputs changed"' ] || check "memo: a text edit: card is $(row "$T" card .status)"
[ "$(row "$T" poster .status)" = '"waits for what it reads"' ] || check "memo: poster reads card, which has no state: $(row "$T" poster .status)"
render "$T" poster --missing || check "memo: render poster: $(tail -c 300 "$OUT/render.txt")"
[ "$(made "$T" card) $(made "$T" poster)" = "2 2" ] || check "memo: rendering a reader did not make what it reads first: $(counts "$T")"
first=$(runs "$T")
hand "$T" 's/instructions: Another card\./instructions: A test card./'
J '[.assets[] | select(.name == "card" or .name == "poster") | .status] == ["current", "current"]' <<< "$(status "$T")" || check "memo: reverted, not current: $(status "$T" | jq -c '[.assets[] | {name, status}]')"
[ "$(row "$T" card .path)" = "$old_file" ] || check "memo: reverted, card reads $(row "$T" card .path), not $old_file"
render "$T" all --missing || check "memo: render all, reverted"
[ "$(runs "$T")" = "$first" ] || check "memo: reverting ran something"

step "a project: defaults and inputs are in the key; a release in a tool is not"
hand "$T" 's/style: Plain\./style: Bold./'
J '([.stale[]?.name] | index("card")) and ([.missing[]?] | index("poster"))' <<< "$(status "$T")" || check "memo: a defaults edit did not put card out of date, and poster waiting on it: $(status "$T" | jq -c '{stale, missing}')"
hand "$T" 's/style: Bold\./style: Plain./'
added "$T" <<YAML
  keyed:
    tool: _/birefnet@9.9.9
    image: <card>
YAML
was=$(row "$T" keyed .inputs)
hand "$T" 's#_/birefnet\@9\.9\.9#birefnet#'
[ "$(row "$T" keyed .inputs)" = "$was" ] || check "memo: a release or _/ in a tool moved the key"
hand "$T" 's#tool: birefnet#tool: ~/a/checkout/of/birefnet#'
[ "$(row "$T" keyed .inputs)" = "$was" ] || check "memo: a local checkout of the package moved the key"
hand "$T" 's#tool: ~/a/checkout/of/birefnet#tool: other#'
[ "$(row "$T" keyed .inputs)" != "$was" ] || check "memo: another package did not move the key"
hand "$T" 's#tool: other#tool: birefnet\n    steps: 2#'
[ "$(row "$T" keyed .inputs)" != "$was" ] || check "memo: an input did not move the key"
[ "$(status "$T" | jq -c '[.warnings[]?.message | select(test("no section"))]')" = "[]" ] || check "memo: a format 4 file warned of a section"
dropped "$T" keyed

step "a project: each render a new version, and -n"
first=$(made "$T" card)
render "$T" card || check "memo: render card: $(tail -c 300 "$OUT/render.txt")"
[ "$(made "$T" card)" = "$((first + 1))" ] || check "memo: a render made $(made "$T" card) of card, from $first"
render "$T" card -n 2 || check "memo: -n 2: $(tail -c 300 "$OUT/render.txt")"
[ "$(made "$T" card)" = "$((first + 3))" ] || check "memo: -n 2 made $(made "$T" card) of card, from $((first + 1))"
J '[.versions[-3:][].seed] | unique | length == 3' <<< "$(held "$T" card)" || check "memo: a render and -n did not draw new seeds"
J '[.versions[].number] == [range(1; (.versions | length) + 1)]' <<< "$(held "$T" card)" || check "memo: card's numbers are not in order"
render "$T" copied || check "memo: copied over a new card"
first=$(made "$T" copied)
render "$T" copied || check "memo: a render of a function with no seed: $(tail -c 300 "$OUT/render.txt")"
says memo "takes no seed"
[ "$(made "$T" copied)" = "$first" ] || check "memo: a render of a function with no seed made a version"
code=0; cozy project -p "$T" render card -n 2 --approve --on local > "$OUT/render.txt" 2>&1 || code=$?
{ [ "$code" != 0 ] && says memo --approve; } || check "memo: -n 2 --approve was not refused (exit $code)"
}

# A run belongs to a project, as a commit to a repository: naming none it refuses (the folder it's run in names none: cl-819), and in
# one its outputs land in files/ by hash, where --file and approve name them, and reclaim takes them once nothing does.
project_runs() {
step "a project: a run outside one refuses, and in one lands in files/"
local R=$OUT/run-project away code hash other real lines
away=$(mktemp -d)
code=0; (cd "$away" && cozy run "$here/project/draw.py" instructions=outside) > "$OUT/outside.txt" 2>&1 || code=$?
rmdir "$away"
{ [ "$code" = 2 ] && grep -qF 'name the project: -p <name>, or COZY_PROJECT=<name>' "$OUT/outside.txt" && grep -qF 'next: cozy project new <name>' "$OUT/outside.txt"; } || check "runs: outside a project: exit $code: $(tail -c 300 "$OUT/outside.txt")"
cozy project new "$R" > /dev/null || check "runs: project new"
yaml_in "$R" <<YAML
version: 4
assets:
  kyogre:
    tool: $here/project/draw.py/draw
    instructions: A blue whale.
YAML
real=$(status "$R" | jq -r .id)
(cd "$R" && cozy run -p "$R" "$here/project/draw.py" instructions=first) > "$OUT/run-first.txt" 2>&1 || check "runs: a run in the project: $(tail -c 300 "$OUT/run-first.txt")"
hash=$(sed -n 's/^image: \([0-9a-f]\{12\}\)$/\1/p' "$OUT/run-first.txt"); [ -n "$hash" ] || check "runs: no line to paste: $(tail -c 300 "$OUT/run-first.txt")"
ls "$R/files/$hash"* > /dev/null 2>&1 || check "runs: $hash is not in files/: $(ls "$R/files")"
[ "$(find "$R/.runs" -type f 2> /dev/null | wc -l)" = 0 ] || check "runs: a file stayed where it landed: $(find "$R/.runs" -type f)"
{ J '.run_files == 1' <<< "$(status "$R")" && [ -n "$(file_at "$R" "$hash")" ] && [ "$(sql1 "$R" "SELECT count(*) FROM run_files WHERE run > 0 AND output != ''")" = 1 ]; } || check "runs: its row: $(sql "$R" 'SELECT * FROM run_files')"
J '.run_files == 1' <<< "$(status "$R")" || check "runs: status does not count it: $(status "$R" | jq -c .run_files)"
cozy run -p "$R" "$here/project/draw.py" instructions=second > "$OUT/run-second.txt" 2>&1 || check "runs: -p: $(tail -c 300 "$OUT/run-second.txt")"
other=$(sed -n 's/^image: \([0-9a-f]\{12\}\)$/\1/p' "$OUT/run-second.txt"); [ -n "$other" ] && [ "$other" != "$hash" ] || check "runs: -p made no new file: $(tail -c 300 "$OUT/run-second.txt")"
(cd "$R" && cozy run -p "$R" "$here/project/copied.py" --file "image=$hash") > "$OUT/run-copy.txt" 2>&1 || check "runs: --file by hash: $(tail -c 300 "$OUT/run-copy.txt")"
J '.run_files == 3' <<< "$(status "$R")" || check "runs: three runs, and status says $(status "$R" | jq -c .run_files)"

step "a project: run ls in it lists its runs, and --all every one"
(cd "$R" && cozy --json run ls -p "$R") > "$OUT/runs-in.json" 2> "$OUT/runs-in.err" || check "runs: run ls in it: $(tail -c 300 "$OUT/runs-in.err")"
J --arg p "$real" '(.runs | length == 3) and all(.runs[]; .project_id == $p)' "$OUT/runs-in.json" || check "runs: run ls in it: $(jq -c '[.runs[] | {number, project_id}]' "$OUT/runs-in.json")"
(cd "$R" && cozy --json run ls -p "$R" --all --limit 500) > "$OUT/runs-all.json" 2> "$OUT/runs-all.err" || check "runs: run ls --all: $(tail -c 300 "$OUT/runs-all.err")"
J --arg p "$real" 'any(.runs[]; .project_id != $p)' "$OUT/runs-all.json" || check "runs: run ls --all lists only this project"

step "a project: approve takes any file of its, by hash"
cozy project -p "$R" approve kyogre "$hash" > "$OUT/approve.txt" 2>&1 || check "runs: approve by hash: $(tail -c 300 "$OUT/approve.txt")"
grep -Eq "^    approved: \"?$hash\"?\$" <(cozy project -p "$R" export) || check "runs: approve wrote: $(grep -n 'approved' <(cozy project -p "$R" export))"
J '.assets[] | select(.name == "kyogre") | .pinned and (.status | startswith("approved"))' <<< "$(status "$R")" || check "runs: kyogre: $(row "$R" kyogre .)"
J '.run_files == 2' <<< "$(status "$R")" || check "runs: a pinned file still counts as loose: $(status "$R" | jq -c .run_files)"

step "a project: reclaim takes a run's file once nothing names it"
lines=$(cozy --json project -p "$R" reclaim | jq .files); [ "$lines" -ge 2 ] || check "runs: reclaim lists $lines files"
J '.next == ["cozy project reclaim --yes"]' <<< "$(cozy --json project -p "$R" reclaim)" || check "runs: reclaim does not say what's next"
ls "$R/files/$other"* > /dev/null 2>&1 || check "runs: reclaim with no --yes deleted"
cozy project -p "$R" reclaim --yes > "$OUT/reclaim.txt" 2>&1 || check "runs: reclaim --yes: $(tail -c 300 "$OUT/reclaim.txt")"
! ls "$R/files/$other"* > /dev/null 2>&1 || check "runs: reclaim --yes left a file nothing names"
ls "$R/files/$hash"* > /dev/null 2>&1 || check "runs: reclaim --yes took the pinned file"
{ [ "$(sql1 "$R" "SELECT count(*) FROM run_files")" = 1 ] && [ "$(sql1 "$R" "SELECT substr(sha256, 8, 12) FROM run_files")" = "$hash" ] && [ -n "$(file_at "$R" "$hash")" ]; } || check "runs: rows left: $(sql "$R" 'SELECT * FROM run_files')"
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
  (cd "$R" && cozy --json run -p "$R" "$OUT/$name.py/echo" text=abc --on local) > "$OUT/named-$name.ndjson" 2> "$OUT/named-$name.err" || check "runs: $name.py: $(tail -c 300 "$OUT/named-$name.err")"
  tail -1 "$OUT/named-$name.ndjson" | J '.run.result.back == "cba"' || check "runs: $name.py did not answer: $(tail -1 "$OUT/named-$name.ndjson" | jq -c .run.reason)"
done
}

# A run's output that files/ can't take (cl-826): it stands only in .runs/, where its run's record names it. reclaim --yes leaves it,
# says so by its sha256, and the run still serves it.
reclaim_unheld() {
step "a project: reclaim --yes leaves an output files/ couldn't take"
local U=$OUT/reclaim-unheld n digest left said
cozy project new "$U" > /dev/null || check "unheld: project new"
mkdir -p "$U/files" && chmod a-w "$U/files"
if touch "$U/files/.writable" 2> /dev/null; then
  echo "unheld: files/ takes writes here even read-only (root, or Windows): nothing makes it refuse"
  rm -rf "$U"; return 0
fi
(cd "$U" && cozy --json run -p "$U" "$here/project/draw.py" instructions=unheld) > "$OUT/unheld.ndjson" 2>&1 || check "unheld: a run: $(tail -c 300 "$OUT/unheld.ndjson")"
n=$(run_number "$OUT/unheld.ndjson")
cozy --json run show "$n" --once > "$OUT/unheld-run.json" 2>&1 || true
digest=$(jq -r '.output[0].sha256 // empty' "$OUT/unheld-run.json" 2> /dev/null || true)
left=$(find "$U/.runs" -type f 2> /dev/null || true)
{ [ -n "$digest" ] && [ "$(printf '%s\n' "$left" | grep -c .)" = 1 ] && [ "sha256:$(sha "$left")" = "$digest" ] && [ -z "$(ls -A "$U/files")" ]; } \
  || check "unheld: files/ refused nothing, or the output isn't in .runs/: ${digest:-no digest}, $(ls -A "$U/files" "$U/.runs" 2>&1 | tr '\n' ' ')"
J '[.warnings[]?.message] | any(contains("stands in .runs/"))' "$OUT/unheld-run.json" || check "unheld: no warning says where it stands: $(jq -c .warnings "$OUT/unheld-run.json" 2> /dev/null)"
# files/ takes writes again, as once its owner fixes it: what a run left there before it could still be the only copy.
chmod u+w "$U/files"
cozy --json project -p "$U" reclaim > "$OUT/unheld-dry.json" 2>&1 || check "unheld: reclaim: $(tail -c 300 "$OUT/unheld-dry.json")"
J --arg d "$digest" '(.unheld | length) == 1 and .unheld[0].sha256 == $d and (.unheld[0].path | startswith(".runs/")) and .files == 0' "$OUT/unheld-dry.json" \
  || check "unheld: reclaim doesn't list it as unheld: $(head -c 300 "$OUT/unheld-dry.json")"
cozy project -p "$U" reclaim --yes > "$OUT/unheld-reclaim.txt" 2>&1 || check "unheld: reclaim --yes: $(tail -c 300 "$OUT/unheld-reclaim.txt")"
said=$(grep -F "files/ doesn't hold what runs left here, so it stays" "$OUT/unheld-reclaim.txt" || true)
{ [ -n "$said" ] && grep -qF "${digest:0:19}" <<< "$said"; } || check "unheld: reclaim --yes doesn't say what stays: $(tail -c 300 "$OUT/unheld-reclaim.txt")"
{ [ -f "$left" ] && [ "sha256:$(sha "$left")" = "$digest" ]; } || check "unheld: reclaim --yes deleted an output files/ doesn't hold: $(ls -A "$U" "$U/.runs" 2>&1 | tr '\n' ' ')"
rm -rf "$OUT/unheld-out"
cozy run show "$n" --once -o "$OUT/unheld-out" > "$OUT/unheld-show.txt" 2>&1 || true
{ [ "$(find "$OUT/unheld-out" -type f 2> /dev/null | wc -l)" = 1 ] && [ "sha256:$(sha "$(find "$OUT/unheld-out" -type f)")" = "$digest" ]; } \
  || check "unheld: after reclaim --yes its run can't give its output: $(tail -c 300 "$OUT/unheld-show.txt")"
echo "unheld: run $n output ${digest:-none}; reclaim --yes left ${left#"$U"/} sha256:$(sha "$left" 2> /dev/null || true); said: ${said:-nothing}"
rm -rf "$U"
}

# by_content <script>: a one-file package as `protocol::code` sends it, the script and then its tree's document, each PUT by its
# digest; the tree's digest on stdout.
by_content() {
  local hex doc tree
  hex=$(sha256sum "$1" | cut -c1-64)
  api -sS -o /dev/null -X PUT --data-binary "@$1" "http://$ADDR/v1/files/sha256:$hex" || return 1
  doc=$(jq -cn --arg h "$hex" --argjson n "$(stat -c %s "$1")" --arg p "${1##*/}" '{entries: [{blob: {length: $n, sha256: $h}, kind: "file", path: $p}]}')
  tree=$(printf %s "$doc" | sha256sum | cut -c1-64)
  printf %s "$doc" | api -sS -o /dev/null -X PUT --data-binary @- "http://$ADDR/v1/files/sha256:$tree" || return 1
  echo "sha256:$tree"
}

# What a page sends, or any client on another computer: a project by its id, local code by content (its tree, as `protocol::code`
# makes it), a payload as text, a file by its hash, and no path. A file given to a run joins its project, so the run can be asked
# again once what held it is gone.
path_free() {
step "a project: a run with no path sent"
local R=$OUT/path-free lock=$HOME/.cozy/calcifer/calcifer.lock.holder id pic hex other n asked input code w h url script tree copied
cozy project new "$R" > /dev/null || check "path-free: project new"
id=$(status "$R" | jq -r .id)
ADDR=$(jq -r .address "$lock") TOKEN=$(jq -r .token "$lock")
script=$here/project/draw.py
tree=$(by_content "$script") || check "path-free: PUT draw.py by content"
asked=$(ask "$(jq -n --arg t "$tree" --arg p "$id" '{target: "draw", code: {tree: $t, at: "draw.py"}, project: $p, on: "local", input_text: "instructions: with no path\nseed: 7\n"}')")
n=$(jq -r '.run.number // empty' <<< "$asked")
[ -n "$n" ] || { check "path-free: a run by the project's id, local code by content and input_text: $asked"; return; }
ended "$n" > "$OUT/free-draw.json"
J --arg i "$id" '.status == "completed" and .project_id == $i and .project == "path-free"' "$OUT/free-draw.json" || check "path-free: the run: $(jq -c '{status, project, project_id, reason}' "$OUT/free-draw.json")"
J '.request.input.instructions.text == "with no path" and .request.seed == 7 and (.request.target | endswith("/draw"))' "$OUT/free-draw.json" || check "path-free: request: $(jq -c .request "$OUT/free-draw.json")"
J '.output | all(has("path") | not)' "$OUT/free-draw.json" || check "path-free: an output names a path of Calcifer's: $(jq -c .output "$OUT/free-draw.json")"
url=$(jq -r '.output[0].url' "$OUT/free-draw.json") pic=$OUT/free-output.png
[ "$(api -o "$pic" -w '%{http_code}' "http://$ADDR$url")" = 200 ] && [ "sha256:$(sha256sum "$pic" | cut -c1-64)" = "$(jq -r '.output[0].sha256' "$OUT/free-draw.json")" ] || check "path-free: $url does not serve its output"
w=$(ffprobe -v error -select_streams v:0 -show_entries stream=width -of csv=p=0 "$pic") h=$(ffprobe -v error -select_streams v:0 -show_entries stream=height -of csv=p=0 "$pic")
J --argjson w "$w" --argjson h "$h" '.output[0] | .width == $w and .height == $h' "$OUT/free-draw.json" || check "path-free: measured $(jq -c '.output[0] | {width, height}' "$OUT/free-draw.json"), not $w x $h"
[ "$(api --get --data-urlencode "project=$id" "http://$ADDR/v1/runs" | jq -c '[.runs[].number]')" = "$(api --get --data-urlencode "project=path-free" "http://$ADDR/v1/runs" | jq -c '[.runs[].number]')" ] || check "path-free: GET /v1/runs by id and by name differ"
[ "$(api -o "$OUT/free-unknown.json" -w '%{http_code}' "http://$ADDR/v1/runs?project=project-0000000000000000")" = 404 ] && J '.error.code == "project_unknown"' "$OUT/free-unknown.json" || check "path-free: GET /v1/runs of an unknown id: $(head -c 200 "$OUT/free-unknown.json")"
J '.error.code == "project_unknown"' <<< "$(ask "$(jq -n --arg t "$tree" '{target: "draw", code: {tree: $t, at: "draw.py"}, project: "project-0000000000000000", on: "local", input_text: "instructions: nowhere"}')")" || check "path-free: POST /v1/runs of an unknown id was not refused"
J '.error.code == "target_by_path"' <<< "$(ask "$(jq -n --arg t "$script" --arg p "$id" '{target: $t, project: $p, on: "local", input_text: "instructions: by path"}')")" || check "path-free: local code by a path was not refused"

step "a project: a file by its hash joins the project, and the run can be asked again"
copied=$(by_content "$here/project/copied.py") || check "path-free: PUT copied.py by content"
pic=$OUT/free.png
ffmpeg -v error -y -f lavfi -i "color=c=0x$(printf %06x $((RANDOM * RANDOM % 16777216))):size=64x48" -frames:v 1 "$pic"
hex=$(sha "$pic")
code=$(api -o "$OUT/free-put.json" -w '%{http_code}' -X PUT --data-binary "@$pic" "http://$ADDR/v1/files/sha256:$hex")
{ [ "$code" = 201 ] || [ "$code" = 200 ]; } || check "path-free: PUT: $code $(head -c 200 "$OUT/free-put.json")"
asked=$(ask "$(jq -n --arg t "$copied" --arg p "$id" --arg x "image: sha256:$hex" '{target: "", code: {tree: $t, at: "copied.py"}, project: $p, on: "local", input_text: $x}')")
n=$(jq -r '.run.number // empty' <<< "$asked")
[ -n "$n" ] || { check "path-free: a run given a held file: $asked"; return; }
J '.status == "completed"' <<< "$(ended "$n")" || check "path-free: run $n given a held file did not complete"
[ -f "$R/files/$hex.png" ] || check "path-free: the file a run was given is not in files/: $(ls "$R/files")"
[ -n "$(file_at "$R" "$hex")" ] || check "path-free: the file a run was given is not in the project"
# What held it is gone, as a day later.
rm -f "$HOME/.cozy/calcifer/files/$hex"
[ "$(api -o /dev/null -w '%{http_code}' -I "http://$ADDR/v1/files/sha256:$hex")" = 404 ] || check "path-free: the held file is still held"
input=$(cozy --json --full run show "$n" | jq -c .request.input)
J --arg s "sha256:$hex" '.image == $s' <<< "$input" || check "path-free: request.input: $input"
asked=$(ask "$(jq -n --arg t "$copied" --arg p "$id" --argjson i "$input" '{target: "", code: {tree: $t, at: "copied.py"}, project: $p, on: "local", input: $i}')")
n=$(jq -r '.run.number // empty' <<< "$asked")
[ -n "$n" ] && J '.status == "completed"' <<< "$(ended "$n")" || check "path-free: asking it again: $asked"
# A hash prefix, in text and in a request file.
asked=$(ask "$(jq -n --arg t "$copied" --arg p "$id" --arg x "image: ${hex:0:12}" '{target: "", code: {tree: $t, at: "copied.py"}, project: $p, on: "local", input_text: $x}')")
n=$(jq -r '.run.number // empty' <<< "$asked")
[ -n "$n" ] && J '.status == "completed"' <<< "$(ended "$n")" || check "path-free: a hash prefix in input_text: $asked"
printf 'image: %s\n' "${hex:0:12}" > "$R/req.yaml"
cozy run -p "$R" "$here/project/copied.py" --input "$R/req.yaml" > "$OUT/free-input.txt" 2>&1 || check "path-free: a hash prefix in --input: $(tail -c 300 "$OUT/free-input.txt")"
J '.error.code == "file_not_held"' <<< "$(ask "$(jq -n --arg t "$copied" --arg p "$id" --arg x "image: sha256:$(printf %064d 1)" '{target: "", code: {tree: $t, at: "copied.py"}, project: $p, on: "local", input_text: $x}')")" || check "path-free: a hash nothing holds was not refused file_not_held"
# A person's own file is copied: changing it later changes nothing of the project's.
other=$OUT/free-other.png
ffmpeg -v error -y -f lavfi -i "color=c=0x$(printf %06x $((RANDOM * RANDOM % 16777216))):size=48x64" -frames:v 1 "$other"
hex=$(sha "$other")
cozy run -p "$R" "$here/project/copied.py" --file "image=$other" > "$OUT/free-other.txt" 2>&1 || check "path-free: a run given a person's file: $(tail -c 300 "$OUT/free-other.txt")"
printf x >> "$other"
[ "$(sha "$R/files/$hex.png")" = "$hex" ] || check "path-free: the project's copy changed with the person's file"

step "a project: GET /v1/targets says what a person runs and a function's contract, and the CLI prints them"
# A package, as a run reads it: its machine's Runtime, kept from installing this source, whatever PyPI's newest can read.
cozy run -p "$R" "$here/project/shots.py" > "$OUT/free-functions.txt" 2>&1 || check "path-free: run of a package names no function: $(tail -c 300 "$OUT/free-functions.txt")"
grep -q shoot "$OUT/free-functions.txt" || check "path-free: its functions: $(tail -c 300 "$OUT/free-functions.txt")"
tree=$(by_content "$here/project/shots.py") || check "path-free: PUT shots.py by content"
api --get --data-urlencode "tree=$tree" --data-urlencode "at=shots.py" "http://$ADDR/v1/targets" > "$OUT/free-target.json"
J '(.functions | length > 1) and (.next | length > 0) and (has("contract") | not)' "$OUT/free-target.json" || check "path-free: targets of a package: $(jq -c '{functions, next}' "$OUT/free-target.json")"
api --get --data-urlencode "target=shoot" --data-urlencode "tree=$tree" --data-urlencode "at=shots.py" "http://$ADDR/v1/targets" > "$OUT/free-function.json"
J '.function == "shoot" and (.contract | contains("/shoot")) and (.request.fields | length > 0)' "$OUT/free-function.json" || check "path-free: targets of a function: $(jq -c '{function, contract}' "$OUT/free-function.json")"
cozy run "$here/project/shots.py/shoot" --describe > "$OUT/free-describe.txt" 2>&1 && grep -q '/shoot' "$OUT/free-describe.txt" || check "path-free: --describe: $(tail -c 300 "$OUT/free-describe.txt")"
# The same source no run has read: read from its source by PyPI's newest Runtime, which answers, or says what would read it.
cp "$here/project/shots.py" "$OUT/free-never-run.py"; printf '\n# never run\n' >> "$OUT/free-never-run.py"
tree=$(by_content "$OUT/free-never-run.py") || check "path-free: PUT free-never-run.py by content"
api --get --data-urlencode "tree=$tree" --data-urlencode "at=free-never-run.py" "http://$ADDR/v1/targets" > "$OUT/free-never-run.json"
J '(.functions | length > 1) or (.error.code == "describe_failed" and (.error.next[0] | startswith("cozy run ")))' "$OUT/free-never-run.json" || check "path-free: targets of a source no run has read: $(head -c 300 "$OUT/free-never-run.json")"

step "a project: what a machine holds, and Calcifer's log, by their routes"
J '.machine == "local" and (.packages | type == "array")' <<< "$(api "http://$ADDR/v1/machines/local/packages")" || check "path-free: GET /v1/machines/local/packages"
J '.machine == "local" and (.models | type == "array")' <<< "$(api "http://$ADDR/v1/machines/local/models")" || check "path-free: GET /v1/machines/local/models"
api "http://$ADDR/v1/machines/local/logs/calcifer?lines=5" > "$OUT/free-log.txt"
{ [ -s "$OUT/free-log.txt" ] && [ "$(wc -l < "$OUT/free-log.txt")" -le 5 ]; } || check "path-free: logs/calcifer?lines=5: $(wc -l < "$OUT/free-log.txt") lines"
[ "$(api -o "$OUT/free-nolog.json" -w '%{http_code}' "http://$ADDR/v1/machines/hikaru/logs/calcifer")" = 400 ] && J '.error.code == "log_not_kept"' "$OUT/free-nolog.json" || check "path-free: logs/calcifer elsewhere: $(head -c 200 "$OUT/free-nolog.json")"
api --max-time 3 -N -H 'Accept: text/event-stream' "http://$ADDR/v1/machines/local/logs/calcifer?lines=5" > "$OUT/free-log.sse" 2> /dev/null || true
grep -q '"type":"lines"' "$OUT/free-log.sse" || check "path-free: logs/calcifer did not follow: $(head -c 200 "$OUT/free-log.sse")"
}

# Approved and pinned are one thing, a box's `approved`: one line of its export changes, and everything else stands.
pins() {
step "a project: approve sets one field"
local T=$OUT/pinned before after version file changed
mkdir -p "$T"
yaml_in "$T" <<YAML
version: 4
assets:
  card:
    tool: $here/project/draw.py/draw
    instructions: A test card.
    aspect_ratio: "16:9"

  flat: {instructions: A flat one., tool: $here/project/draw.py/draw}
  copied:
    tool: $here/project/copied.py/copy
    image: <card>
  poster:
    tool: $here/project/draw.py/draw
    instructions: |
      <card> as a poster.
      # not a comment, though it looks like one
YAML
render "$T" all || check "pins: render all: $(tail -c 300 "$OUT/render.txt")"
render "$T" card -n 2 || check "pins: render card -n 2: $(tail -c 300 "$OUT/render.txt")"
before=$OUT/pins-before.yaml; cozy project -p "$T" export > "$before"
# An edit saved just before the approve stands.
hand "$T" 's/instructions: A flat one\./instructions: A flat one, edited just now./'
cozy project -p "$T" approve card 1 > "$OUT/approve.txt" 2>&1 || check "pins: approve card 1: $(tail -c 300 "$OUT/approve.txt")"
grep -q 'A flat one, edited just now' <(cozy project -p "$T" export) || check "pins: an edit saved just before approve is gone"
file=$(take_file "$T" card image 0); file=${file#sha256:}
hand "$T" 's/instructions: A flat one, edited just now\./instructions: A flat one./'
diff "$before" <(cozy project -p "$T" export) > "$OUT/approve.diff" || true
{ [ "$(grep -c '^[<>]' "$OUT/approve.diff")" = 1 ] && grep -Eq "^> *approved: \"?${file:0:12}\"?$" "$OUT/approve.diff"; } || check "pins: approve changed more than its one line: $(cat "$OUT/approve.diff")"
J '.assets[] | select(.name == "card") | .approved == 1 and .pinned and .status == "approved: version 1"' <<< "$(status "$T")" || check "pins: status does not say card is pinned to 1: $(row "$T" card .)"
[ "$(sql1 "$T" "SELECT count(*) FROM versions WHERE record LIKE '%\"pin\"%'")" = 0 ] || check "pins: a version's record holds an approval"

step "a project: a pin stops a run, and status says it's out of date"
hand "$T" 's/instructions: A test card\./instructions: Another card./'
version=$(made "$T" card)
render "$T" card --missing || check "pins: render a pinned card: $(tail -c 300 "$OUT/render.txt")"
[ "$(made "$T" card)" = "$version" ] || check "pins: a text edit re-rendered a pinned card"
changed=$(row "$T" card .status)
[ "$changed" = '"approved: version 1; its inputs changed: its inputs"' ] || check "pins: status of an edited pin is $changed"
J '.standing == "out_of_date"' <<< "$(row "$T" card .)" || check "pins: an approved asset with new inputs doesn't show out of date: $(row "$T" card .)"
render "$T" card || check "pins: naming a pinned card: $(tail -c 300 "$OUT/render.txt")"
[ "$(made "$T" card)" = "$((version + 1))" ] || check "pins: naming it did not make a take to look at"
[ "$(row "$T" card .approved)" = 1 ] || check "pins: naming it moved the pin"
hand "$T" 's/instructions: Another card\./instructions: A test card./'

step "a project: what makes an approved asset leaves what reads it, its description doesn't, and it leaves itself"
after=$(row "$T" poster .inputs)
hand "$T" 's/instructions: A test card\./instructions: Yet another card./'
[ "$(row "$T" poster .inputs)" = "$after" ] || check "pins: an approved asset's instructions moved what reads it"
hand "$T" 's/instructions: Yet another card\./instructions: A test card./'
hand "$T" 's/^    aspect_ratio: "16:9"$/$&\n    description: Words for a reader./'
[ "$(row "$T" poster .inputs)" != "$after" ] || check "pins: a description didn't move what reads it"
J '.standing == "current"' <<< "$(row "$T" card .)" || check "pins: a description moved its own asset: $(row "$T" card .)"
hand "$T" '$_ = "" if /^    description: Words for a reader\.$/'

step "a project: unpin takes the line away"
cozy project -p "$T" unapprove card > "$OUT/unpin.txt" 2>&1 || check "pins: unpin card: $(tail -c 300 "$OUT/unpin.txt")"
diff "$before" <(cozy project -p "$T" export) > "$OUT/unpin.diff" || check "pins: unpin did not give back the box: $(cat "$OUT/unpin.diff")"
J '.assets[] | select(.name == "card") | (.pinned | not) and .approved == null' <<< "$(status "$T")" || check "pins: unpinned, status is $(row "$T" card .)"

step "a project: a pin over an input keeps it, and a line of its own"
render "$T" copied > /dev/null 2>&1 || true
cozy project -p "$T" approve copied 1 > "$OUT/approve.txt" 2>&1 || check "pins: approve copied 1: $(tail -c 300 "$OUT/approve.txt")"
{ grep -q '^    image: <card>$' <(cozy project -p "$T" export) && grep -q '^    approved: ' <(cozy project -p "$T" export); } || check "pins: a pin over <card> lost it: $(grep -n 'image:' <(cozy project -p "$T" export))"
cozy project -p "$T" unapprove copied > "$OUT/unpin.txt" 2>&1 || check "pins: unpin copied: $(tail -c 300 "$OUT/unpin.txt")"
diff "$before" <(cozy project -p "$T" export) > "$OUT/unpin.diff" || check "pins: unpin copied did not give back the box: $(cat "$OUT/unpin.diff")"
cozy project -p "$T" approve flat 1 > "$OUT/approve.txt" 2>&1 || check "pins: approve an asset on one line: $(tail -c 300 "$OUT/approve.txt")"
[ "$(row "$T" flat .approved)" = 1 ] || check "pins: flat: $(grep -n -A3 'flat:' <(cozy project -p "$T" export))"
cozy project -p "$T" unapprove flat > "$OUT/unpin.txt" 2>&1 || check "pins: unpin flat: $(tail -c 300 "$OUT/unpin.txt")"
diff "$before" <(cozy project -p "$T" export) > "$OUT/unpin.diff" || check "pins: unpin flat did not give back the box: $(cat "$OUT/unpin.diff")"

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
printf 'version: 4\nassets: {}\n' | yaml_in "$C"
pic=$(prefix "$C" "$C/teal.png")
yaml_in "$C" <<YAML
version: 4
assets:
  teal:
    approved: $pic
  wrong:
    tool: $here/project/draw.py/draw
    instructions: It renders without that.
    approved: cafe0000
  nothing:
    approved: ./path.png
  no_package:
    instructions: Nothing makes me.
  only_words:
    description: Nothing makes me, and I say what I am.
  no_kind:
    tool: $here/project/draw.py/draw
    instructions: An image, then.
  any_tool:
    tool: $here/project/draw.py/draw
    instructions: Nor me either.
YAML
J '.assets[] | select(.name == "teal") | .pinned and (.tool == null) and (.approved == null)' <<< "$(status "$C")" || check "pins: a constant: $(row "$C" teal .)"
J '.assets[] | select(.name == "teal") | .kind == "image"' <<< "$(status "$C")" || check "pins: a constant's kind is not its file's: $(row "$C" teal .)"
J '[.warnings[].message] | any(contains("no file'"'"'s hash starts with cafe0000")) and any(contains("is not a hash"))' <<< "$(status "$C")" || check "pins: a bad pin is not warned of: $(status "$C" | jq -c .warnings)"
render "$C" teal || check "pins: render a constant: $(tail -c 300 "$OUT/render.txt")"
says pins "is a constant"
code=0; cozy project -p "$C" unapprove teal > "$OUT/unpin.txt" 2>&1 || code=$?
[ "$code" != 0 ] || check "pins: unpinning a constant took its one line"
render "$C" wrong || check "pins: wrong's bad pin stopped it: $(tail -c 300 "$OUT/render.txt")"
[ "$(made "$C" wrong)" = 1 ] || check "pins: wrong was not made without its bad pin"

step "a project: an asset with no package is its words, one with no kind an image, and the rest goes on"
# A box is its words and an optional tool (27c5db3f5): with none, it's only words, and nothing's wrong with it.
J '.assets[] | select(.name == "no_package") | .standing == "current" and .status == "only words"' <<< "$(status "$C")" || check "pins: an asset with no package is not its words: $(row "$C" no_package .)"
J '[(.warnings // [])[] | select(.name == "no_package")] == []' <<< "$(status "$C")" || check "pins: an asset that is its words is warned of: $(status "$C" | jq -c .warnings)"
J '.assets[] | select(.name == "no_kind") | .kind == "image"' <<< "$(status "$C")" || check "pins: an asset with no kind is not an image: $(row "$C" 'no_kind' .)"
J '.assets[] | select(.name == "only_words") | .standing == "current" and .status == "only words"' <<< "$(status "$C")" || check "pins: an asset that is only words: $(row "$C" only_words .)"
hand "$C" 's/It renders without that\./Rendered again./'
render "$C" all --missing || check "pins: render all with what can't be made: $(tail -c 400 "$OUT/render.txt")"
[ "$(made "$C" any_tool)" = 1 ] || check "pins: a tool makes what it makes, whatever a file once said: $(made "$C" any_tool)"
[ "$(made "$C" wrong)" = 2 ] || check "pins: what could be made was not: wrong has $(made "$C" wrong)"
}

# What a reader gets: an asset's file and description, none of what makes that asset.
entities() {
step "a project: defaults are said, a description is words, and it reaches what mentions its box"
local E=$OUT/entities code
mkdir -p "$E"
yaml_in "$E" <<YAML
version: 4
defaults:
  all:
    style: Nothing reads this.
  image:
    tool: nothing
assets:
  hero:
    tool: $here/project/draw.py/draw
    instructions: The sheet.
    description: A brave hero, unlike <hero_old>.
  hero_old:
    tool: $here/project/draw.py/draw
    instructions: From <hero>.
    description: The hero of old.
YAML
said() { J --arg s "$1" '[.warnings[].message] | any(contains($s))' <<< "$(status "$E")" || check "entities: no warning of '$1': $(status "$E" | jq -c .warnings)"; }
said "defaults.all: the only keys are image, video and audio"
said "defaults.image.tool: defaults fill a node's inputs"
said "hero's description says <hero_old>: a description is words"
J '.assets[] | select(.name == "hero") | (.fields.description | contains("brave")) and ((.reads // []) | length == 0)' <<< "$(status "$E")" || check "entities: hero: $(row "$E" hero .)"
render "$E" hero_old || check "entities: render hero_old: $(tail -c 400 "$OUT/render.txt")"
J '.assets[] | select(.name == "hero") | .kind == "image"' <<< "$(status "$E")" || check "entities: a made picture's kind comes from its version: $(row "$E" hero .)"
[ "$(read_of "$E" hero_old)" = '["hero"]' ] || check "entities: hero_old reads $(read_of "$E" hero_old)"
local drew; drew=$(asked "$E" hero_old)
J '.input.instructions.references | length == 1 and (.[0] | .key == "hero" and .type == "image" and (.file | startswith("sha256:")) and .description == "A brave hero, unlike <hero_old>.")' <<< "$drew" || check "entities: a read is its picture and its words: $drew"

step "a project: a name that can't be one is refused, with the fix; a dot is read as \`_\`, with a warning (tools.md, Names)"
local N=$OUT/names
mkdir -p "$N"
printf 'version: 4\nassets:\n  kyogre sheet:\n    instructions: A.\n' | yaml_in "$N"
code=0; cozy project -p "$N" status > "$OUT/name.txt" 2>&1 || code=$?
{ [ "$code" != 0 ] && grep -qF "\`kyogre sheet\`: names can't hold spaces or commas; use \`kyogre_sheet\`" "$OUT/name.txt"; } || check "names: a space in a name: exit $code: $(tail -c 300 "$OUT/name.txt")"
printf 'version: 4\nassets:\n  a,b:\n    instructions: A.\n' | yaml_in "$N"
code=0; cozy project -p "$N" status > "$OUT/name.txt" 2>&1 || code=$?
{ [ "$code" != 0 ] && grep -qF "use \`a_b\`" "$OUT/name.txt"; } || check "names: a comma in a name: exit $code: $(tail -c 300 "$OUT/name.txt")"
printf 'version: 4\nassets:\n  a:\n    description: Prose <a b> here.\n  Amy.original: {instructions: From <a>.}\n' | yaml_in "$N"
J '([.assets[].name] | index("Amy_original") != null and index("Amy.original") == null) and ([.warnings[].message] | any(contains("Amy.original")))' <<< "$(status "$N")" || check "names: a dot in a name written by hand: $(cozy --json project -p "$N" status 2>&1 | jq -c '{names: [.assets[]?.name], warnings: [.warnings[]?.message], error}')"
J '[(.warnings // [])[].message] | any(contains("<a b>")) | not' <<< "$(status "$N")" || check "names: <a b> is just text, and was said: $(status "$N" | jq -c .warnings)"
code=0; cozy project -p "$N" rename a "b c" > "$OUT/name.txt" 2>&1 || code=$?
{ [ "$code" != 0 ] && grep -q "names can't hold" "$OUT/name.txt"; } || check "names: rename took a space: exit $code: $(tail -c 300 "$OUT/name.txt")"

step "a project: a package's own tags are text, and so is what can't be a name"
local D=$OUT/dialogue look
mkdir -p "$D"; plate "$D/look.mp4" testsrc2 440 800
printf 'version: 4\nassets: {}\n' | yaml_in "$D"
look=$(prefix "$D" "$D/look.mp4")
yaml_in "$D" <<YAML
version: 4
assets:
  look:
    approved: $look
  kyogre:
    approved: $look
  talk:
    tool: $here/project/shots.py/shoot
    summary: <look> says <D><English>Hello<d> and <d><ENGLISH>Hi</d> to <Picture 1> and <a b> too.
YAML
render "$D" talk || check "text: render talk: $(tail -c 300 "$OUT/render.txt")"
J '[.assets[] | select(.name == "talk")][0] | (.failure == null) and (.missing == null)' <<< "$(status "$D")" || check "text: a tag failed its node: $(row "$D" talk .)"
! grep -qiE '<(d|english)> is no longer|did you mean <(d|english)>' "$OUT/render.txt" || check "text: render warned of a tag: $(tail -c 400 "$OUT/render.txt")"
J '[.warnings[]?.message | select(contains("<d>") or contains("<D>") or contains("</d>") or contains("nglish") or contains("NGLISH") or contains("<a b>") or contains("Picture"))] == []' <<< "$(status "$D")" || check "text: a tag was warned of: $(status "$D" | jq -c .warnings)"
[ "$(read_of "$D" talk)" = '["look"]' ] || check "text: talk reads $(read_of "$D" talk)"
grep -qF '<D><English>Hello<d> and <d><ENGLISH>Hi</d>' "$(file_at "$D" "$(take_file "$D" talk given 0)")" || check "text: the tags did not reach the function unchanged"
printf '  d:\n    approved: %s\n' "$look" | added "$D"
J '[.warnings[].message] | any(contains("d is also a tag of"))' <<< "$(status "$D")" || check "text: a name that is its package's tag was not said: $(status "$D" | jq -c .warnings)"
[ "$(row "$D" talk .status)" = '"current"' ] || check "text: a thing named as a tag was read: $(row "$D" talk .status)"
printf '  ENGLISH:\n    approved: %s\n' "$look" | added "$D"
hand "$D" 's/<Picture 1>/<Englsh> and <Picture 1>/'
J '[.warnings[].message] | any(contains("english is also a tag of")) and any(contains("talk: <Englsh> is no longer in the project") and (contains("did you mean") | not))' <<< "$(status "$D")" || check "text: a tag in another case, or a typo of one, was said wrong: $(status "$D" | jq -c .warnings)"

step "a project: a name that names nothing fails its node, sends nothing, and the rest land"
local G=$OUT/gone pic before draw=$here/project/draw.py code
mkdir -p "$G"; still "$G/pic.png" navy; printf 'version: 4\nassets: {}\n' | yaml_in "$G"
pic=$(prefix "$G" "$G/pic.png")
yaml_in "$G" <<YAML
version: 4
assets:
  kyogre: {approved: "$pic"}
  fine:
    instructions: Stands on its own.
    tool: $draw/draw
  gone:
    instructions: Remove the background from <Image-1>.
    tool: $draw/draw
  typo:
    instructions: From <kyogee>.
    tool: $draw/draw
  reader:
    instructions: From <gone>.
    tool: $draw/draw
  accepted:
    approved: "$pic"
    instructions: Remove the background from <Image-1>.
    tool: $draw/draw
YAML
code=0; render "$G" all || code=$?
[ "$code" != 0 ] || check "gone: render all with nodes that can't run exited 0: $(tail -c 300 "$OUT/render.txt")"
cp "$OUT/render.txt" "$OUT/gone-render.txt"
says gone "gone: <Image-1> is no longer in the project"
says gone "typo: <kyogee> is no longer in the project: did you mean <kyogre>?"
says gone "bring it back, or edit gone's text"
! grep -F 'accepted' "$OUT/render.txt" | grep -q 'no longer' || check "gone: a pinned node failed in render all: $(grep -F accepted "$OUT/render.txt")"
status "$G" > "$OUT/gone-status.json"
J '.assets[] | select(.name == "gone") | .failure.code == "input_missing" and (.failure.message | contains("<Image-1> is no longer in the project")) and (.failure.remedy | contains("bring it back")) and .missing == ["Image-1"]' "$OUT/gone-status.json" || check "gone: its failure: $(jq -c '.assets[] | select(.name == "gone")' "$OUT/gone-status.json")"
J '.assets[] | select(.name == "typo") | (.failure.message | contains("did you mean <kyogre>?")) and .missing == ["kyogee"]' "$OUT/gone-status.json" || check "gone: a typo's failure: $(jq -c '.assets[] | select(.name == "typo")' "$OUT/gone-status.json")"
J '.assets[] | select(.name == "fine") | .failure == null and .versions == 1 and .standing == "current"' "$OUT/gone-status.json" || check "gone: the rest did not land: $(jq -c '.assets[] | select(.name == "fine")' "$OUT/gone-status.json")"
J '.assets[] | select(.name == "reader") | .versions == 0 and .standing == "missing" and (.missing == null) and (.failure.message | contains("gone failed in this render"))' "$OUT/gone-status.json" || check "gone: what reads a failed node should wait: $(jq -c '.assets[] | select(.name == "reader")' "$OUT/gone-status.json")"
J '.assets[] | select(.name == "accepted") | .pinned and .standing == "current" and .failure == null and .missing == ["Image-1"]' "$OUT/gone-status.json" || check "gone: a pin: $(jq -c '.assets[] | select(.name == "accepted")' "$OUT/gone-status.json")"
for name in gone typo reader; do [ "$(made "$G" "$name")" = 0 ] || [ "$(made "$G" "$name")" = null ] || check "gone: $name has a version"; done
[ "$(owed "$G")" = 0 ] || check "gone: something is pending"
J '[.warnings[]?.message] | any(contains("accepted")) | not' "$OUT/gone-status.json" || check "gone: a pin was warned of: $(jq -c .warnings "$OUT/gone-status.json")"
[ "$(jq '[.warnings[]?.message | select(contains("<Image-1>"))] | length' "$OUT/gone-status.json")" = 0 ] || check "gone: a kept failure is also a warning: $(jq -c .warnings "$OUT/gone-status.json")"
cozy project -p "$G" status > "$OUT/gone-status.txt" 2>&1 || true
grep -qF 'gone: <Image-1> is no longer in the project' "$OUT/gone-status.txt" || check "gone: cozy project status does not say it: $(tail -c 400 "$OUT/gone-status.txt")"
before=$(cozy --json run ls --limit 0 | jq '.runs | length')
code=0; render "$G" gone || code=$?
{ [ "$code" != 0 ] && [ "$(cozy --json run ls --limit 0 | jq '.runs | length')" = "$before" ]; } || check "gone: naming it sent a run (exit $code): $(tail -c 300 "$OUT/render.txt")"
code=0; render "$G" accepted || code=$?
{ [ "$code" != 0 ] && [ "$(cozy --json run ls --limit 0 | jq '.runs | length')" = "$before" ]; } || check "gone: running a pinned node again sent a run (exit $code): $(tail -c 300 "$OUT/render.txt")"
J '.assets[] | select(.name == "accepted") | .pinned and (.failure.code == "input_missing") and (.files.image.sha256 | length > 0)' <<< "$(status "$G")" || check "gone: a pin run again: $(row "$G" accepted .)"
# A package read for the first time here: its tags aren't known until it is, and still nothing is sent.
local G1=$OUT/gone-first; mkdir -p "$G1"
yaml_in "$G1" <<YAML
version: 4
assets:
  gone:
    tool: $draw/draw
    instructions: From <Image-1>.
YAML
J '.assets[] | select(.name == "gone") | .missing == null and .failure == null' <<< "$(status "$G1")" || check "gone: named missing before its package's tags are known: $(row "$G1" gone .)"
code=0; render "$G1" gone || code=$?
{ [ "$code" != 0 ] && [ "$(cozy --json run ls --limit 0 | jq '.runs | length')" = "$before" ] && [ "$(row "$G1" gone .failure.code)" = '"input_missing"' ]; } || check "gone: on its package's first read (exit $code): $(tail -c 300 "$OUT/render.txt")"
# Brought back, it runs and its failure goes. An edit does the same.
printf '  Image-1: {approved: "%s"}\n' "$pic" | added "$G"
J '.assets[] | select(.name == "gone") | .failure == null and .missing == null' <<< "$(status "$G")" || check "gone: brought back, it still says so: $(row "$G" gone .)"
render "$G" gone reader || check "gone: brought back: $(tail -c 300 "$OUT/render.txt")"
{ [ "$(made "$G" gone)" = 1 ] && [ "$(made "$G" reader)" = 1 ]; } || check "gone: brought back, gone made $(made "$G" gone), reader $(made "$G" reader)"
hand "$G" 's/From <kyogee>\./From <kyogre>./'
render "$G" typo || check "gone: edited: $(tail -c 300 "$OUT/render.txt")"
J '.assets[] | select(.name == "typo") | .failure == null and .versions == 1' <<< "$(status "$G")" || check "gone: edited: $(row "$G" typo .)"

step "a project: a pin out of date holds its file, says what changed, and only the Run that asks re-runs it"
local O=$OUT/outdated; mkdir -p "$O"; still "$O/drop.png" teal; printf 'version: 4\nassets: {}\n' | yaml_in "$O"
pic=$(prefix "$O" "$O/drop.png")
yaml_in "$O" <<YAML
version: 4
assets:
  dropped: {approved: $pic}
  image_a:
    tool: $draw/draw
    instructions: The first.
  image_b: {tool: $draw/draw, instructions: The second.}
  image_c:
    tool: $draw/draw
    instructions: From <image_a> and <image_b>, and <dropped>.
  image_d:
    tool: $draw/draw
    instructions: From <image_c>.
YAML
render "$O" all || check "outdated: render all: $(tail -c 300 "$OUT/render.txt")"
cozy project -p "$O" approve image_c 1 > "$OUT/outdated-approve.txt" 2>&1 || check "outdated: approve image_c: $(cat "$OUT/outdated-approve.txt")"
cozy project -p "$O" approve image_d 1 > "$OUT/outdated-approve.txt" 2>&1 || check "outdated: approve image_d: $(cat "$OUT/outdated-approve.txt")"
J '.assets[] | select(.name == "image_c") | .pinned and .standing == "current" and .changed == null' <<< "$(status "$O")" || check "outdated: a fresh pin: $(row "$O" image_c .)"
render "$O" image_a || check "outdated: a new image_a: $(tail -c 300 "$OUT/render.txt")"
status "$O" > "$OUT/outdated-pinned.json"
J '.assets[] | select(.name == "image_c") | .pinned and .standing == "out_of_date" and (.changed | length > 0) and .versions == 1 and (.status | contains("its inputs changed"))' "$OUT/outdated-pinned.json" || check "outdated: image_c should be pinned and out of date: $(jq -c '.assets[] | select(.name == "image_c")' "$OUT/outdated-pinned.json")"
J '.assets[] | select(.name == "image_d") | .pinned and .standing == "current"' "$OUT/outdated-pinned.json" || check "outdated: image_d reads the file image_c holds: $(jq -c '.assets[] | select(.name == "image_d")' "$OUT/outdated-pinned.json")"
render "$O" all || check "outdated: the plain run: $(tail -c 300 "$OUT/render.txt")"
{ [ "$(made "$O" image_c)" = 1 ] && [ "$(made "$O" image_d)" = 1 ] && [ "$(made "$O" image_a)" = 3 ]; } || check "outdated: the plain run: image_a $(made "$O" image_a), image_c $(made "$O" image_c), image_d $(made "$O" image_d)"
J '[.assets[] | select(.name == "image_c" or .name == "image_d") | .pinned] == [true, true]' <<< "$(status "$O")" || check "outdated: the plain run moved a pin"
render "$O" all --outdated || check "outdated: --outdated: $(tail -c 400 "$OUT/render.txt")"
cp "$OUT/render.txt" "$OUT/outdated-render.txt"; cozy project -p "$O" export > "$OUT/outdated-after.yaml"; status "$O" > "$OUT/outdated-after.json"
{ [ "$(made "$O" image_c)" = 2 ] && [ "$(made "$O" image_d)" = 2 ]; } || check "outdated: image_c $(made "$O" image_c), image_d $(made "$O" image_d): a pin downstream of a new version counts: $(tail -c 400 "$OUT/render.txt")"
says outdated "image_c: pinned, and"
J '[.assets[] | select(.name == "image_c" or .name == "image_d")] | all(.pinned != true and .standing == "current" and .changed == null)' "$OUT/outdated-after.json" || check "outdated: pins left: $(jq -c '[.assets[] | {name, pinned, standing, changed}]' "$OUT/outdated-after.json")"
J '.assets[] | select(.name == "dropped") | .pinned and .versions == 0 and .standing == "current"' "$OUT/outdated-after.json" || check "outdated: a dropped file: $(jq -c '.assets[] | select(.name == "dropped")' "$OUT/outdated-after.json")"
! grep -E '^    approved: [^$]' <<< "$(sed -n '/^  image_c:/,/^  image_d:/p' <(cozy project -p "$O" export))" || check "outdated: project.yaml keeps a pin: $(grep -nE '^    approved: ' <(cozy project -p "$O" export))"
J --arg drew "$(asked "$O" image_d 1)" '.versions[1].files.image.sha256 as $c | ($drew | contains($c)) and $c != .versions[0].files.image.sha256' <<< "$(held "$O" image_c)" || check "outdated: the new image_d did not read the new image_c: $(held "$O" image_d | jq -c '[.versions[].reads]')"
cozy project -p "$O" approve image_c 1 > "$OUT/outdated-again.txt" 2>&1 || check "outdated: the version it held can't be approved again: $(cat "$OUT/outdated-again.txt")"
J '.assets[] | select(.name == "image_c") | .pinned and .approved == 1 and .standing == "out_of_date" and .versions == 2' <<< "$(status "$O")" || check "outdated: approved again: $(row "$O" image_c .)"
# Edited, and an input gone as well: it can't be made again, so it stays, and says why.
dropped "$O" image_b
code=0; render "$O" all --outdated || code=$?
cp "$OUT/render.txt" "$OUT/outdated-kept-render.txt"
[ "$code" = 0 ] || check "outdated: a pin that can't be re-run failed the render: $(tail -c 300 "$OUT/render.txt")"
says outdated "image_c stays pinned, though <image_a> changed: <image_b> is no longer in the project"
J '.assets[] | select(.name == "image_c") | .pinned and .approved == 1 and .versions == 2 and .failure == null and .standing == "out_of_date" and .missing == ["image_b"] and (.changed | index("<image_b>") | not)' <<< "$(status "$O")" || check "outdated: kept: $(row "$O" image_c .)"

step "a project: its inputs deleted, an accepted node stands: not out of date, not re-run, and what reads it reads its file"
local S2=$OUT/sheet one two held runs_before2; mkdir -p "$S2"; still "$S2/one.png" red; still "$S2/two.png" navy; printf 'version: 4\nassets: {}\n' | yaml_in "$S2"
one=$(prefix "$S2" "$S2/one.png"); two=$(prefix "$S2" "$S2/two.png")
yaml_in "$S2" <<YAML
version: 4
assets:
  image_one: {approved: $one}
  image_two: {approved: $two}
  sheet:
    tool: $draw/draw
    instructions: A character sheet from <image_one> and <image_two>.
  poster:
    tool: $draw/draw
    instructions: From <sheet>.
YAML
render "$S2" all || check "sheet: render all: $(tail -c 300 "$OUT/render.txt")"
cozy project -p "$S2" approve sheet 1 > "$OUT/sheet-approve.txt" 2>&1 || check "sheet: approve: $(cat "$OUT/sheet-approve.txt")"
cozy project -p "$S2" approve poster 1 > "$OUT/sheet-approve.txt" 2>&1 || check "sheet: approve poster: $(cat "$OUT/sheet-approve.txt")"
held=$(take_file "$S2" sheet image 0)
dropped "$S2" image_one image_two
status "$S2" > "$OUT/sheet-status.json"
J --arg h "$held" '.assets[] | select(.name == "sheet") | .pinned and .standing == "current" and .changed == null and .failure == null and .missing == ["image_one", "image_two"] and .status == "approved: version 1" and .files.image.sha256 == $h' "$OUT/sheet-status.json" || check "sheet: its inputs deleted: $(jq -c '.assets[] | select(.name == "sheet")' "$OUT/sheet-status.json")"
J '.assets[] | select(.name == "poster") | .pinned and .standing == "current" and .changed == null and .missing == null' "$OUT/sheet-status.json" || check "sheet: what reads it: $(jq -c '.assets[] | select(.name == "poster")' "$OUT/sheet-status.json")"
J '[.warnings[]?.message | select(contains("image_one"))] == []' "$OUT/sheet-status.json" || check "sheet: warned: $(jq -c .warnings "$OUT/sheet-status.json")"
runs_before2=$(cozy --json run ls --limit 0 | jq '.runs | length')
render "$S2" all --outdated || check "sheet: --outdated: $(tail -c 300 "$OUT/render.txt")"
{ [ "$(cozy --json run ls --limit 0 | jq '.runs | length')" = "$runs_before2" ] && [ "$(made "$S2" sheet)" = 1 ] && [ "$(made "$S2" poster)" = 1 ]; } || check "sheet: --outdated sent a run: sheet $(made "$S2" sheet), poster $(made "$S2" poster): $(tail -c 300 "$OUT/render.txt")"
J '.assets[] | select(.name == "sheet") | .pinned and .approved == 1 and .failure == null' <<< "$(status "$S2")" || check "sheet: --outdated moved it: $(row "$S2" sheet .)"
cozy project -p "$S2" unapprove poster > "$OUT/sheet-unpin.txt" 2>&1 || check "sheet: unpin poster: $(cat "$OUT/sheet-unpin.txt")"
render "$S2" poster || check "sheet: what reads it, again: $(tail -c 300 "$OUT/render.txt")"
grep -qF "$held" <<< "$(asked "$S2" poster)" || check "sheet: what reads it did not read its pinned file: $(asked "$S2" poster)"
runs_before2=$(cozy --json run ls --limit 0 | jq '.runs | length')
code=0; render "$S2" sheet || code=$?
{ [ "$code" != 0 ] && [ "$(cozy --json run ls --limit 0 | jq '.runs | length')" = "$runs_before2" ]; } || check "sheet: naming it (exit $code): $(tail -c 300 "$OUT/render.txt")"
says sheet "sheet: <image_one> is no longer in the project; <image_two> is no longer in the project"
J --arg h "$held" '.assets[] | select(.name == "sheet") | .pinned and .files.image.sha256 == $h and .failure.code == "input_missing"' <<< "$(status "$S2")" || check "sheet: named, it lost its file: $(row "$S2" sheet .)"

step "a project: a cycle of mentions is said, and what is in it still renders"
local Y=$OUT/cycle
mkdir -p "$Y"
yaml_in "$Y" <<YAML
version: 4
assets:
  one:
    tool: $here/project/draw.py/draw
    instructions: From <two>.
  two:
    tool: $here/project/draw.py/draw
    instructions: From <one>.
YAML
J '[.warnings[].message | select(contains("which is made from it: that mention stays plain text"))] | length == 1' <<< "$(status "$Y")" || check "cycle: not said once: $(status "$Y" | jq -c .warnings)"
render "$Y" all || check "cycle: render all: $(tail -c 300 "$OUT/render.txt")"
{ [ "$(made "$Y" one)" = 1 ] && [ "$(made "$Y" two)" = 1 ]; } || check "cycle: made $(counts "$Y")"

}

# Orphans (cl-761): what the file no longer names keeps its versions and files until `reclaim --yes`.
orphans() {
step "a project: a box renamed by a hand's import keeps its versions"
local T=$OUT/orphaned code shared gone_file
mkdir -p "$T"
yaml_in "$T" <<YAML
version: 4
assets:
  old:
    tool: $here/project/draw.py/draw
    instructions: The old one.
  reader:
    tool: $here/project/draw.py/draw
    instructions: <old> on a shelf.
YAML
render "$T" all || check "orphans: render all: $(tail -c 300 "$OUT/render.txt")"
hand "$T" 's/^  old:$/  new:/; s/<old>/<new>/'
{ J '.orphans == null or (.orphans | length == 0)' <<< "$(status "$T")" && [ "$(made "$T" new)" = 1 ] && [ "$(row "$T" new .status)" = '"current"' ]; } || check "orphans: renamed, new is $(row "$T" new .)"
[ "$(row "$T" reader .standing)" = '"current"' ] || check "orphans: a rename put its reader out of date: $(row "$T" reader .status)"

step "a project: reclaim lists, and --yes deletes what nothing else names"
render "$T" all --missing || check "orphans: render all renamed"
shared=$(take_file "$T" new image 0)
# Another asset's version names the same file: it stays. The rest of the orphan goes.
added "$T" <<YAML
  doomed:
    tool: $here/project/draw.py/draw
    instructions: Going.
  kept:
    tool: $here/project/draw.py/draw
    instructions: Stays.
YAML
render "$T" all --missing || check "orphans: render all with doomed"
gone_file=$(take_file "$T" doomed image 0)
db_sql "$T" "UPDATE versions SET record = json_set(record, '\$.files.frame', '$gone_file') WHERE key = 'kept'"
dropped "$T" doomed
added "$T" <<YAML
  second:
    tool: $here/project/draw.py/draw
    instructions: Second.
YAML
render "$T" second || check "orphans: render second"
local second_file; second_file=$(take_file "$T" second image 0)
dropped "$T" second
cozy --json project -p "$T" reclaim > "$OUT/reclaim.json" 2>&1 || check "orphans: reclaim: $(tail -c 300 "$OUT/reclaim.json")"
J '[.orphans[].name] == ["doomed", "second"] and (.deleted | not)' "$OUT/reclaim.json" || check "orphans: reclaim lists $(jq -c . "$OUT/reclaim.json")"
{ J '.orphans | any(.name == "second" and .versions == 1)' "$OUT/reclaim.json" && [ -f "$T/files/${second_file#sha256:}.png" ]; } || check "orphans: a listing deleted something"
cozy --json project -p "$T" reclaim --yes > "$OUT/reclaim.json" 2>&1 || check "orphans: reclaim --yes: $(tail -c 300 "$OUT/reclaim.json")"
J '.deleted and .files >= 1 and .bytes > 0' "$OUT/reclaim.json" || check "orphans: reclaim --yes says $(jq -c . "$OUT/reclaim.json")"
{ J '(.orphans // []) == []' <<< "$(status "$T")" && [ "$(sql1 "$T" "SELECT count(*) FROM versions WHERE key LIKE 'doomed%' OR key LIKE 'second%'")" = 0 ]; } || check "orphans: their versions are still in project.db"
[ ! -e "$T/files/${second_file#sha256:}.png" ] || check "orphans: a file nothing names was not deleted"
[ -f "$T/files/${gone_file#sha256:}.png" ] || check "orphans: a file another asset's version names was deleted"
[ -f "$T/files/${shared#sha256:}.png" ] || check "orphans: a kept asset's file was deleted"

step "a project: reclaim refuses while the project renders"
added "$T" <<YAML
  waiting:
    tool: $here/project/shots.py/slow
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
yaml_in "$T" <<YAML
version: 4
assets:
  waiting:
    tool: $here/project/shots.py/slow
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
# The walk (cl-770): every job known when asked, each going on its own, a deleted asset's queued job saying so, and nothing of the walk left behind.
walking() {
step "a project: what's queued is known at once, a long job holds up nothing it isn't read by, a deleted asset's job stops and lands nothing"
local W=$OUT/walking stream=$OUT/walking-events.txt id run back
mkdir -p "$W"
yaml_in "$W" <<YAML
version: 4
assets:
  running_gone:
    tool: $here/project/shots.py/slow
    seconds: 900
  reads_gone:
    tool: $here/project/draw.py/draw
    instructions: From <running_gone>, which is deleted as it runs.
  slow:
    tool: $here/project/shots.py/slow
    seconds: 5
  beside:
    tool: $here/project/draw.py/draw
    instructions: Reads nothing, so it goes now.
  one:
    tool: $here/project/draw.py/draw
    instructions: From <slow>.
  two:
    tool: $here/project/draw.py/draw
    instructions: From <slow>.
  last:
    tool: $here/project/draw.py/draw
    instructions: From <one> and <two>.
  gone_soon:
    tool: $here/project/draw.py/draw
    instructions: Also from <slow>, and deleted while it waits.
YAML
id=$(cozy --json project -p "$W" status | jq -r .id)
api -N --max-time 120 "http://$ADDR/v1/events" > "$stream" 2> /dev/null &
local tail=$!
cozy project -p "$W" render all --queue --on local > "$OUT/walking-queue.txt" 2>&1 || check "walking: render: $(tail -c 300 "$OUT/walking-queue.txt")"
J '[.board.jobs[].asset] as $a | (["running_gone", "reads_gone", "slow", "beside", "one", "two", "last", "gone_soon"] - $a) == []' <<< "$(status "$W")" || check "walking: not every job is on the board at once: $(status "$W" | jq -c '[.board.jobs[] | .asset]')"
J '.board.jobs[] | select(.asset == "last") | .needs | contains(["one", "two"])' <<< "$(status "$W")" || check "walking: last doesn't say what it waits for: $(status "$W" | jq -c '.board.jobs[] | select(.asset == "last")')"
for _ in $(seq 240); do run=$(status "$W" | jq -r '.board.jobs[] | select(.asset == "running_gone") | .run // empty'); [ -n "$run" ] && [ "$(cozy --json run show "$run" --once | jq -r .status)" = in_progress ] && break; sleep 0.25; done
[ "$(cozy --json run show "$run" --once | jq -r .status)" = in_progress ] || check "walking: running_gone isn't running: $(status "$W" | jq -c '.board.jobs[] | select(.asset == "running_gone")')"
# On one machine they queue there: each went as soon as it could, not after the long job.
for _ in $(seq 120); do J '[.board.jobs[] | select(.asset == "slow" or .asset == "beside") | .run] | all' <<< "$(status "$W")" && break; sleep 0.25; done
J '[.board.jobs[] | select(.asset == "slow" or .asset == "beside") | .run] | all' <<< "$(status "$W")" || check "walking: what reads nothing waited for the long job: $(status "$W" | jq -c '[.board.jobs[] | {asset, run}]')"
[ "$(owed "$W")" -ge 1 ] || check "walking: what the walk owes isn't in project.db"
dropped "$W" gone_soon
for _ in $(seq 20); do J '.board.jobs[] | select(.asset == "gone_soon") | .state == "canceled"' <<< "$(status "$W")" && break; sleep 0.25; done
back=$(api -X DELETE "http://$ADDR/v1/projects/$id/assets/running_gone")
for _ in $(seq 60); do [ "$(cozy --json run show "$run" --once | jq -r .status)" = canceled ] && break; sleep 0.5; done
[ "$(cozy --json run show "$run" --once | jq -r .status)" = canceled ] || check "walking: a deleted asset's run goes on billing: $(cozy --json run show "$run" --once | jq -c '{status, progress}')"
for _ in $(seq 240); do J '.busy | not' <<< "$(status "$W")" && break; sleep 0.5; done
kill "$tail" 2> /dev/null
{ [ "$(made "$W" slow)" = 1 ] && [ "$(made "$W" beside)" = 1 ] && [ "$(made "$W" one)" = 1 ] && [ "$(made "$W" two)" = 1 ] && [ "$(made "$W" last)" = 1 ]; } || check "walking: made $(status "$W" | jq -c '[.assets[] | {(.name): .versions}] | add')"
for gone in gone_soon running_gone; do
  [ "$(made "$W" "$gone")" = 0 ] || check "walking: deleted $gone has a version"
  J --arg gone "$gone" '.board.jobs[] | select(.asset == $gone) | .state == "canceled" and (.why | test("deleted"))' <<< "$(status "$W")" || check "walking: deleted $gone's job says $(status "$W" | jq -c --arg gone "$gone" '.board.jobs[] | select(.asset == $gone)')"
done
J '.board.jobs[] | select(.asset == "reads_gone") | .state == "failed" and (.error.message | test("running_gone was deleted"))' <<< "$(status "$W")" || check "walking: what read a deleted asset says $(status "$W" | jq -c '.board.jobs[] | select(.asset == "reads_gone")')"
# Put back, it is the asset, and not the render it stopped.
api -X PUT -d "$(jq -c '{text, at}' <<< "$back")" "http://$ADDR/v1/projects/$id/assets/running_gone" > /dev/null || check "walking: PUT didn't take back what DELETE answered"
{ grep -q '^  running_gone:' <(cozy project -p "$W" export) && J '.busy | not' <<< "$(status "$W")" && [ "$(made "$W" running_gone)" = 0 ]; } || check "walking: put back, running_gone went again: $(status "$W" | jq -c '{busy, jobs: [.board.jobs[] | select(.asset == "running_gone")]}')"
[ "$(owed "$W")" = 0 ] || check "walking: what the walk owed outlived the render"
[ -z "$(find "$W/.runs" -type f 2> /dev/null)" ] || check "walking: left in .runs: $(find "$W/.runs" -type f)"
grep -q "\"type\":\"job.updated\".*\"project\":\"$id\"\|\"project\":\"$id\".*\"type\":\"job.updated\"" "$stream" || check "walking: /v1/events told no job.updated: $(head -c 300 "$stream")"
}

walk_restarted() {
step "a project: Calcifer killed mid-walk, and the walk goes on with no run sent twice"
local R=$OUT/walk-restarted lock=$HOME/.cozy/calcifer/calcifer.lock.holder pid run
mkdir -p "$R"
yaml_in "$R" <<YAML
version: 4
assets:
  first:
    tool: $here/project/shots.py/slow
    seconds: 12
  second:
    tool: $here/project/shots.py/slow
    seconds: 3
  reader:
    tool: $here/project/draw.py/draw
    instructions: From <first> and <second>.
YAML
cozy project -p "$R" render all --queue --on local > "$OUT/walk-restarted.txt" 2>&1 || check "restarted: render: $(tail -c 300 "$OUT/walk-restarted.txt")"
for _ in $(seq 240); do run=$(status "$R" | jq -r '.board.jobs[] | select(.asset == "first") | .run // empty'); [ -n "$run" ] && [ "$(cozy --json run show "$run" --once | jq -r .status)" = in_progress ] && break; sleep 0.25; done
[ "$(cozy --json run show "$run" --once | jq -r .status)" = in_progress ] || check "restarted: first isn't running: $(status "$R" | jq -c .board.jobs)"
# A copy of the folder is a project of its own, with its own id, and goes on with none of what its render owes.
local copy=$OUT/walk-restarted-copy
cp -a "$R" "$copy"
J '(.busy | not) and ((.board.jobs // []) | length == 0) and ((.queue // []) | length == 0)' <<< "$(status "$copy")" || check "restarted: a copy goes on with its render: $(status "$copy" | jq -c '{busy, queue}')"
[ "$(owed "$copy")" = 0 ] || check "restarted: a copy kept what the render owes"
pid=$(jq -r .pid "$lock")
kill -9 "$pid"; while kill -0 "$pid" 2> /dev/null; do sleep 0.1; done
cozy project ls > /dev/null || check "restarted: cozy doesn't answer after Calcifer was killed"
ADDR=$(jq -r .address "$lock") TOKEN=$(jq -r .token "$lock")
[ "$(jq -r .pid "$lock")" != "$pid" ] || check "restarted: Calcifer is still pid $pid"
for _ in $(seq 240); do J '.busy | not' <<< "$(status "$R")" && break; sleep 0.5; done
{ [ "$(made "$R" first)" = 1 ] && [ "$(made "$R" second)" = 1 ] && [ "$(made "$R" reader)" = 1 ]; } || check "restarted: made $(status "$R" | jq -c '[.assets[] | {(.name): .versions}] | add')"
local sent; sent=$(cozy --json run ls --limit 0 | jq -c --arg r "$(status "$R" | jq -r .id)" '[.runs[] | select(.project_id == $r) | .target] | group_by(.) | map({(.[0]): length}) | add')
J '[.[]] | add == 3' <<< "$sent" || check "restarted: runs sent: $sent"
[ "$(status "$copy" | jq -r .id)" != "$(status "$R" | jq -r .id)" ] || check "restarted: a copy has its original's id"
[ "$(cozy --json run ls --limit 0 | jq -c --arg r "$(status "$copy" | jq -r .id)" '[.runs[] | select(.project_id == $r)] | length')" = 0 ] || check "restarted: a copy sent a run"
[ "$(owed "$R")" = 0 ] || check "restarted: what the walk owed outlived the render"
}

no_double_spend() {
step "a project: a folder copied mid-render runs nothing twice and loses nothing landed"
local S=$OUT/spend copy=$OUT/spend-copy run
mkdir -p "$S"
yaml_in "$S" <<YAML
version: 4
assets:
  first:
    tool: $here/project/draw.py/draw
    instructions: The one before.
  slow:
    tool: $here/project/shots.py/slow
    seconds: 15
  reader:
    tool: $here/project/draw.py/draw
    instructions: From <slow>.
YAML
render "$S" first || check "spend: first: $(tail -c 300 "$OUT/render.txt")"
cozy project -p "$S" render slow reader --queue --on local > "$OUT/spend-queue.txt" 2>&1 || check "spend: render: $(tail -c 300 "$OUT/spend-queue.txt")"
for _ in $(seq 240); do run=$(status "$S" | jq -r '.board.jobs[] | select(.asset == "slow") | .run // empty'); [ -n "$run" ] && [ "$(cozy --json run show "$run" --once | jq -r .status)" = in_progress ] && break; sleep 0.25; done
[ -n "$run" ] || check "spend: slow never ran"
# A copy is a project of its own, and goes on with none of what this one owes.
cp -a "$S" "$copy"
J '(.busy | not) and ((.board.jobs // []) | length == 0)' <<< "$(status "$copy")" || check "spend: a copy goes on with its render: $(status "$copy" | jq -c '{busy, board}')"
for _ in $(seq 240); do J '.busy | not' <<< "$(status "$S")" && break; sleep 0.5; done
{ [ "$(made "$S" first)" = 1 ] && [ "$(made "$S" slow)" = 1 ] && [ "$(made "$S" reader)" = 1 ]; } || check "spend: made $(status "$S" | jq -c '[.assets[] | {(.name): .versions}] | add')"
local ran; ran=$(cozy --json run ls --limit 0 | jq -c --arg s "$(status "$S" | jq -r .id)" '[.runs[] | select(.project_id == $s) | .target] | group_by(.) | map({(.[0]): length}) | add')
J '[.[]] | add == 3' <<< "$ran" || check "spend: runs: $ran"
[ "$(cozy --json run ls --limit 0 | jq --arg c "$(status "$copy" | jq -r .id)" '[.runs[] | select(.project_id == $c)] | length')" = 0 ] || check "spend: the copy sent a run"
[ "$(owed "$copy")" = 0 ] || check "spend: the copy kept what its original owes"
}

render_all_n() {
step "a project: -n is for an asset you name, and all makes one of each, and says so"
local A=$OUT/all-n
mkdir -p "$A"
yaml_in "$A" <<YAML
version: 4
assets:
  first:
    tool: $here/project/draw.py/draw
    instructions: One.
  second:
    tool: $here/project/draw.py/draw
    instructions: Two.
YAML
cozy project -p "$A" render all -n 3 --on local > "$OUT/all-n.txt" 2>&1 || check "all -n: $(tail -c 300 "$OUT/all-n.txt")"
{ [ "$(made "$A" first)" = 1 ] && [ "$(made "$A" second)" = 1 ]; } || check "all -n: made $(status "$A" | jq -c '[.assets[] | {(.name): .versions}] | add')"
grep -q 'makes one of each' "$OUT/all-n.txt" || check "all -n: doesn't say so: $(cat "$OUT/all-n.txt")"
cozy project -p "$A" render first -n 2 --on local > "$OUT/one-n.txt" 2>&1 || check "name -n: $(tail -c 300 "$OUT/one-n.txt")"
[ "$(made "$A" first)" = 3 ] || check "name -n: made $(made "$A" first) of first"
}

queueing() {
step "a project: three renders queued from one shell, one canceled, the rest land"
local T=$OUT/queueing
mkdir -p "$T"
yaml_in "$T" <<YAML
version: 4
assets:
  first:
    tool: $here/project/shots.py/slow
    seconds: 8
  second:
    tool: $here/project/draw.py/draw
    instructions: From <first>.
  third:
    tool: $here/project/draw.py/draw
    instructions: Also from <first>.
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
{ [ "$(made "$T" first)" = 1 ] && [ "$(made "$T" second)" = 1 ] && [ "$(made "$T" third)" = 0 ]; } || check "queue: made $(counts "$T")"
step "a project: a hand's edit that can't be read is refused, changing nothing, and the render goes on"
local B=$OUT/queue-broken code
mkdir -p "$B"
yaml_in "$B" <<YAML
version: 4
assets:
  slow:
    tool: $here/project/shots.py/slow
    seconds: 6
  after:
    tool: $here/project/draw.py/draw
    instructions: From <slow>.
  also:
    tool: $here/project/draw.py/draw
    instructions: Also from <slow>.
YAML
cozy project -p "$B" render after also --queue --on local > "$OUT/queue-broken.txt" 2>&1 || check "broken: render after: $(tail -c 300 "$OUT/queue-broken.txt")"
cozy project -p "$B" export > "$OUT/queue-broken.yaml"
{ cat "$OUT/queue-broken.yaml"; printf '  : [ this does not read\n'; } > "$B.broken.yaml"
code=0; cozy project -p "$B" import "$B.broken.yaml" > "$OUT/queue-broken-refused.txt" 2>&1 || code=$?
[ "$code" != 0 ] || check "broken: an edit that doesn't read was taken"
cmp -s "$OUT/queue-broken.yaml" <(cozy project -p "$B" export) || check "broken: a refused edit changed the project: $(diff "$OUT/queue-broken.yaml" <(cozy project -p "$B" export) | head -5)"
J '.busy and ([.queue[].name] | index("after") != null)' <<< "$(status "$B")" || check "broken: the render stopped: $(status "$B" | jq -c '{busy, queue}')"
cozy project -p "$B" cancel also > "$OUT/queue-broken-cancel.txt" 2>&1 || check "broken: cancel: $(tail -c 300 "$OUT/queue-broken-cancel.txt")"
grep -q 'dropped also' "$OUT/queue-broken-cancel.txt" || check "broken: cancel said $(cat "$OUT/queue-broken-cancel.txt")"
for _ in $(seq 240); do J '.busy | not' <<< "$(status "$B")" && break; sleep 0.5; done
{ [ "$(made "$B" slow)" = 1 ] && [ "$(made "$B" after)" = 1 ] && [ "$(made "$B" also)" = 0 ]; } || check "broken: made $(counts "$B")"

step "a project: a render uses the file as it was asked, so an edit waits for the next"
local W=$OUT/queue-edit going prompt
mkdir -p "$W"
yaml_in "$W" <<YAML
version: 4
assets:
  gate:
    tool: $here/project/shots.py/slow
    seconds: 6
  words:
    tool: $here/project/draw.py/draw
    instructions: Old words.
  reader:
    tool: $here/project/draw.py/draw
    instructions: From <words> after <gate>.
YAML
cozy project -p "$W" render all --on local > "$OUT/queue-edit.txt" 2>&1 &
going=$!
for _ in $(seq 120); do [ "$(status "$W" | jq '[.queue[]? | select(.name == "gate" and .run != null)] | length')" = 1 ] && break; sleep 0.25; done
hand "$W" 's/instructions: Old words\./instructions: New words./'
wait "$going" || check "edit: the render: $(tail -c 400 "$OUT/queue-edit.txt")"
! grep -q 'cannot render' "$OUT/queue-edit.txt" || check "edit: a pass said it couldn't: $(grep 'cannot render' "$OUT/queue-edit.txt")"
{ [ "$(made "$W" words)" = 1 ] && [ "$(made "$W" reader)" = 1 ]; } || check "edit: the pass made $(counts "$W")"
[ "$(prompt "$W" words)" = 'Old words.' ] || check "edit: the pass used $(prompt "$W" words)"
case "$(row "$W" words .status)" in *'out of date'*) ;; *) check "edit: words is $(row "$W" words .status)" ;; esac
render "$W" reader || check "edit: the next render: $(tail -c 400 "$OUT/render.txt")"
{ [ "$(made "$W" words)" = 2 ] && [ "$(made "$W" reader)" = 2 ] && [ "$(prompt "$W" words)" = 'New words.' ]; } || check "edit: the next render made $(counts "$W")"
grep -qF "$(take_file "$W" words image -1)" <<< "$(asked "$W" reader)" || check "edit: the reader did not read the new file"

step "a project: two queued renders with an edit between use their own text"
local Q=$OUT/queue-two
mkdir -p "$Q"
yaml_in "$Q" <<YAML
version: 4
assets:
  gate:
    tool: $here/project/shots.py/slow
    seconds: 4
  note:
    tool: $here/project/draw.py/draw
    instructions: Text one, after <gate>.
YAML
cozy project -p "$Q" render note --queue --on local > "$OUT/queue-two-1.txt" 2>&1 || check "two: first: $(tail -c 300 "$OUT/queue-two-1.txt")"
hand "$Q" 's/Text one/Text two/'
cozy project -p "$Q" render note --queue --on local > "$OUT/queue-two-2.txt" 2>&1 || check "two: second: $(tail -c 300 "$OUT/queue-two-2.txt")"
for _ in $(seq 240); do J '.busy | not' <<< "$(status "$Q")" && break; sleep 0.5; done
J --arg a "$(asked "$Q" note 0 | jq -r .input.instructions.text)" --arg b "$(asked "$Q" note 1 | jq -r .input.instructions.text)" -n '[$a, $b] | any(startswith("Text one")) and any(startswith("Text two"))' || check "two: note was made from: $(asked "$Q" note 0 | jq -r .input.instructions.text) / $(asked "$Q" note 1 | jq -r .input.instructions.text)"
[ "$(sql1 "$Q" "SELECT count(*) FROM asked")" = 0 ] || check "two: a finished render left its text in project.db: $(sql "$Q" 'SELECT digest FROM asked')"

[ -n "${ALONE:-}" ] && return 0
step "a project: a canceled render all sends nothing again"
local A=$OUT/queue-all
mkdir -p "$A"
yaml_in "$A" <<YAML
version: 4
assets:
  long:
    tool: $here/project/shots.py/slow
    seconds: 120
  after:
    tool: $here/project/draw.py/draw
    instructions: From <long>.
YAML
cozy project -p "$A" render all --queue --on local > "$OUT/queue-all.txt" 2>&1 || check "queue: render all: $(tail -c 300 "$OUT/queue-all.txt")"
for _ in $(seq 120); do [ "$(status "$A" | jq '[.queue[] | select(.run != null)] | length')" = 1 ] && break; sleep 0.5; done
cozy project -p "$A" cancel all > "$OUT/queue-all-cancel.txt" 2>&1 || check "queue: cancel all: $(tail -c 300 "$OUT/queue-all-cancel.txt")"
{ grep -q 'canceled run' "$OUT/queue-all-cancel.txt" && grep -q 'dropped after' "$OUT/queue-all-cancel.txt"; } || check "queue: cancel all said $(cat "$OUT/queue-all-cancel.txt")"
for _ in $(seq 120); do J '(.busy | not) and (.queue // [] | length == 0)' <<< "$(status "$A")" && break; sleep 0.5; done
J '(.busy | not) and (.queue // [] | length == 0)' <<< "$(status "$A")" || check "queue: after cancel all: $(status "$A" | jq -c '{busy, queue}')"
{ [ "$(made "$A" long)" = 0 ] && [ "$(made "$A" after)" = 0 ]; } || check "queue: cancel all made $(counts "$A")"
}

# Every render makes a new version, but of what is pinned and not named. Only approve or unpin moves a pin.
every_render() {
step "a project: render all twice, a pinned asset named, and --missing"
local T=$OUT/every-render pin line
mkdir -p "$T"
yaml_in "$T" <<YAML
version: 4
assets:
  fixed:
    tool: $here/project/draw.py/draw
    instructions: Pinned.
  card:
    tool: $here/project/draw.py/draw
    instructions: A card.
  poster:
    tool: $here/project/draw.py/draw
    instructions: <card> as a poster.
YAML
render "$T" fixed || check "every render: fixed: $(tail -c 300 "$OUT/render.txt")"
cozy project -p "$T" approve fixed 1 > "$OUT/approve.txt" 2>&1 || check "every render: approve: $(tail -c 300 "$OUT/approve.txt")"
line=$(grep -n '^    approved: ' <(cozy project -p "$T" export))
render "$T" all || check "every render: render all: $(tail -c 300 "$OUT/render.txt")"
says every-render "fixed is approved: skipped. Name it to make a new version, or unapprove it"
render "$T" all || check "every render: render all again: $(tail -c 300 "$OUT/render.txt")"
{ [ "$(made "$T" card)" = 2 ] && [ "$(made "$T" poster)" = 2 ] && [ "$(made "$T" fixed)" = 1 ]; } || check "every render: two passes made $(counts "$T")"
grep -qF "$(take_file "$T" card image -1)" <<< "$(asked "$T" poster)" || check "every render: the poster did not read the newest card"
J '[.versions[].seed] | unique | length == 2' <<< "$(held "$T" card)" || check "every render: a pass did not draw a new seed"
render "$T" fixed || check "every render: naming the pinned: $(tail -c 300 "$OUT/render.txt")"
{ [ "$(made "$T" fixed)" = 2 ] && [ "$(grep -n '^    approved: ' <(cozy project -p "$T" export))" = "$line" ] && [ "$(row "$T" fixed .approved)" = 1 ]; } || check "every render: naming the pinned moved its pin: $(grep -n 'image:' <(cozy project -p "$T" export)), $(row "$T" fixed .approved)"
render "$T" all --missing || check "every render: --missing: $(tail -c 300 "$OUT/render.txt")"
{ [ "$(made "$T" card)" = 2 ] && [ "$(made "$T" poster)" = 2 ] && [ "$(made "$T" fixed)" = 2 ]; } || check "every render: --missing made $(counts "$T")"
}

# One frame of a video is an asset of its own, made here, free: moved, only it is made again. A shot makes no pictures: `writes:`, on a
# beat or an asset, is said and ignored and stays out of its key, and a mention of what it named names nothing, which fails only that asset.
# A name with nothing under it, as a still used to be declared, has nothing to make: no style alone goes to a model.
# A writer leaves 2 GiB in the page cache and flushes it once at its end, as a download did, while the page edits, reads and asks
# frames that land through the walk. ext4 commits every file's data with its journal, so a write that waited on the disk under
# Calcifer's one lock took seconds, and every request queued behind it (43 s for a PATCH on the laptop).
no_stall() {
step "a project: no request waits on another program's writes"
local T=$OUT/no-stall lock=$HOME/.cozy/calcifer/calcifer.lock.holder id look writer i slow
ADDR=$(jq -r .address "$lock") TOKEN=$(jq -r .token "$lock")
mkdir -p "$T"; plate "$T/look.mp4" testsrc2 440 800 6
printf 'version: 4\nassets: {}\n' | yaml_in "$T"
look=$(prefix "$T" "$T/look.mp4")
{ printf 'version: 4\nassets:\n  look: {approved: %s}\n  note: {description: A note.}\n' "$look"
  for i in 1 2 3 4 5; do printf '  f%s: {tool: cozy/frame, video: <look>, at: %s}\n' "$i" "$i"; done; } | yaml_in "$T"
render "$T" all || check "no stall: render all: $(tail -c 300 "$OUT/render.txt")"
id=$(api -X POST -d "$(jq -n --arg p "$T" '{project: $p}')" "http://$ADDR/v1/projects" | jq -r .id)
dd if=/dev/zero of="$T/hog" bs=8M count=256 conv=fdatasync status=none & writer=$!
: > "$T/took.txt"
for i in $(seq 1 40); do
  api -o /dev/null -w '%{time_total} PATCH note\n' -X PATCH -d "{\"fields\": {\"description\": \"Note $i.\"}}" "http://$ADDR/v1/projects/$id/assets/note" >> "$T/took.txt"
  api -o /dev/null -w '%{time_total} GET project\n' "http://$ADDR/v1/projects/$id" >> "$T/took.txt"
  api -o /dev/null -w '%{time_total} PATCH frame\n' -X PATCH -d "{\"fields\": {\"at\": $((i % 5)).$((i % 10))}}" "http://$ADDR/v1/projects/$id/assets/f$((i % 5 + 1))" >> "$T/took.txt"
  api -o /dev/null -w '%{time_total} render ask\n' -X POST -H "Idempotency-Key: stall-$i-$RANDOM" -d "{\"names\": [\"f$((i % 5 + 1))\"], \"on\": [\"local\"]}" "http://$ADDR/v1/projects/$id/runs" >> "$T/took.txt"
done
wait "$writer"; rm -f "$T/hog"
slow=$(awk '$1 > 3' "$T/took.txt")
[ -z "$slow" ] || check "no stall: requests waited on another program's writes: $(sort -rn "$T/took.txt" | head -5 | tr '\n' ';')"
}

frames() {
step "a project: a frame of a shot, moved, re-renders nothing"
local T=$OUT/frames look made was before
mkdir -p "$T"; plate "$T/look.mp4" testsrc2 440 800 8
printf 'version: 4\nassets: {}\n' | yaml_in "$T"
look=$(prefix "$T" "$T/look.mp4")
yaml_in "$T" <<YAML
version: 4
assets:
  long_take:
    tool: $here/project/shots.py/shoot
    summary: Walking on <look>.
    beats:
      - duration: 4
        description: She runs.
      - duration: 4
        description: She stands.
  harbor:
    tool: cozy/frame
    video: <long_take>
    at: 1
  look:
    approved: $look
YAML
render "$T" all || check "frames: render all: $(tail -c 400 "$OUT/render.txt")"
J '.versions[-1] | (.runs | length == 0) and (.files.image != null)' <<< "$(held "$T" harbor)" || check "frames: harbor is not a frame: $(held "$T" harbor | jq -c '.versions[-1]')"
made=$(runs "$T"); was=$(row "$T" long_take .inputs)
local frame_key; frame_key=$(row "$T" harbor .inputs)
hand "$T" 's/^    at: 1$/    at: 2/'
[ "$(row "$T" long_take .inputs)" = "$was" ] || check "frames: moving a frame moved its shot's key"
render "$T" harbor || check "frames: render it: $(tail -c 300 "$OUT/render.txt")"
says frames "harbor: version"
[ "$(made "$T" harbor)" = 1 ] || check "frames: a tool kept its old version: $(made "$T" harbor)"
[ "$(row "$T" harbor .inputs)" != "$frame_key" ] || check "frames: a frame's key does not hold its time"
[ "$(runs "$T")" = "$made" ] || check "frames: moving a frame ran something"

step "a project: a mention of nothing fails only what says it"
added "$T" <<YAML
  next:
    tool: $here/project/shots.py/shoot
    summary: Dawn on <look>, by <coast.1>.
  other:
    tool: $here/project/shots.py/shoot
    summary: Noon on <look>.
  bare_still:
  waits:
    tool: $here/project/shots.py/shoot
    summary: Dusk on <look>, by <bare_still>.
YAML
before=$(runs "$T")
render "$T" all --missing && check "frames: render all exited 0 with a mention of nothing"
says frames "next: <coast.1> is no longer in the project"
says frames "waits cannot render: bare_still has nothing to make"
J '[.warnings[].message] | any(contains("bare_still says nothing"))' <<< "$(status "$T")" || check "frames: a name alone was not said: $(status "$T" | jq -c .warnings)"
J '.assets[] | select(.name == "next") | .failure.code == "input_missing" and .missing == ["coast.1"]' <<< "$(status "$T")" || check "frames: next: $(row "$T" next .)"
[ "$(made "$T" next)" = 0 ] || [ "$(made "$T" next)" = null ] || check "frames: next has a version"
[ "$(made "$T" other)" = 1 ] || check "frames: other did not land: $(tail -c 300 "$OUT/render.txt")"
[ "$(runs "$T")" = "$((before + 1))" ] || check "frames: a mention of nothing ran something: $(runs "$T") runs, was $before"
}

# What a function reads of an asset, and what it says of a field that isn't one of its inputs.
direct_inputs() {
step "a project: a function reads its own inputs, and says of what isn't one"
local T=$OUT/inputs
mkdir -p "$T"
yaml_in "$T" <<YAML
version: 4
assets:
  checked:
    tool: $here/project/shots.py/typed
    summary: Some words.
    no_such_field: Nothing reads this.
  told:
    tool: $here/project/shots.py/echo
    b: its own
    c: 3
YAML
render "$T" checked || check "inputs: typed: $(tail -c 400 "$OUT/render.txt")"
says inputs "reads no input no_such_field"
render "$T" told || check "inputs: inputs: $(tail -c 400 "$OUT/render.txt")"
heard() { cat "$(file_at "$T" "$(take_file "$T" "$1" given)")"; }
[ "$(heard told | jq -c .)" = '{"b":"its own","c":3}' ] || check "inputs: told's inputs: $(heard told)"
}

# A tool is a function: what a box names, what makes it, what its box shows, and what crosses a mention. Weightless, by project/tool.py.
# What the canvas does, with no page: each verb is its route, with its fields, and a refusal names what's next. web/live.js holds these against an open canvas.
by_the_cli() {
step "a project by the CLI alone: add, set, one asset's status, delete and restore"
local T=$OUT/by-cli tool=$here/project/tool.py line lock=$HOME/.cozy/calcifer/calcifer.lock.holder
mkdir -p "$T"; still "$T/red.png" red
printf 'version: 4\nassets: {}\n' | yaml_in "$T"
ADDR=$(jq -r .address "$lock") TOKEN=$(jq -r .token "$lock")
refuses() { # refuses <what> <what it says> <command…>
  local what=$1 says=$2 code=0; shift 2
  cozy project -p "$T" "$@" > "$OUT/by-cli.txt" 2>&1 || code=$?
  [ "$code" != 0 ] && grep -qF -- "$says" "$OUT/by-cli.txt" || check "by the CLI: $what: exit $code: $(tail -c 300 "$OUT/by-cli.txt")"
}
# A list typed with `=` goes by what the tool declares: `beats` is a list, and text in a text field stays text whatever it looks like.
cozy project -p "$T" add "$tool/told" typed instructions=plain > /dev/null || check "by the CLI: add typed"
cozy project -p "$T" set typed 'beats=[{"description":"One","duration":3}]' 'instructions=[1, 2]' > /dev/null || check "by the CLI: set typed"
J '(.asset.fields.beats | type) == "array" and .asset.fields.instructions == "[1, 2]"' <<< "$(held "$T" typed)" || check "by the CLI: fields aren't as the tool declares: $(held "$T" typed | jq -c .asset.fields)"
cozy project -p "$T" delete typed > /dev/null || check "by the CLI: delete typed"
rows() { api "http://$ADDR/v1/projects/$(status "$T" | jq -r .id)/canvas"; }
[ "$(cozy project -p "$T" import "$T/red.png" red_art)" = red_art ] || check "by the CLI: import did not print its name"
# Read with this machine's Runtime, as a run reads it: a box doesn't wait on PyPI's.
(cd "$T" && cozy --json run -p "$T" "$tool/still" instructions=primed --on local > /dev/null 2>&1) || check "by the CLI: a run to read the tool"

[ "$(cozy project -p "$T" add "$tool/still" drawn 'instructions=From the line' count=3 style=flat soft:=true at=400,120,360)" = drawn ] || check "by the CLI: add: no name"
J '.assets[] | select(.name == "drawn") | .tool != null and .fields.instructions == "From the line" and .fields.count == 3 and .fields.style == "flat" and .fields.soft == true' <<< "$(status "$T")" || check "by the CLI: add: $(row "$T" drawn .fields)"
J '.cards.drawn == {"x": 400, "y": 120, "w": 360}' <<< "$(rows)" || check "by the CLI: add at=: $(rows | head -c 300)"
refuses "a name taken" "cozy project set drawn" add "$tool/still" drawn
[ "$(cozy project -p "$T" add "$tool/still")" = still_1 ] || check "by the CLI: add with no name takes what its tool calls it"
[ "$(cozy project -p "$T" add "$tool/still")" = still_2 ] || check "by the CLI: a name cozy picks counts up"
[ "$(cozy project -p "$T" add "$tool/slow")" = tool_1 ] || check "by the CLI: with no name of its tool's, its package's"
cozy project -p "$T" delete still_1 still_2 tool_1 > /dev/null || check "by the CLI: delete of three"
[ "$(cozy project -p "$T" add text mood 'text=Soft light.')" = mood ] || check "by the CLI: add text"
printf 'text: |\n  Soft light,\n  a long way down.\n' > "$T/mood.yaml"
cozy project -p "$T" set mood --input "$T/mood.yaml" > /dev/null || check "by the CLI: a text asset from a file"
grep -q 'a long way down' <<< "$(row "$T" mood .fields.text)" || check "by the CLI: a text asset's words: $(row "$T" mood .)"
cozy project -p "$T" delete mood > /dev/null || check "by the CLI: delete mood"
refuses "two names" "one name, not" add "$tool/still" one two
refuses "a tool that makes nothing" "cozy package ls" add "$tool/words" nothing
refuses "a place that isn't one" "at is x,y or x,y,w" add "$tool/still" nowhere at=here

cozy project -p "$T" set drawn instructions='Plain words' count=4 'seed:=7' > /dev/null || check "by the CLI: set"
J '.assets[] | select(.name == "drawn") | .fields.instructions == "Plain words" and .fields.count == 4 and .fields.seed == 7' <<< "$(status "$T")" || check "by the CLI: set: $(row "$T" drawn .fields)"
cozy project -p "$T" set drawn style= > /dev/null || check "by the CLI: set style="
J '.assets[] | select(.name == "drawn") | .fields | has("style") | not' <<< "$(status "$T")" || check "by the CLI: style= did not clear it: $(row "$T" drawn .fields)"
printf 'aspect_ratio: "16:9"\ncount: 2\n' > "$T/fields.yaml"
cozy project -p "$T" set drawn --input "$T/fields.yaml" count=1 > /dev/null || check "by the CLI: set --input"
J '.assets[] | select(.name == "drawn") | .fields.aspect_ratio == "16:9" and .fields.count == 1' <<< "$(status "$T")" || check "by the CLI: set --input, a term over it: $(row "$T" drawn .fields)"
printf 'count: 2\n' | cozy project -p "$T" set drawn --input - > /dev/null || check "by the CLI: set --input -"
[ "$(row "$T" drawn .fields.count)" = 2 ] || check "by the CLI: set --input -: $(row "$T" drawn .fields)"
cozy project -p "$T" set drawn "tool=$tool/told" > /dev/null || check "by the CLI: set tool="
[ "$(row "$T" drawn .tool)" = "\"$tool/told\"" ] || check "by the CLI: tool= did not change what makes it: $(row "$T" drawn .tool)"
cozy project -p "$T" set drawn "tool=$tool/still" > /dev/null || check "by the CLI: set tool= back"
cozy project -p "$T" set drawn at=-50,75 > /dev/null || check "by the CLI: set at="
J '.cards.drawn == {"x": -50, "y": 75}' <<< "$(rows)" || check "by the CLI: set at=: $(rows | head -c 300)"
cozy project -p "$T" set DRAWN at=1,2,320 > /dev/null || check "by the CLI: set at=, in any case"
J '.cards.drawn == {"x": 1, "y": 2, "w": 320} and (.cards | has("DRAWN") | not)' <<< "$(rows)" || check "by the CLI: a row under another spelling: $(rows | head -c 300)"
refuses "nothing to set" "nothing to set" set drawn
refuses "not a term" "isn't field=value" set drawn instructions
refuses "nothing named that" "cozy project status" set no_such words=here
refuses "no such asset's place" "cozy project status" set no_such at=1,2

render "$T" drawn || check "by the CLI: render drawn: $(tail -c 300 "$OUT/render.txt")"
J '.asset.name == "drawn" and (.versions | length == 1)' <<< "$(cozy --json project -p "$T" status drawn)" || check "by the CLI: status of one: $(cozy --json project -p "$T" status drawn | head -c 300)"
cozy project -p "$T" status drawn | grep -q 'Plain words' || check "by the CLI: status of one does not say its words"
refuses "status of nothing" "cozy project status" status no_such
line=$(cozy project -p "$T" delete drawn) || check "by the CLI: delete"
line=${line##*$'\n'}
J '.assets | all(.name != "drawn")' <<< "$(status "$T")" || check "by the CLI: delete left it"
case $line in "cozy project -p $T restore drawn "*) ;; *) check "by the CLI: delete did not print what restores it: $line" ;; esac
refuses "not what delete printed" "cozy project status" restore drawn nonsense
refuses "nothing named that" "cozy project status" delete no_such
eval "$line" > /dev/null || check "by the CLI: what delete printed: $line"
J '.assets[] | select(.name == "drawn") | .versions == 1 and .fields.instructions == "Plain words"' <<< "$(status "$T")" || check "by the CLI: restore lost it: $(row "$T" drawn .)"
J '.cards.drawn == {"x": 1, "y": 2, "w": 320}' <<< "$(rows)" || check "by the CLI: restore lost its place"
eval "$line" > "$OUT/by-cli.txt" 2>&1 && check "by the CLI: restored twice"
grep -qF "cozy project rename drawn" "$OUT/by-cli.txt" || check "by the CLI: a name taken names no next step: $(tail -c 300 "$OUT/by-cli.txt")"
J '.next[0] | startswith("cozy project") and contains(" restore drawn ")' <<< "$(cozy --json project -p "$T" delete drawn)" || check "by the CLI: delete --json names no next"
}

tool_functions() {
step "a tool is a function: called directly, its prompt the files it mentions, and past a bound clamped with a line saying so"
local T=$OUT/tools tool=$here/project/tool.py red blue id one input read before lock=$HOME/.cozy/calcifer/calcifer.lock.holder key
mkdir -p "$T"; still "$T/red.png" red; still "$T/blue.png" blue
printf 'version: 4\nassets: {}\n' | yaml_in "$T"
red=$(prefix "$T" "$T/red.png"); blue=$(prefix "$T" "$T/blue.png")
yaml_in "$T" <<YAML
version: 4
assets:
  red_art: {approved: $red, description: PUBLIC-RED-WORDS}
  blue_art: {approved: $blue}
  direct:
    tool: $tool/still
    instructions: Look at <red_art> and <blue_art>.
    aspect_ratio: "16:9"
    count: 99
    style: nonsense
  madeup:
    tool: $tool/told
    instructions: SECRET-PROMPT-WORDS
    shade: 0.5
    description: PUBLIC-MADEUP-WORDS
  reader:
    tool: $tool/told
    instructions: Uses <madeup>.
    shade: 7
    aspect_ratio: "9:16"
    beats:
      - description: One
        duration: 5
  alone:
    tool: $tool
    instructions: Alone <madeup>.
YAML
render "$T" direct || check "tools: direct: $(tail -c 500 "$OUT/render.txt")"
input=$(asked "$T" direct)
J '(.target | endswith("/still")) and (.input | .aspect_ratio == "16:9" and .count == 4 and (has("style") | not) and .instructions.text == "Look at <red_art> and <blue_art>.")' <<< "$input" || check "tools: the call: $input"
J '.input.instructions.references | map(.key) == ["red_art", "blue_art"] and (.[0] | .type == "image" and .description == "PUBLIC-RED-WORDS" and (.file | length == 71))' <<< "$input" || check "tools: <red_art> is not the picture it names: $input"
says tools "count: 99 is past what it takes, so it's 4"; says tools "isn't one of flat, noise"

step "a tool is a function: its bounds clamp, and only what's meant for a reader crosses a mention"
render "$T" madeup reader || check "tools: told: $(tail -c 500 "$OUT/render.txt")"
heard() { asked "$T" "$1" | jq -c .input; }
J '.shade == 0.5 and .instructions.text == "SECRET-PROMPT-WORDS" and (has("description") | not)' <<< "$(heard madeup)" || check "tools: what told heard: $(heard madeup)"
J '.shade == 1 and .aspect_ratio == "1:1" and (.beats | length == 1)' <<< "$(heard reader)" || check "tools: a bound didn't clamp: $(heard reader)"
says tools "shade: 7 is past what it takes"
read=$(heard reader)
grep -q PUBLIC-MADEUP-WORDS <<< "$read" || check "tools: a description didn't reach what mentions it: $read"
! grep -q SECRET-PROMPT-WORDS <<< "$read" || check "tools: a prompt crossed its mention: $read"
J '.instructions.references | length == 1 and (.[0] | keys == ["description", "file", "key", "type"])' <<< "$read" || check "tools: a read carries more than what's meant for a reader: $read"
cozy project -p "$T" approve madeup 1 > /dev/null 2>&1 || check "tools: approve madeup"
was=$(row "$T" reader .inputs)
hand "$T" 's/instructions: SECRET-PROMPT-WORDS/instructions: SECRET-WORDS-EDITED/'
[ "$(row "$T" reader .inputs)" = "$was" ] || check "tools: an approved read's instructions moved its reader's key"
hand "$T" 's/instructions: SECRET-WORDS-EDITED/instructions: SECRET-PROMPT-WORDS/'
cozy project -p "$T" unapprove madeup > /dev/null 2>&1 || check "tools: unapprove madeup"
render "$T" alone || check "tools: a package alone stopped the render: $(tail -c 400 "$OUT/render.txt")"
says tools "alone: a tool is a function, and"

step "a tool is a function: it's an input to the memo, and what makes no difference stays out"
before=$(row "$T" direct .standing)
[ "$before" = '"current"' ] || check "tools: direct should be current: $before"
hand "$T" "s|tool: $tool/still|tool: $tool/told|"
[ "$(row "$T" direct .standing)" = '"out_of_date"' ] || check "tools: changing a node's function didn't put its take out of date: $(status "$T" | jq -c '.stale')"
hand "$T" "s|tool: $tool/told|tool: $tool/still| if /^  direct:\$/ .. /^  madeup:\$/"
[ "$(row "$T" direct .standing)" = '"current"' ] || check "tools: the same function again isn't current"

step "a tool is a function: GET /v1/tools lists a project's local tools, what each makes, and its fields by type"
ADDR=$(jq -r .address "$lock") TOKEN=$(jq -r .token "$lock")
id=$(status "$T" | jq -r .id)
api --get --data-urlencode "project=$id" "http://$ADDR/v1/tools" > "$OUT/tools.json"
one() { jq -c --arg f "$1" '.tools[] | select(.local and .function == $f)' "$OUT/tools.json"; }
J '[.tools[] | select(.local) | .function] | sort == ["need", "slow", "still", "told"]' "$OUT/tools.json" || check "tools: listed, and only @app.tool's: $(jq -c '[.tools[] | select(.local) | .function]' "$OUT/tools.json")"
J --arg t "$tool" '.id == ($t + "/still") and .makes == "image" and .label == "A still" and .name == "still_1"' <<< "$(one still)" || check "tools: still: $(one still)"
J '.makes == "video" and (.fields[0] | .field == "video" and .type == "slot" and .accept == "video" and .required)' <<< "$(one need)" || check "tools: need: $(one need)"
field() { jq -c --arg f "$2" '.fields[] | select(.field == $f)' <<< "$(one "$1")"; }
J '.type == "prompt" and .takes == ["image"]' <<< "$(field still instructions)" || check "tools: a prompt of pictures: $(field still instructions)"
J '.type == "number" and .min == 1 and .max == 4 and .integer' <<< "$(field still count)" || check "tools: count: $(field still count)"
J '.type == "shape" and (.ratios | sort == ["16:9", "1:1"]) and .auto' <<< "$(field still aspect_ratio)" || check "tools: aspect_ratio: $(field still aspect_ratio)"
J '.type == "choice" and .options == ["flat", "noise"]' <<< "$(field still style)" || check "tools: style: $(field still style)"
J '.type == "toggle"' <<< "$(field still soft)" || check "tools: soft: $(field still soft)"
J '.type == "seed"' <<< "$(field still seed)" || check "tools: seed: $(field still seed)"
J '.type == "number" and .min == 0 and .max == 1 and .auto' <<< "$(field told shade)" || check "tools: a bound told to a box: $(field told shade)"
J '.type == "toggle" and .default == true' <<< "$(field told bright)" || check "tools: a toggle's default: $(field told bright)"
J '.type == "group" and .list and .min_items == 1 and .max_items == 4 and (.members[] | select(.field == "duration") | .max == 15 and .total and .unit == "s")' <<< "$(field told beats)" || check "tools: a list group: $(field told beats)"
J '.type == "slot" and .accept == "video"' <<< "$(field told video)" || check "tools: a file by its type: $(field told video)"
J '.label == "Told"' <<< "$(one told)" || check "tools: a tool's label: $(one told)"
J '.package_title == "Tool"' <<< "$(one told)" || check "tools: its package in words, heading it in the list: $(one told)"

step "a tool is a function: a box from a tool takes its kind from what it makes, and what's missing fails it before anything runs"
one=$(api -X POST -H "Idempotency-Key: tool-$RANDOM$RANDOM" -d "$(jq -n --arg t "$tool/need" '{tool: $t, name: "kept_video"}')" "http://$ADDR/v1/projects/$id/assets")
J '.name == "kept_video"' <<< "$one" || check "tools: POST assets: $one"
J '.assets[] | select(.name == "kept_video") | .kind == "video"' <<< "$(status "$T")" || check "tools: its kind isn't from its output: $(row "$T" kept_video .kind)"
key=tool-$RANDOM$RANDOM
for n in 1 2; do
  one=$(api -X POST -H "Idempotency-Key: $key" -d "$(jq -n --arg t "$tool/need" '{tool: $t, name: "by_key"}')" "http://$ADDR/v1/projects/$id/assets")
  J '.name == "by_key"' <<< "$one" || check "tools: POST assets under a key: $one"
  [ "$n" = 2 ] || before=$(seq_of "$T")
done
[ "$(grep -c '^  by_key' <(cozy project -p "$T" export))" = 1 ] || check "tools: a key's retry made another asset"
[ "$(seq_of "$T")" = "$before" ] || check "tools: a key's retry wrote again"
cozy project -p "$T" delete by_key > /dev/null || check "tools: delete by_key"
before=$(runs "$T")
render "$T" kept_video || true
says tools "needs its video"
[ "$(runs "$T")" = "$before" ] || check "tools: a run went with its file missing"

step "a tool is a function: Caption asks what the package says to, through the binder, and writes nothing"
before=$(seq_of "$T")
one=$(api -X POST -d '{"on": "local"}' "http://$ADDR/v1/projects/$id/assets/red_art/caption")
J '.name == "red_art" and (.text | contains("THE-PACKAGE-ASKS 1 pictures, 0 videos"))' <<< "$one" || check "tools: caption: $one"
J '.request.input.image | .type == "image" and .description == "PUBLIC-RED-WORDS"' <<< "$(cozy --json --full run show "$(jq -r .run <<< "$one")" --once)" || check "tools: caption's call: $(cozy --json --full run show "$(jq -r .run <<< "$one")" --once | jq -c .request)"
[ "$(seq_of "$T")" = "$before" ] || check "tools: caption wrote to the project"
J --arg e caption_no_file '.error.code == $e' <<< "$(api -X POST "http://$ADDR/v1/projects/$id/assets/kept_video/caption")" || check "tools: a caption of nothing isn't refused"
}

# The tools (design/tools.md, weightless proofs 1 to 11): a box's fields, by its tool's types, as its function gets them through the one
# binder that Render, `cozy project render` and `cozy run` share. The every-type tool is project/boxes.py's `shot`, the composed one
# project/probe's `composed`, which calls project/probe-callee, and the takes are project/shots.py's.

# tool_pictures <dir>: a project of three pictures (amy, with words, sky and the still coast makes), a 6 s video and a sound, and a text box.
tool_pictures() {
  local T=$1 amy sky walk hum
  mkdir -p "$T"; still "$T/amy.png" red; still "$T/sky.png" teal; plate "$T/walk.mp4" testsrc2 440 800
  ffmpeg -v error -y -f lavfi -i sine=frequency=220:duration=1 "$T/hum.wav"
  printf 'version: 4\nassets: {}\n' | yaml_in "$T"
  amy=$(prefix "$T" "$T/amy.png"); sky=$(prefix "$T" "$T/sky.png"); walk=$(prefix "$T" "$T/walk.mp4"); hum=$(prefix "$T" "$T/hum.wav")
  yaml_in "$T" <<YAML
version: 4
assets:
  amy: {approved: $amy, description: "Amy, tall."}
  sky: {approved: $sky}
  walk: {approved: $walk, description: A walk.}
  hum: {approved: $hum}
  mood: {text: Soft light.}
  coast: {tool: $here/project/tool.py/still, instructions: A coast., description: A volcanic coast.}
YAML
}

# keys <prompt>: its references' keys and types, in order.
keys() { jq -c '[.references[] | [.key, .type]]' <<< "$1"; }

binder_types() {
step "a tool: each type reaches its function as typed, and each prompt only what its text mentions (weightless 1)"
local T=$OUT/types input given
tool_pictures "$T"
added "$T" <<YAML
  shot:
    tool: $here/project/boxes.py/shot
    summary: <Amy> walks on <COAST> in <mood>, as in <walk:24-72>, and <amy> again; <hum> plays.
    pictures: <coast.1> and <walk>.
    negative: no <amy> here
    beats:
      - duration: 4
        description: <amy> runs.
        look: {lens: close, note: tight}
        angles:
          - framing: <coast> wide
            hold: 2
        still: <sky>
      - duration: 3
        description: Nothing named.
    aspect_ratio: "2:1"
    turbo: false
    bright: true
    megapixels: 9
    segments: 2
    quality: draft
    resolution: 1080p
    edit: head
    first_frame: <amy>
    plate: <walk>
    sound: <hum>
    clip: <walk:0-48>
    references: [<amy>, <coast>]
    steps: 12
    seed: 5
YAML
render "$T" coast shot || { check "types: render: $(tail -c 500 "$OUT/render.txt")"; return; }
input=$(asked "$T" shot | jq -c .input)
printf '%s\n' "$input" > "$OUT/types-input.json"
J '.summary.text == "<amy> walks on <coast> in Soft light., as in <walk:24-72>, and <amy> again; <hum> plays."' <<< "$input" || check "types: the summary's text, each mention as its box spells it and the text box its words: $(jq -c .summary.text <<< "$input")"
[ "$(keys "$(jq -c .summary <<< "$input")")" = '[["amy","image"],["coast","image"],["walk:24-72","video"],["hum","audio"]]' ] || check "types: the summary's references, once each, in first-mention order: $(keys "$(jq -c .summary <<< "$input")")"
J --arg amy "sha256:$(sha "$T/amy.png")" --arg walk "sha256:$(sha "$T/walk.mp4")" '.summary.references | (.[0] | .description == "Amy, tall." and .file == $amy) and (.[1].description == "A volcanic coast.") and all(.file | startswith("sha256:")) and (.[2].file != $walk)' <<< "$input" || check "types: what a reference holds, a range cut: $(jq -c .summary.references <<< "$input")"
J '.pictures.text == "<coast.1> and walk." and (.pictures.references | map(.key) == ["coast.1"])' <<< "$input" || check "types: a prompt of pictures, a version by its number, a video as its name: $(jq -c .pictures <<< "$input")"
J '.negative == "no amy here"' <<< "$input" || check "types: words take a box as its name: $(jq -c .negative <<< "$input")"
J '.beats | length == 2 and (.[0] | .duration == 4 and (.description.references | map(.key) == ["amy"]) and .look == {"lens": "close", "note": "tight"} and (.angles[0] | .hold == 2 and (.framing.references | map(.key) == ["coast"])) and (.still | startswith("sha256:"))) and (.[1].description == {"text": "Nothing named.", "references": []})' <<< "$input" \
  || check "types: groups two deep, each prompt only its own: $(jq -c .beats <<< "$input")"
J '.aspect_ratio == "16:9" and .megapixels == 4 and .turbo == false and .bright == true and .segments == 2 and .quality == "draft" and .resolution == "1080p" and .edit == "head" and .steps == 12 and .seed == 5' <<< "$input" \
  || check "types: toggles, numbers, choices, the shape and the seed: $(jq -c 'del(.summary, .pictures, .beats, .references, .clip)' <<< "$input")"
J --arg amy "sha256:$(sha "$T/amy.png")" --arg walk "sha256:$(sha "$T/walk.mp4")" --arg hum "sha256:$(sha "$T/hum.wav")" '.first_frame == $amy and .plate == $walk and .sound == $hum' <<< "$input" || check "types: a slot holds what's written in it: $(jq -c '{first_frame, plate, sound}' <<< "$input")"
J '.clip | .type == "video" and .description == "A walk." and (.file | startswith("sha256:"))' <<< "$input" || check "types: a described video slot: $(jq -c .clip <<< "$input")"
J '.references | length == 2 and all(.type == "image") and (map(.description) == ["Amy, tall.", "A volcanic coast."])' <<< "$input" || check "types: a list slot of pictures: $(jq -c .references <<< "$input")"
J 'has("pickup") | not' <<< "$input" || check "types: a pickup no gesture made: $(jq -c .pickup <<< "$input")"
says types "megapixels: 9 is past what it takes, so it's 4"
says types "isn't a shape it takes, so it's 16:9"
says types "negative takes words, so <amy> goes as its name"
says types "<walk>, a video, goes as its name"
given=$(cat "$(file_at "$T" "$(take_file "$T" shot given)")")
J '.first_frame == {"file": "ImageFile"} and .plate == {"file": "VideoFile"} and .sound == {"file": "AudioFile"} and .clip.file == {"file": "VideoFile"} and (.summary.references | keys_unsorted == ["amy", "coast", "walk:24-72", "hum"]) and .beats[0].angles[0].hold == 2 and .beats[0].look.lens == "close"' <<< "$given" \
  || check "types: what the function got, as typed: $given"
}

three_pictures() {
step "a tool: three pictures mentioned are three references, and none is a frame (weightless 3)"
local T=$OUT/three input
tool_pictures "$T"
added "$T" <<YAML
  three:
    tool: $here/project/boxes.py/shot
    summary: <amy>, <coast> and <sky> meet.
    plate: <walk>
  trio:
    tool: $here/project/tool.py/still
    instructions: <amy>, <coast> and <sky> together, not <walk>.
YAML
render "$T" coast three trio || { check "three: render: $(tail -c 500 "$OUT/render.txt")"; return; }
input=$(asked "$T" three | jq -c .input)
[ "$(keys "$(jq -c .summary <<< "$input")")" = '[["amy","image"],["coast","image"],["sky","image"]]' ] && J '(has("first_frame") | not) and (has("references") | not) and (.beats[0] | has("still") | not)' <<< "$input" \
  || check "three: a shot's three pictures: $input"
input=$(asked "$T" trio | jq -c .input)
J '(.instructions.references | map(.key) == ["amy", "coast", "sky"]) and (.instructions.text | endswith("not walk."))' <<< "$input" || check "three: a still's three pictures, and a video it doesn't take: $(jq -c .instructions <<< "$input")"
}

# One composed tool's runs, with its calls: shade from the other package, then tint of its own when that came out dark.
composed_tool() {
step "a tool that composes: it calls another package, branches on its answer, calls its own, and shows each step (weightless 2)"
local T=$OUT/composed page run
mkdir -p "$T"; still "$T/page.png" gray
printf 'version: 4\nassets: {}\n' | yaml_in "$T"
page=$(prefix "$T" "$T/page.png")
yaml_in "$T" <<YAML
version: 4
assets:
  page: {approved: $page}
  dark: {tool: $here/project/probe/composed, page: <page>, darkness: 0.75}
  light: {tool: $here/project/probe/composed, page: <page>, darkness: 0.25}
YAML
render "$T" dark light || { check "composed: render: $(tail -c 500 "$OUT/render.txt")"; return; }
run=$(held "$T" dark | jq -r '.versions[-1].runs[-1]')
ended "$run" > "$OUT/composed-dark.json"
J '[.calls[] | select(.status == "succeeded") | .function] as $f | ($f | index("shade")) != null and ($f | index("tint")) != null and ($f | index("enlarge")) == null' "$OUT/composed-dark.json" \
  || check "composed: a dark shade calls tint, and nothing enlarges unasked: $(jq -c '[.calls[] | {function, status}]' "$OUT/composed-dark.json")"
# Each publish is a revision of the box's picture, shown as it lands: Shaded, then Lightened, which the result is.
J '[.output[] | select(.name == "image")] | length == 1 and .[0].rev == 2 and .[0].label == "Lightened"' "$OUT/composed-dark.json" || check "composed: two previews and the result: $(jq -c '[.output[] | {name, rev, label}]' "$OUT/composed-dark.json")"
ended "$(held "$T" light | jq -r '.versions[-1].runs[-1]')" > "$OUT/composed-light.json"
J '([.calls[] | .function] | (index("shade") != null) and (index("tint") == null)) and ([.output[] | select(.name == "image")][0] | .rev == 1 and .label == "Shaded")' "$OUT/composed-light.json" || check "composed: a light shade still called tint: $(jq -c '{calls: [.calls[] | {function, status}], output: [.output[] | {name, rev, label}]}' "$OUT/composed-light.json")"
}

# A failed composed render asked again: the step that finished comes from the memo. Its failure is deliberate: never alone.
memo_retry() {
[ -z "${ALONE:-}" ] || return 0
step "a tool that composes: a failure, then Try again, and the step that finished is the memo's (weightless 5)"
local T=$OUT/retry page code=0 failed again
# A page of its own colour, so no memo of an earlier run answers.
mkdir -p "$T"; still "$T/page.png" "$(printf '0x%06x' $(((RANDOM << 9 ^ RANDOM) % 16777216)))"
printf 'version: 4\nassets: {}\n' | yaml_in "$T"
page=$(prefix "$T" "$T/page.png")
yaml_in "$T" <<YAML
version: 4
assets:
  page: {approved: $page}
  flaky: {tool: $here/project/probe/composed, page: <page>, darkness: 0.5, fails_until: $T/ready}
YAML
render "$T" flaky || code=$?
failed=$(cozy --json run ls -p "$T" --limit 0 --state failed | jq -r '[.runs[] | select(.target | endswith("/composed"))][0].number // empty')
{ [ "$code" != 0 ] && [ -n "$failed" ] && [ "$(made "$T" flaky)" = 0 ]; } || check "retry: the deliberate failure (exit $code, run ${failed:-none}): $(tail -c 400 "$OUT/render.txt")"
[ -z "$failed" ] || ended "$failed" > "$OUT/retry-failed.json" || true
J '.status == "failed" and ([.calls[] | select(.function == "shade") | [.status, .memoized]] == [["succeeded", false]])' "$OUT/retry-failed.json" || check "retry: the failed run's shade: $(jq -c '{status, calls: [.calls[]? | {function, status, memoized}]}' "$OUT/retry-failed.json")"
touch "$T/ready"
render "$T" flaky || check "retry: Try again: $(tail -c 400 "$OUT/render.txt")"
again=$(held "$T" flaky | jq -r '.versions[-1].runs[-1]')
ended "$again" > "$OUT/retry-again.json"
J --arg seed "$(jq -r .request.seed "$OUT/retry-failed.json")" '.status == "completed" and (.request.seed | tostring) == $seed and ([.calls[] | select(.function == "shade") | .memoized] == [true])' "$OUT/retry-again.json" \
  || check "retry: shade wasn't the memo's on the same seed: $(jq -c '{status, seed: .request.seed, calls: [.calls[]? | {function, status, memoized}]}' "$OUT/retry-again.json")"
}

# A composed render canceled while its call runs: the call ends with it, so it leaves no result, and the next render makes it again,
# though it's memoized. Its cancel is deliberate: never alone.
cancel_children() {
[ -z "${ALONE:-}" ] || return 0
step "a tool that composes: canceled mid-step, every call it made ends canceled (weightless 6)"
local T=$OUT/cancel page run=''
mkdir -p "$T"; still "$T/page.png" "$(printf '0x%06x' $(((RANDOM << 9 ^ RANDOM) % 16777216)))"
printf 'version: 4\nassets: {}\n' | yaml_in "$T"
page=$(prefix "$T" "$T/page.png")
yaml_in "$T" <<YAML
version: 4
assets:
  page: {approved: $page}
  long: {tool: $here/project/probe/composed, page: <page>, hold: 20}
YAML
cozy project -p "$T" render long --queue --on local > "$OUT/cancel.txt" 2>&1 || check "cancel: render: $(tail -c 300 "$OUT/cancel.txt")"
for _ in $(seq 240); do run=$(status "$T" | jq -r '.board.jobs[] | select(.asset == "long") | .run // empty'); [ -n "$run" ] && [ "$(cozy --json run show "$run" --once | jq -r .status)" = in_progress ] && break; sleep 0.5; done
# Into shade's 20 s.
sleep 6
cozy project -p "$T" cancel long > /dev/null 2>&1 || check "cancel: cancel long"
for _ in $(seq 120); do [ "$(cozy --json run show "$run" --once | jq -r .status)" = canceled ] && break; sleep 0.5; done
ended "$run" > "$OUT/cancel.json" || true
J '.status == "canceled" and ((.calls // []) | all(.function == "composed" or .status == "canceled"))' "$OUT/cancel.json" || check "cancel: the render and its calls: $(jq -c '{status, calls}' "$OUT/cancel.json")"
[ "$(made "$T" long)" = 0 ] || check "cancel: a canceled render made a version"
# A shade that went on would have ended by now, memoized.
sleep 20
render "$T" long || check "cancel: the render after: $(tail -c 300 "$OUT/render.txt")"
ended "$(held "$T" long | jq -r '.versions[-1].runs[-1]')" > "$OUT/cancel-after.json" || true
J '[.calls[] | select(.function == "shade") | .memoized] == [false]' "$OUT/cancel-after.json" || check "cancel: the canceled shade went on to its end: $(jq -c '[.calls[]? | {function, status, memoized}]' "$OUT/cancel-after.json")"
}

# The same box by `cozy run` in its project and by Render: one binder, so one call.
run_as_render() {
step "a tool: the same box through cozy run and through Render is the same call (weightless 7)"
local T=$OUT/same rendered ran called
tool_pictures "$T"
added "$T" <<YAML
  same:
    tool: $here/project/tool.py/told
    instructions: <Amy> by <coast>, in <mood>.
    shade: 0.5
    beats:
      - description: <coast> at dusk.
        duration: 4
    seed: 9
YAML
render "$T" coast same || { check "same: render: $(tail -c 500 "$OUT/render.txt")"; return; }
rendered=$(asked "$T" same | jq -S -c .input)
called=$(cozy --json --full project -p "$T" status same | jq -S -c '.call // .asset.call // empty')
(cd "$T" && cozy --json run -p "$T" "$here/project/tool.py/told" 'instructions=<Amy> by <coast>, in <mood>.' 'shade:=0.5' 'beats:=[{"description": "<coast> at dusk.", "duration": 4}]' 'seed:=9' --on local) > "$OUT/same.ndjson" 2> "$OUT/same.err" \
  || check "same: cozy run: $(tail -c 400 "$OUT/same.err")"
ran=$(cozy --json --full run show "$(tail -1 "$OUT/same.ndjson" | jq -r .run.number)" --once | jq -S -c .request.input)
[ "$rendered" = "$ran" ] || check "same: Render sent $rendered and cozy run $ran"
J '.instructions.text == "<amy> by <coast>, in Soft light.." and (.instructions.references | map(.key) == ["amy", "coast"]) and (.beats[0].description.references | map(.key) == ["coast"])' <<< "$rendered" || check "same: what it sent: $rendered"
[ -z "$called" ] || [ "$(jq -S -c '.input // .' <<< "$called")" = "$rendered" ] || check "same: status's call is $called, and Render sent $rendered"
}

# What a run makes comes down, but a result field typed `Latents`: that stays on its machine. A slot's extras are read only from a version
# its own package made.
latents_stay() {
step "a tool: a Latents result stays on its machine, any other file comes down, and only its own package reads its extras (weightless 9)"
local T=$OUT/latents run held name url
tool_pictures "$T"
(cd "$T" && cozy --json run -p "$T" "$here/project/boxes.py/shot" 'plate=<walk>' --on local) > "$OUT/latents.ndjson" 2> "$OUT/latents.err" || { check "latents: the run: $(tail -c 400 "$OUT/latents.err")"; return; }
run=$(tail -1 "$OUT/latents.ndjson" | jq -r .run.number)
ended "$run" > "$OUT/latents.json"
# Landed, an output has a url that serves its bytes (cl-819): none for what stays.
J '.output[] | select(.name == "latents") | .stays and .url == null and .sha256 != null' "$OUT/latents.json" || check "latents: a Latents field: $(jq -c '[.output[] | {name, stays, url}]' "$OUT/latents.json")"
J '[.output[] | select(.name == "video" or .name == "given")] | length == 2 and all(.url != null and (.stays | not))' "$OUT/latents.json" || check "latents: what else it made didn't come down: $(jq -c '[.output[] | {name, stays, url}]' "$OUT/latents.json")"
# Downloaded by its digest, each is what the run says it made; what stays isn't among them.
cozy run show "$run" --once -o "$OUT/latents-out" > "$OUT/latents-out.txt" 2>&1 || check "latents: run show -o: $(tail -c 300 "$OUT/latents-out.txt")"
for name in video given; do
  held=$(jq -r --arg n "$name" '.output[] | select(.name == $n) | .sha256' "$OUT/latents.json")
  [ "sha256:$(sha "$(ls "$OUT/latents-out/$run-$name"* 2> /dev/null | head -1)")" = "$held" ] || check "latents: $name downloaded isn't $held: $(ls "$OUT/latents-out")"
done
[ -z "$(ls "$OUT/latents-out" | grep -- "-latents")" ] || check "latents: what stays came down: $(ls "$OUT/latents-out")"
held=$(jq -r '.output[] | select(.name == "latents") | .sha256' "$OUT/latents.json")
{ sql1 "$T" "SELECT record FROM lock_files WHERE sha256 = '$held'" | grep -q '"machine":"local"' && [ ! -e "$T/files/${held#sha256:}" ]; } || check "latents: project.db doesn't say who holds them, or they came here: $(sql "$T" "SELECT record FROM lock_files WHERE sha256 = '$held'")"
[ "$(api -o /dev/null -w '%{http_code}' "http://$ADDR/v1/runs/$run/outputs/latents")" = 409 ] || check "latents: the output route served what stays"
(cd "$T" && cozy --json run -p "$T" "$here/project/shots.py/only_untyped" prompt=alone --on local) > "$OUT/untyped.ndjson" 2> "$OUT/untyped.err" || check "latents: an untyped run: $(tail -c 300 "$OUT/untyped.err")"
tail -1 "$OUT/untyped.ndjson" | J '.run.output[0] | (.stays | not) and .url != null' || check "latents: an untyped File didn't come down: $(tail -1 "$OUT/untyped.ndjson" | jq -c .run.output)"
url=$(tail -1 "$OUT/untyped.ndjson" | jq -r '.run.output[0].url')
[ "$(api -o "$OUT/untyped.blob" -w '%{http_code}' "http://$ADDR$url")" = 200 ] && [ "sha256:$(sha "$OUT/untyped.blob")" = "$(tail -1 "$OUT/untyped.ndjson" | jq -r '.run.output[0].sha256')" ] \
  || check "latents: $url doesn't serve the untyped File's sha256"
added "$T" <<YAML
  ours: {tool: $here/project/shaped.py/shot, summary: On <walk>., beats: [{duration: 2}]}
  theirs: {tool: $here/project/shots.py/shoot, summary: On <walk>.}
  up_ours: {tool: $here/project/shaped.py/up, video: <ours>}
  up_theirs: {tool: $here/project/shaped.py/up, video: <theirs>}
YAML
render "$T" up_ours up_theirs || { check "latents: the ups: $(tail -c 500 "$OUT/render.txt")"; return; }
J '.input.video | .type == "video" and (.latents | startswith("sha256:")) and .own == false' <<< "$(asked "$T" up_ours)" || check "latents: its own package's take didn't give its latents: $(asked "$T" up_ours | jq -c .input.video)"
J '.input.video | .type == "video" and .latents == null and (.file | startswith("sha256:"))' <<< "$(asked "$T" up_theirs)" || check "latents: another package's take gave its latents: $(asked "$T" up_theirs | jq -c .input.video)"
J '.absent == 0' <<< "$(status "$T")" || check "latents: status counts what stays on its machine as absent: $(status "$T" | jq -c .absent)"
}

# Pick up from here: only the gesture fills a pickup, from `cozy project render`, Calcifer's route and `cozy run`, and the take it makes says so.
pickup_take() {
step "a tool: Pick up from here reaches the function, writes nothing, and its take records it (weightless 10)"
local T=$OUT/pickup look before id input
mkdir -p "$T"; plate "$T/look.mp4" testsrc2 440 800
printf 'version: 4\nassets: {}\n' | yaml_in "$T"
look=$(prefix "$T" "$T/look.mp4")
yaml_in "$T" <<YAML
version: 4
assets:
  look: {approved: $look}
  clash: {tool: $here/project/shots.py/shoot, summary: Two lights meet on <look>.}
  shaped: {tool: $here/project/shaped.py/shot, summary: On <look>., beats: [{duration: 3}]}
YAML
given() { cat "$(file_at "$T" "$(take_file "$T" "$1" given "${2:--1}")")"; }
render "$T" clash shaped || { check "pickup: the first takes: $(tail -c 400 "$OUT/render.txt")"; return; }
J '.pickup == null' <<< "$(given clash)" || check "pickup: a render with no gesture picked up: $(given clash)"
before=$OUT/pickup-before.yaml; cozy project -p "$T" export > "$before"
render "$T" clash 'pickup.take=<clash.1>' pickup.out_frame=12 || check "pickup: render: $(tail -c 400 "$OUT/render.txt")"
J '.pickup.out_frame == 12' <<< "$(given clash)" || check "pickup: it didn't reach the function: $(given clash)"
J '.input.pickup | .out_frame == 12 and .take.type == "video" and (.take.file | startswith("sha256:"))' <<< "$(asked "$T" clash)" || check "pickup: the call: $(asked "$T" clash | jq -c .input.pickup)"
{ cmp -s "$before" <(cozy project -p "$T" export) && ! grep -q pickup <(cozy project -p "$T" export); } || check "pickup: the boxes changed: $(diff "$before" <(cozy project -p "$T" export))"
J '.versions[-1].pickup == {"take": "<clash.1>", "out_frame": 12} and .versions[0].pickup == null' <<< "$(held "$T" clash)" || check "pickup: the take doesn't record it: $(held "$T" clash | jq -c '[.versions[] | {number, pickup}]')"
says pickup "picks up <clash.1> at frame 12"
# On the frames the tool's take lands on, with its own extras.
render "$T" shaped 'pickup.take=<shaped.1>' pickup.out_frame=50 pickup.beats_from=1.5 || check "pickup: shaped: $(tail -c 400 "$OUT/render.txt")"
says pickup "frame 50 isn't one a pickup lands on: it picks up at 56"
input=$(asked "$T" shaped | jq -c .input.pickup)
J '.out_frame == 56 and .beats_from == 1.5 and .take.own == true and (.take.latents | startswith("sha256:"))' <<< "$input" || check "pickup: its own take, snapped: $input"
render "$T" shaped 'pickup.take=<clash.2>' pickup.out_frame=90 || check "pickup: another box's take: $(tail -c 400 "$OUT/render.txt")"
input=$(asked "$T" shaped | jq -c .input.pickup)
J '.out_frame == 90 and .take.own == false and .take.latents == null' <<< "$input" || check "pickup: another package's take: $input"
# Calcifer's route, as the page sends it.
id=$(status "$T" | jq -r .id)
api -X POST -H "Idempotency-Key: pickup-$RANDOM$RANDOM" -d '{"names": ["clash"], "on": ["local"], "pickup": {"take": "<clash.2>", "out_frame": 24}}' "http://$ADDR/v1/projects/$id/runs" > "$OUT/pickup-route.json"
for _ in $(seq 240); do [ "$(made "$T" clash)" = 3 ] && J '.busy | not' <<< "$(status "$T")" && break; sleep 0.5; done
J '.versions[-1].pickup == {"take": "<clash.2>", "out_frame": 24}' <<< "$(held "$T" clash)" && J '.pickup.out_frame == 24' <<< "$(given clash)" || check "pickup: POST runs: $(cat "$OUT/pickup-route.json") $(held "$T" clash | jq -c '[.versions[] | {number, pickup}]')"
# `cozy run`, by the same binder.
(cd "$T" && cozy --json run -p "$T" "$here/project/shots.py/shoot" 'summary=Two lights meet on <look>.' 'pickup.take=<clash.1>' pickup.out_frame=30 --on local) > "$OUT/pickup-run.ndjson" 2> "$OUT/pickup-run.err" || check "pickup: cozy run: $(tail -c 300 "$OUT/pickup-run.err")"
input=$(cozy --json --full run show "$(tail -1 "$OUT/pickup-run.ndjson" | jq -r .run.number)" --once | jq -c .request.input)
J '.pickup.out_frame == 30 and .pickup.take.type == "video" and (.summary.references | map(.key) == ["look"])' <<< "$input" || check "pickup: cozy run's call: $input"
# Written in the file, it's said and not sent.
printf '  written: {tool: %s/project/shots.py/shoot, summary: On <look>., pickup: {take: <clash.1>, out_frame: 5}}\n' "$here" | added "$T"
render "$T" written || check "pickup: a written pickup stopped the render: $(tail -c 400 "$OUT/render.txt")"
says pickup "a pickup isn't written; use Pick up from here"
J '.input | has("pickup") | not' <<< "$(asked "$T" written)" || check "pickup: a written pickup was sent: $(asked "$T" written | jq -c .input)"
}

# Paul, 11-06: no --on is the render set, and with no rental that's this computer. release-gate/sessions/run-no-on.sh holds a rental's case.
run_no_on() {
step "a run with no --on and no rental goes to this computer (weightless 11)"
local T=$OUT/no-on
if [ "$(cozy --json machine ls | jq '[.machines[] | select(.kind != "local" and (.state == "ready" or .state == "booting"))] | length')" != 0 ]; then
  echo "another machine is up: release-gate/sessions/run-no-on.sh is that case"; return 0
fi
mkdir -p "$T"; printf 'version: 4\nassets: {}\n' | yaml_in "$T"
(cd "$T" && cozy --json run -p "$T" "$here/project/tool.py/still" 'instructions=Wherever it ends first.') > "$OUT/no-on.ndjson" 2> "$OUT/no-on.err" || { check "no --on: $(tail -c 300 "$OUT/no-on.err")"; return; }
tail -1 "$OUT/no-on.ndjson" | J '.run.machine == "local" and .run.status == "completed"' || check "no --on: it went to $(tail -1 "$OUT/no-on.ndjson" | jq -c '.run | {machine, status}')"
}

# Runs that fail on purpose, on a machine with no history to keep clean: never alone.
failures() {
step "a project: a clip ffprobe can't read is left out and the rest is cut, a still among it"
local U=$OUT/unreadable look pic code=0
mkdir -p "$U"; plate "$U/look.mp4" testsrc2 440 800 4; still "$U/pic.png" navy
printf 'version: 4\nassets: {}\n' | yaml_in "$U"
look=$(prefix "$U" "$U/look.mp4"); pic=$(prefix "$U" "$U/pic.png")
yaml_in "$U" <<YAML
version: 4
assets:
  look:
    approved: $look
  pic:
    approved: $pic
  good:
    tool: $here/project/shots.py/fresh
    plate: <look>
    summary: Dusk on <look>.
  bad:
    tool: $here/project/shots.py/noise
    summary: Dusk on <look>.
  film:
    tool: cozy/edit
    tracks:
      - clips:
          - <good>
          - <bad>
          - <pic>
  hopeless:
    tool: cozy/edit
    tracks:
      - clips:
          - <bad>
YAML
cozy project -p "$U" render all --on local > "$OUT/render.txt" 2>&1 || code=$?
[ "$code" = 0 ] || check "unreadable: render exited $code: $(tail -c 400 "$OUT/render.txt")"
says unreadable "film.mp4 is cut without bad: "
says unreadable "holds no video ffprobe reads"
says unreadable "(without bad: ffprobe can't read it)"
[ "$(frames_of "$U/film.mp4")" = 168 ] || check "unreadable: the film is $(frames_of "$U/film.mp4") frames, not good's 96 and the still's 72"
J --arg bad "bad: ffprobe can't read it" '.cuts[] | select(.cut == "film.mp4") | .without == [$bad]' <<< "$(status "$U")" || check "unreadable: status: $(status "$U" | jq -c .cuts)"
cozy project -p "$U" status 2>&1 | grep -qF "(without bad: ffprobe can't read it" || check "unreadable: status doesn't say what it went without"
step "a project: with no clip to use, there is no film, or the one that stands stays, and no version is made"
says unreadable "hopeless.mp4 isn't cut, and no version is made: none of its clips can be used"
[ ! -e "$U/hopeless.mp4" ] || check "unreadable: a film was made of nothing"
[ "$(made "$U" hopeless)" = 0 ] || check "unreadable: nothing cut made a version: $(made "$U" hopeless)"
hand "$U" 's/- <good>/- <bad>/; $_ = "" if /^ *- <pic>$/'
code=0; render "$U" film || code=$?
[ "$code" = 0 ] || check "unreadable: no clip to use exited $code: $(tail -c 400 "$OUT/render.txt")"
says unreadable "film.mp4 isn't cut, and no version is made: none of its clips can be used: the film that stands stays"
[ "$(frames_of "$U/film.mp4")" = 168 ] || check "unreadable: the film that stood went: $(frames_of "$U/film.mp4") frames"
[ "$(made "$U" film)" = 1 ] || check "unreadable: nothing cut made a version: $(made "$U" film)"
step "a project: a failed run is no version, and its seed is kept for the next try"
local T=$OUT/failing look seed
mkdir -p "$T"; plate "$T/look.mp4" testsrc2 440 800
printf 'version: 4\nassets: {}\n' | yaml_in "$T"
look=$(prefix "$T" "$T/look.mp4")
yaml_in "$T" <<YAML
version: 4
assets:
  no:
    tool: $here/project/shots.py/shoot
    refuse: Not this one.
    summary: <look>.
  reader:
    tool: $here/project/shots.py/shoot
    summary: <no> and <look>.
  look:
    approved: $look
YAML
code=0; render "$T" reader || code=$?
{ [ "$code" != 0 ] && says failures "Not this one."; } || check "failures: its own refusal (exit $code): $(tail -c 400 "$OUT/render.txt")"
[ "$(made "$T" no)" = null ] || [ "$(made "$T" no)" = 0 ] || check "failures: a refusal made a version"
says failures "reader cannot render: no failed in this render"
stops
failed_once
absent_piece
rename_goes_on
renames_read
}

# A film short of its pieces, as a lock from 0.18.5 can hold one, isn't its state: any render cuts it whole, with its pieces as written.
# Naming a cut cuts it again.
short_film_heals() {
step "a project: a short film in its record repairs itself, and naming a cut cuts it"
local T=$OUT/healing a b short version
mkdir -p "$T"
plate "$T/a.mp4" testsrc2 440 800; plate "$T/b.mp4" smptebars 660 800; plate "$T/short.mp4" testsrc2 440 800 2
printf 'version: 4\nassets: {}\n' | yaml_in "$T"
a=$(prefix "$T" "$T/a.mp4"); b=$(prefix "$T" "$T/b.mp4"); short=$(prefix "$T" "$T/short.mp4")
yaml_in "$T" <<YAML
version: 4
assets:
  a: {approved: $a}
  b: {approved: $b}
  short_film: {approved: $short}
  edit:
    tool: cozy/edit
    tracks:
      - clips:
          - <a>
          - <b>
YAML
render "$T" edit || check "healing: first cut: $(tail -c 400 "$OUT/render.txt")"
J '.cuts[0].frames == 288' <<< "$(status "$T")" || check "healing: the whole film: $(status "$T" | jq -c .cuts)"
local whole; whole=$(take_file "$T" edit video)
db_replace "$T" "${whole#sha256:}" "$(sha "$T/short.mp4")"
rm -f "$T/edit.mp4"; cp "$T/short.mp4" "$T/edit.mp4"
[ "$(row "$T" edit .standing)" = '"out_of_date"' ] || check "healing: a short film stands as its state: $(row "$T" edit .status)"
render "$T" all --missing || check "healing: render: $(tail -c 400 "$OUT/render.txt")"
J '.cuts[0].frames == 288' <<< "$(status "$T")" || check "healing: not whole after a render: $(status "$T" | jq -c .cuts)"
[ "$(row "$T" edit .standing)" = '"current"' ] || check "healing: not current: $(row "$T" edit .status)"
n=$(ffprobe -v error -count_frames -select_streams v:0 -show_entries stream=nb_read_frames -of csv=p=0 "$T/edit.mp4"); [ "$n" = 288 ] || check "healing: edit.mp4 is really $n frames"
version=$(held "$T" edit | jq '.versions[-1].number')
render "$T" edit || check "healing: naming it: $(tail -c 400 "$OUT/render.txt")"
says healing "(unchanged)"
[ "$(held "$T" edit | jq '.versions[-1].number')" = "$version" ] || check "healing: the same bytes made a new version: $(tail -c 400 "$OUT/render.txt")"
}

# Takes cut in the walk that lands them, new bytes nothing has measured. First in the project part: on a new computer this is also
# what fetches Calcifer's ffprobe.
cut_of_takes() {
step "a project: a cut holds every take landed in its walk"
local T=$OUT/landed a b n
mkdir -p "$T/plates"
plate "$T/plates/a.mp4" testsrc2 440 800 4; plate "$T/plates/b.mp4" testsrc 550 800 4
printf 'version: 4\nassets: {}\n' | yaml_in "$T"
a=$(prefix "$T" "$T/plates/a.mp4") b=$(prefix "$T" "$T/plates/b.mp4")
yaml_in "$T" <<YAML
version: 4
assets:
  plate_a:
    approved: $a
  plate_b:
    approved: $b
  one:
    tool: $here/project/shots.py/fresh
    plate: <plate_a>
    summary: Dusk on <plate_a>.
  two:
    tool: $here/project/shots.py/fresh
    plate: <plate_b>
    summary: Dawn on <plate_b>.
  film:
    tool: cozy/edit
    fps: 24
    tracks:
      - clips:
          - <one>
          - <two>
YAML
filmed() {
  n=$(ffprobe -v error -count_frames -select_streams v:0 -show_entries stream=nb_read_frames -of csv=p=0 "$T/film.mp4" 2>/dev/null) || n=
  [ "$n" = 192 ] || check "landed: $1: film.mp4 is $n frames, not 192: $(grep -F 'left out' "$OUT/render.txt" | cut -c1-200)"
  ! grep -qF 'left out' "$OUT/render.txt" || check "landed: $1: a piece was left out: $(grep -F 'left out' "$OUT/render.txt" | cut -c1-200)"
  J --arg want "$(take_file "$T" two video -1)" 'any(.asset.prefixes[-1].timeline.tracks[0].clips[]; .sha256 == $want)' <<< "$(held "$T" film)" \
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
printf 'version: 4\nassets:\n' | yaml_in "$T"
for name in a b c d e f g h; do
  printf '  %s:\n    tool: %s/project/shots.py/crash\n    crash: the machine is broken\n' "$name" "$here" | added "$T"
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
printf 'version: 4\nassets: {}\n' | yaml_in "$T"
look=$(prefix "$T" "$T/look.mp4")
yaml_in "$T" <<YAML
version: 4
assets:
  look:
    approved: $look
  broken:
    tool: $here/project/shots.py/crash
    summary: <look>.
    crash: the card ran out of memory
  reader:
    tool: $here/project/shots.py/shoot
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
printf 'version: 4\nassets: {}\n' | yaml_in "$T"
a=$(prefix "$T" "$T/a.mp4"); b=$(prefix "$T" "$T/b.mp4")
yaml_in "$T" <<YAML
version: 4
assets:
  a: {approved: $a}
  b: {approved: $b}
  film:
    tool: cozy/edit
    tracks:
      - clips:
          - <a>
          - <b>
YAML
render "$T" film || check "absent: first cut: $(tail -c 400 "$OUT/render.txt")"
first=$(sha "$T/film.mp4")
rm -f "$T"/files/"$b"*
hand "$T" '$done ||= s/^( *)- <b>$/$&\n$1- <a>/'
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
stored=$(file_at "$T" "$a")
chmod u+w "$stored"; printf 'changed' >> "$stored"
render "$T" film || check "absent: a changed piece failed the render: $(tail -c 400 "$OUT/render.txt")"
says absent "(without a: its bytes changed)"
[ "$(frames_of "$T/film.mp4")" = 144 ] || check "absent: a changed piece was cut: $(frames_of "$T/film.mp4") frames"

step "a project: a piece with no version yet is left out, and its landing cuts it in"
local P=$OUT/arriving
mkdir -p "$P"
plate "$P/a.mp4" testsrc2 440 800
printf 'version: 4\nassets: {}\n' | yaml_in "$P"
a=$(prefix "$P" "$P/a.mp4")
yaml_in "$P" <<YAML
version: 4
assets:
  a: {approved: $a}
  later:
    tool: $here/project/shots.py/shoot
    summary: Dusk on <a>.
  film:
    tool: cozy/edit
    tracks:
      - clips:
          - <a>
          - <later>
YAML
render "$P" film || check "arriving: $(tail -c 400 "$OUT/render.txt")"
says arriving "(without later: no version yet)"
[ "$(frames_of "$P/film.mp4")" = 144 ] || check "arriving: the film is $(frames_of "$P/film.mp4") frames"
render "$P" later || check "arriving: later: $(tail -c 400 "$OUT/render.txt")"
[ "$(frames_of "$P/film.mp4")" = 288 ] || check "arriving: the film didn't cut it in: $(frames_of "$P/film.mp4") frames: $(tail -c 400 "$OUT/render.txt")"
! grep -qF 'without later' "$OUT/render.txt" || check "arriving: still says without: $(tail -c 400 "$OUT/render.txt")"
hand "$P" '$done ||= s/^( *)- <later>$/$&\n$1- <lost>/'
render "$P" film || check "arriving: a piece that names nothing failed the cut: $(tail -c 400 "$OUT/render.txt")"
says arriving "(without lost: no longer in the project)"
[ "$(frames_of "$P/film.mp4")" = 288 ] || check "arriving: with a piece that names nothing the film is $(frames_of "$P/film.mp4") frames"
}

# Status and the closing line say what the film holds, not what its pieces add up to; a piece with no version yet is warned of.
film_is_its_file() {
step "a project: a film's length is its file's, and a piece with no version warns"
local T=$OUT/truth a
mkdir -p "$T"
plate "$T/a.mp4" testsrc2 440 800
plate "$T/short.mp4" testsrc2 440 800 1
printf 'version: 4\nassets: {}\n' | yaml_in "$T"
a=$(prefix "$T" "$T/a.mp4")
yaml_in "$T" <<YAML
version: 4
assets:
  a: {approved: $a}
  later:
    tool: $here/project/shots.py/shoot
  film:
    tool: cozy/edit
    tracks:
      - clips:
          - <a>
          - <later>
YAML
render "$T" film || check "truth: cut: $(tail -c 400 "$OUT/render.txt")"
says truth "warning"
says truth "(without later: no version yet)"
J '.cuts[0].frames == 144' <<< "$(status "$T")" || check "truth: status of the film: $(status "$T" | jq -c .cuts)"
[ "$(stat -c %h "$T/film.mp4")" = 1 ] || check "truth: film.mp4 is a hard link: $(stat -c %h "$T/film.mp4") names"
local stored
stored=$(file_at "$T" "$(take_file "$T" film video -1)")
[ "$(id -u)" = 0 ] || [ ! -w "$stored" ] || check "truth: $stored can be written"
cp "$T/short.mp4" "$T/film.mp4"
J '.cuts[0].frames == 24' <<< "$(status "$T")" || check "truth: status doesn't read the file: $(status "$T" | jq -c .cuts)"
[ "$(sha "$stored")" = "$(basename "${stored%.*}")" ] || check "truth: writing over film.mp4 changed $stored"
}

# A cut's versions are all kept and listed; approving an older one brings it and its timeline to the root, and unpinning goes back.
cut_versions() {
step "a project: a cut keeps its versions, and the film follows the state"
local T=$OUT/edits a b one first_timeline two
mkdir -p "$T"
plate "$T/a.mp4" testsrc2 440 800; plate "$T/b.mp4" smptebars 660 800
printf 'version: 4\nassets: {}\n' | yaml_in "$T"
a=$(prefix "$T" "$T/a.mp4"); b=$(prefix "$T" "$T/b.mp4")
yaml_in "$T" <<YAML
version: 4
assets:
  a: {approved: $a}
  b: {approved: $b}
  edit:
    tool: cozy/edit
    tracks:
      - clips:
          - <a>
YAML
render "$T" edit || check "edits: first cut: $(tail -c 400 "$OUT/render.txt")"
one=$(sha "$T/edit.mp4"); first_timeline=$(sha "$T/edit.otio")
hand "$T" 's/^          - <a>$/          - <a>\n          - <b>/'
render "$T" edit || check "edits: second cut: $(tail -c 400 "$OUT/render.txt")"
two=$(sha "$T/edit.mp4")
[ "$one" != "$two" ] || check "edits: the second cut is the first"
cozy project -p "$T" status > "$OUT/edits-status.txt" 2>&1 || check "edits: cozy project status can't read a project with a cut: $(tail -c 300 "$OUT/edits-status.txt")"
cozy --json --full project -p "$T" status > "$OUT/edits-status.json" 2> "$OUT/edits-status.err" || check "edits: cozy --json project status can't read a project with a cut: $(tail -c 300 "$OUT/edits-status.err")"
J '[.assets[] | select(.name == "edit") | .prefixes[].timeline.tracks[0].clips[].sha256] | length == 3 and all(startswith("sha256:"))' "$OUT/edits-status.json" || check "edits: a clip's one hash: $(jq -c '.assets[] | select(.name == "edit") | .prefixes' "$OUT/edits-status.json")"
J '.assets[] | select(.name == "edit") | (.prefixes | length) == 2 and (.prefixes[0].from == ["a"]) and (.prefixes[1].from == ["a", "b"])' <<< "$(status "$T")" || check "edits: status: $(row "$T" edit .prefixes)"
ls "$T/files/$one"* > /dev/null 2>&1 || check "edits: the first film is gone from files/"
render "$T" edit || check "edits: naming it: $(tail -c 400 "$OUT/render.txt")"
says edits "(unchanged)"
[ "$(made "$T" edit)" = 2 ] || check "edits: the same pieces made a version: $(made "$T" edit)"
cozy project -p "$T" approve edit "$(held "$T" edit | jq '.versions[0].number')" > "$OUT/edits-approve.txt" 2>&1 || check "edits: approve: $(tail -c 300 "$OUT/edits-approve.txt")"
[ "$(sha "$T/edit.mp4")" = "$one" ] || check "edits: edit.mp4 isn't the approved film: $(tail -c 300 "$OUT/edits-approve.txt")"
[ "$(sha "$T/edit.otio")" = "$first_timeline" ] || check "edits: edit.otio isn't the approved film's"
cozy project -p "$T" unapprove edit > "$OUT/edits-unpin.txt" 2>&1 || check "edits: unpin: $(tail -c 300 "$OUT/edits-unpin.txt")"
[ "$(sha "$T/edit.mp4")" = "$two" ] || check "edits: edit.mp4 didn't go back to the state: $(tail -c 300 "$OUT/edits-unpin.txt")"
[ "$(made "$T" edit)" = 2 ] || check "edits: choosing made a version: $(made "$T" edit)"
}

# A rename asked while a render goes happens at once: by hand (its export imported), and by the one call.
rename_goes_on() {
step "a project: a rename during a render happens at once"
local T=$OUT/renaming going n
mkdir -p "$T"
yaml_in "$T" <<YAML
version: 4
assets:
  old:
    tool: $here/project/shots.py/slow
    seconds: 0
  whole:
    tool: $here/project/shots.py/slow
    seconds: 0
  busy:
    tool: $here/project/shots.py/slow
    seconds: 12
YAML
render "$T" old whole || check "rename: first render: $(tail -c 400 "$OUT/render.txt")"
cozy project -p "$T" render busy --on local > "$OUT/rename-busy.txt" 2>&1 &
going=$!
n=''
for _ in $(seq 120); do n=$(row "$T" busy .status | sed -n 's/.*making (run \([0-9]*\)).*/\1/p'); [ -n "$n" ] && break; sleep 0.25; done
[ -n "$n" ] || check "rename: the render never showed a run: $(tail -c 300 "$OUT/rename-busy.txt")"
# By hand, its export's import is the rename (cl-803): nothing more to run.
hand "$T" 's/^  old:/  new:/'
[ "$(made "$T" new)" = 1 ] || check "rename: new has $(made "$T" new) versions during the render"
cozy project -p "$T" rename whole entire > "$OUT/rename.txt" 2>&1 || check "rename: the one call, during a render: $(tail -c 300 "$OUT/rename.txt")"
grep -q '^  entire:$' <(cozy project -p "$T" export) || check "rename: the file did not change at once: $(cozy project -p "$T" export)"
[ "$(made "$T" entire)" = 1 ] || check "rename: entire has $(made "$T" entire) versions during the render"
cozy project -p "$T" rename busy going > "$OUT/rename.txt" 2>&1 || check "rename: what's rendering: $(tail -c 300 "$OUT/rename.txt")"
wait "$going" || check "rename: the render: $(tail -c 300 "$OUT/rename-busy.txt")"
[ "$(made "$T" new)" = 1 ] || check "rename: new has $(made "$T" new) versions after the render ended"
[ "$(made "$T" entire)" = 1 ] || check "rename: entire has $(made "$T" entire) versions after the render ended"
[ "$(made "$T" going)" = 1 ] || check "rename: what was rendering has $(made "$T" going) versions under its new name"
}

# A name is no input: renaming something leaves what reads it as it stood, whatever kind of reader it is.
renames_read() {
step "a project: a rename puts nothing out of date: a text reader, a text asset, an Edit, what's approved"
local T=$OUT/renames-read look name before
mkdir -p "$T"; plate "$T/look.mp4" testsrc2 440 800
printf 'version: 4\nassets: {}\n' | yaml_in "$T"
look=$(prefix "$T" "$T/look.mp4")
yaml_in "$T" <<YAML
version: 4
assets:
  look:
    approved: $look
  still:
    tool: $here/project/draw.py/draw
    instructions: A still.
  mood:
    text: Like <still> at dusk.
  poster:
    tool: $here/project/draw.py/draw
    instructions: <still> on a wall. <mood>
  framed:
    tool: $here/project/draw.py/draw
    instructions: <still> in a frame.
  clash:
    tool: $here/project/shots.py/shoot
    summary: Two lights meet on <look>.
  film:
    tool: cozy/edit
    tracks:
      - clips:
          - <clash>
YAML
render "$T" all || check "renames: render: $(tail -c 300 "$OUT/render.txt")"
cozy project -p "$T" approve framed 1 > /dev/null || check "renames: approve"
before=$(counts "$T")
cozy project -p "$T" rename still picture > /dev/null || check "renames: a read"
cozy project -p "$T" rename mood tone > /dev/null || check "renames: a text asset"
cozy project -p "$T" rename clash fight > /dev/null || check "renames: a clip"
# By hand, the import is the rename: an approved one too.
hand "$T" 's/^  poster:$/  print:/'
hand "$T" 's/^  framed:$/  hung:/'
J '[.assets[] | select(.standing != "current") | {name, status}] == []' <<< "$(status "$T")" || check "renames: out of date: $(status "$T" | jq -c '[.assets[] | select(.standing != "current") | {name, status}]')"
J '.versions[-1].approved' <<< "$(held "$T" hung)" || check "renames: its approval: $(held "$T" hung | jq -c .versions)"
render "$T" all --missing || check "renames: render what's missing: $(tail -c 300 "$OUT/render.txt")"
[ "$(counts "$T" | tr -d '"' | sed 's/picture/still/; s/tone/mood/; s/fight/clash/; s/print/poster/; s/hung/framed/')" = "$(tr -d '"' <<< "$before")" ] || check "renames: something ran: $before then $(counts "$T")"

step "a project: a rename by hand keeps the box's versions and approval, and puts nothing out of date; a description edit does (weightless 4)"
# As an editor saves it, with no `cozy project rename`: a read and its mentions, and an approved box.
hand "$T" 's/^  picture:$/  painting:/; s/<picture>/<painting>/g; s/^  hung:$/  shown:/'
J '[.assets[] | select(.standing != "current") | {name, status}] == []' <<< "$(status "$T")" || check "renames: by hand, out of date: $(status "$T" | jq -c '[.assets[] | select(.standing != "current") | {name, status}]')"
{ [ "$(made "$T" painting)" = 1 ] && [ "$(made "$T" shown)" = 1 ] && J '.versions[-1].approved' <<< "$(held "$T" shown)"; } || check "renames: by hand, a box lost its versions or approval: $(counts "$T"), $(held "$T" shown | jq -c '.versions')"
# Decision 3: a box's description goes to its readers with its picture.
hand "$T" 's/^  painting:$/  painting:\n    description: A still, on a wall./'
{ [ "$(row "$T" print .standing)" = '"out_of_date"' ] && [ "$(row "$T" painting .standing)" = '"current"' ]; } || check "renames: a description edit: print $(row "$T" print .standing), painting $(row "$T" painting .standing)"
}

# Three assets failing alike, a package that isn't there, so no run is made.
said_once() {
step "a project: a failure several assets share is said once"
local T=$OUT/once code=0
mkdir -p "$T"
yaml_in "$T" <<YAML
version: 4
assets:
  one:
    tool: $T/missing.py
    instructions: A coast.
  two:
    tool: $T/missing.py
    instructions: A harbor.
  three:
    tool: $T/missing.py
    instructions: A wall.
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
printf 'version: 4\nassets: {}\n' | yaml_in "$T"
A=$(prefix "$T" "$T/plates/a.mp4") B=$(prefix "$T" "$T/plates/b.mp4") R=$(prefix "$T" "$T/plates/r.mp4")
local SA SB SR; SA=$(sha "$T/plates/a.mp4") SB=$(sha "$T/plates/b.mp4") SR=$(sha "$T/plates/r.mp4")
yaml_in "$T" <<YAML
version: 4
assets:
  a:
    approved: $A
  b:
    approved: $B
  edit:
    tool: cozy/edit
    fps: 24
    tracks:
      - clips:
          - video: <a>
            in: 7
            out: 151
          - video: <b>
            in: 13
            out: 140
          - video: <a>
            in: 48
            out: 96
          - video: <b>
            in: 150
            out: 161
  trailer:
    tool: cozy/edit
    fps: 24
    tracks:
      - clips:
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
J '.width == 854 and .height == 480 and .fps == 24 and .frames == 192' <<< "$(sql1 "$T" "SELECT record FROM measured WHERE sha256 = 'sha256:$SA'")" || check "cuts: a's measures aren't kept: $(sql "$T" "SELECT record FROM measured WHERE sha256 = 'sha256:$SA'")"
J '[.cuts[].frames] == [330, 384] and ([.cuts[].cut] == ["edit.mp4", "trailer.mp4"])' <<< "$(status "$T")" || check "cuts: status: $(status "$T" | jq -c .cuts)"

step "a project: same takes, not cut again; the key follows the pieces"
local first made_at; first=$(sha "$T/edit.mp4"); made_at=$(stat -c %Y%N "$T/edit.mp4" 2>/dev/null || stat -c %Y "$T/edit.mp4")
render "$T" all --missing || check "cuts: render all again: $(tail -c 300 "$OUT/render.txt")"
says cuts "(unchanged)"
[ "$(stat -c %Y%N "$T/edit.mp4" 2>/dev/null || stat -c %Y "$T/edit.mp4")" = "$made_at" ] || check "cuts: same takes, cut again"
[ "$(made "$T" edit)" = 1 ] || check "cuts: a cut kept its old version"
cp "$T/edit.otio" "$OUT/edit.otio.was"; rm -f "$T/edit.otio"
render "$T" all --missing || check "cuts: with its .otio deleted: $(tail -c 300 "$OUT/render.txt")"
cmp -s "$T/edit.otio" "$OUT/edit.otio.was" || check "cuts: a deleted .otio did not come back the same"
[ "$(stat -c %Y%N "$T/edit.mp4" 2>/dev/null || stat -c %Y "$T/edit.mp4")" = "$made_at" ] || check "cuts: a deleted .otio cut again"

step "a project: a take at 25 fps, retimed; a take that isn't a piece any more"
hand "$T" "s/^    approved: \"?$B\"?\$/    approved: \"$R\"/"
render "$T" all --missing || check "cuts: with r: $(tail -c 300 "$OUT/render.txt")"
n=$(ffprobe -v error -count_frames -select_streams v:0 -show_entries stream=nb_read_frames,r_frame_rate -of csv=p=0 "$T/edit.mp4" 2>/dev/null) || n=
[ "$n" = "24/1,330" ] || check "cuts: the retimed edit.mp4 is $n, not 330 frames at 24/1"
says cuts "it retimes b (25 fps) to 24 fps"
"${FR[@]}" "$T/edit.mp4" 24 "$T/plates/a.mp4:7:151" "$T/plates/r.mp4:13:140" "$T/plates/a.mp4:48:96" "$T/plates/r.mp4:150:161" > "$OUT/frames-2.txt" 2>&1 || true
tail -1 "$OUT/frames-2.txt" | grep -q 'frames PASS' || check "cuts: the retimed cut's frames are not their sources': $(grep FAIL "$OUT/frames-2.txt" | head -3 | paste -sd' ')"
hand "$T" "s/^    approved: \"?$R\"?\$/    approved: \"$B\"/"
render "$T" all --missing || check "cuts: b back: $(tail -c 300 "$OUT/render.txt")"
[ "$(sha "$T/edit.mp4")" = "$first" ] || check "cuts: the first edit again is another file"

step "a project: approve recuts here, and a tool reads a newest out of date, and says so"
local S=$OUT/recut; mkdir -p "$S"; plate "$S/look.mp4" testsrc2 440 800 3
printf 'version: 4\nassets: {}\n' | yaml_in "$S"
A=$(prefix "$S" "$S/look.mp4")
yaml_in "$S" <<YAML
version: 4
assets:
  shot:
    tool: $here/project/shots.py/bare
    plate: <look>
    summary: One.
  look:
    approved: $A
  edit:
    tool: cozy/edit
    tracks:
      - clips:
          - <shot>
YAML
render "$S" all || check "recut: render all: $(tail -c 300 "$OUT/render.txt")"
[ -s "$S/edit.mp4" ] || check "recut: no edit.mp4"
hand "$S" 's/summary: One\./summary: Two./; s/^    tracks:$/    fps: 30\n$&/'
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
printf 'version: 4\nassets: {}\n' | yaml_in "$W"
A=$(prefix "$W" "$W/a.mp4") B=$(prefix "$W" "$W/b.mp4") R=$(prefix "$W" "$W/c.mp4")
yaml_in "$W" <<YAML
version: 4
assets:
  a:
    approved: $A
  b:
    approved: $B
  edit:
    tool: cozy/edit
    aspect_ratio: "16:9"
    tracks:
      - clips:
          - video: <a>
            in: 0
            out: 48
          - video: <b>
            in: 24
            out: 48
  whole:
    tool: cozy/edit
    aspect_ratio: "16:9"
    tracks:
      - clips:
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
hand "$W" "s/^    approved: \"?$B\"?\$/    approved: \"$R\"/"
render "$W" all --missing || check "rounded: with an 854x480 plate: $(tail -c 300 "$OUT/render.txt")"
[ "$(size "$W/edit.mp4")" = 1344,768 ] || check "rounded: a plate of another size: edit.mp4 is $(size "$W/edit.mp4"), not 1344,768"

# For the HTTP surface: a cut in the first project of the suite.
cp "$T/plates/a.mp4" "$P/a.mp4"
A=$(prefix "$P" "$P/a.mp4")
yaml_in "$P" <<YAML
version: 4
assets:
  a:
    approved: $A
  edit:
    tool: cozy/edit
    tracks:
      - clips:
          - <a>
YAML
render "$P" all || check "cuts: the HTTP surface's project: $(tail -c 300 "$OUT/render.txt")"
}

# `cozy project import` by path and URL: one call, a real file, a name, one constant, with no machine. Then what the
# page does by the same routes: a project by name, bytes by parts and resumed, an element by hash, its canvas row, its thumbnail.
imports() {
step "a project: import by path and by URL, and the name it gets"
local U=$OUT/imported code hash name lock=$HOME/.cozy/calcifer/calcifer.lock.holder ID
local seed=https://github.com/cozy-creator/cozy-creator/raw/master/release-gate/ref/seed.jpg
local lost=https://raw.githubusercontent.com/cozy-creator/cozy-creator/master/release-gate/ref/no-such.jpg
ADDR=$(jq -r .address "$lock") TOKEN=$(jq -r .token "$lock")
mkdir -p "$U"; printf 'version: 4\nassets: {}\n' | yaml_in "$U"
name=$(cozy project -p "$U" import "$seed" 2> "$OUT/import-url.err") || check "import: a URL: $(tail -c 300 "$OUT/import-url.err")"
hash=$(sha256sum "$U"/files/*.jpg | cut -c1-64)
{ [ "$name" = seed ] && [ ! -L "$U/files/$hash.jpg" ] && approved_in "$U" seed | grep -Eq "^    approved: \"?${hash:0:12}\"?$"; } \
  || check "import: a URL did not print its name, land as a real file and become one constant: $name; $(cozy project -p "$U" export); $(ls -l "$U/files" 2>&1 | head -3)"
[ "$(row "$U" seed .pinned)" = true ] || check "import: status does not show seed as a constant: $(row "$U" seed .)"
! sql "$U" "SELECT record FROM lock_files UNION ALL SELECT record FROM versions UNION ALL SELECT value FROM fields" | grep -q github || check "import: project.db names the URL"
code=0; cozy project -p "$U" import "$lost" > "$OUT/import-lost.txt" 2>&1 || code=$?
[ "$code" != 0 ] || check "import: a URL that is not there was accepted"
printf 'not a picture\n' > "$U/note.txt"
code=0; cozy project -p "$U" import "$U/note.txt" > "$OUT/import-txt.txt" 2>&1 || code=$?
[ "$code" != 0 ] || check "import: a text file was accepted"
still "$U/Kyogre Art (1).png" red
[ "$(cozy project -p "$U" import "$U/Kyogre Art (1).png")" = kyogre_art_1 ] || check "import: a stem is not lower-cased and made a name"
[ "$(cozy project -p "$U" import "$U/Kyogre Art (1).png" 'Hero.Sheet one')" = Hero_Sheet_one ] || check "import: a written name does not keep its case, with its dot and space as _ (a dot is a sub-selector, 7a5dd659b)"
code=0; cozy project -p "$U" import "$U/Kyogre Art (1).png" hero.sheet-one > "$OUT/import-taken.txt" 2>&1 || code=$?
{ [ "$code" != 0 ] && grep -q 'Hero_Sheet_one is in the project already' "$OUT/import-taken.txt"; } || check "import: a typed name taken in another case, with - for _ (exit $code): $(tail -c 300 "$OUT/import-taken.txt")"
[ "$(cozy project -p "$U" import "$U/Kyogre Art (1).png")" = kyogre_art_2 ] || check "import: a picked name doesn't count up"
[ "$(ls "$U/files" | grep -c "^$(sha "$U/Kyogre Art (1).png")")" = 1 ] || check "import: three elements of one file are not one file: $(ls "$U/files")"
[ "$(cozy --json project -p "$U" import "$U/Kyogre Art (1).png" | jq -r .already)" = true ] || check "import: a file the project has does not say already"
still "$U/joins.png" blue
name=$(cozy project -p "$U" import "$U/joins.png" --into kyogre_art_1 2> "$OUT/import-into.err") || check "import --into: $(tail -c 300 "$OUT/import-into.err")"
[ "$name" = kyogre_art_1.2 ] || check "import --into: a file did not join kyogre_art_1 as its version 2: $name"
[ "$(row "$U" kyogre_art_1 .versions)" = 2 ] || check "import --into: status does not count two versions: $(row "$U" kyogre_art_1 .)"
! grep -q '^  joins' <(cozy project -p "$U" export) || check "import --into: it made an asset of its own: $(cozy project -p "$U" export)"

step "a project: import keeps every other box as it was, and reads no name twice"
local Y=$OUT/imported-kept; mkdir -p "$Y"
yaml_in "$Y" <<'YAML'
# Before everything.
version: 4
assets:
  # Among the assets.
  hero:   # a name too
    description: |
      A hero,

      in two paragraphs.
  shot:
    summary: |
      Kept as it is, <hero> and all.
      # not a comment, though it looks like one

# Before the end.
YAML
cozy project -p "$Y" export > "$OUT/imported-kept.before"
still "$Y/hero.png" navy
[ "$(cozy project -p "$Y" import "$Y/hero.png")" = hero_2 ] || check "import: a name the project has is not taken"
cozy project -p "$Y" export | perl -0pe 's/\n  hero_2:\n    approved: "?\w+"?\n//' | diff - "$OUT/imported-kept.before" > "$OUT/imported-kept.diff" || check "import: other boxes moved: $(cat "$OUT/imported-kept.diff")"
J '.assets | map(.name) | sort == ["hero", "hero_2", "shot"]' <<< "$(status "$Y")" || check "import: the file does not read as it did with one asset more: $(status "$Y" | jq -c '.assets | map(.name)')"

step "a project: a take lifted out: a file by its hash, a name and words, and what it held goes on standing"
local lifted_from; lifted_from=$(ls "$Y/files" | head -1); lifted_from=${lifted_from%%.*}
local lifted; lifted=$(cozy --json project -p "$Y" import "${lifted_from#sha256:}" lifted_one --description "Its own words." | jq -r .name)
[ "$lifted" = lifted_one ] || check "lift: it took the name $lifted"
J '.assets[] | select(.name == "lifted_one") | .pinned and (.tool == null) and .fields.description == "Its own words."' <<< "$(status "$Y")" || check "lift: $(row "$Y" lifted_one .)"

step "a project: a name makes one, bytes by parts in any order and resumed, an element by hash"
local X P2 len hex mid url held big n
api -o "$OUT/named.json" -X POST -d '{"name":"web made.it"}' "http://$ADDR/v1/projects" > /dev/null
X=$(jq -r .path "$OUT/named.json") ID=$(jq -r .id "$OUT/named.json")
{ [ "$X" = "$HOME/web_made_it" ] && [ "$(jq -r .name "$OUT/named.json")" = web_made_it ] && ! grep -q '^  ' <(cozy project -p "$X" export); } || check "POST /v1/projects {name}: $(cat "$OUT/named.json")"
P2=$(api -X POST -d '{"name":"web made.it"}' "http://$ADDR/v1/projects" | jq -r .path)
[ "$P2" = "$X"_2 ] || check "POST /v1/projects {name}, again: $P2"
# A folder of anything in the home is left as it is: the next free name, and it says so.
mkdir -p "$HOME/web_taken" && echo mine > "$HOME/web_taken/keep"
api -o "$OUT/named-taken.json" -X POST -d '{"name":"web taken"}' "http://$ADDR/v1/projects" > /dev/null
{ [ "$(jq -r .path "$OUT/named-taken.json")" = "$HOME/web_taken_2" ] && [ "$(ls "$HOME/web_taken")" = keep ] && jq -e '.said | any(contains("is taken"))' "$OUT/named-taken.json" > /dev/null; } || check "POST /v1/projects {name}, a folder there: $(jq -c . "$OUT/named-taken.json")"
plate "$OUT/parts.mp4" testsrc2 440 800 2
len=$(stat -c %s "$OUT/parts.mp4") hex=$(sha "$OUT/parts.mp4") mid=$((len / 2))
url=http://$ADDR/v1/files/sha256:$hex
tail -c +$((mid + 1)) "$OUT/parts.mp4" > "$OUT/parts.tail"; head -c "$mid" "$OUT/parts.mp4" > "$OUT/parts.head"
code=$(api -o /dev/null -D "$OUT/part-first.hdr" -w '%{http_code}' -X PUT -H "Content-Range: bytes $mid-$((len - 1))/$len" --data-binary "@$OUT/parts.tail" "$url")
{ [ "$code" = 204 ] && grep -qi "^cozy-held: $mid-$((len - 1))" "$OUT/part-first.hdr"; } || check "a part: $code, $(cat "$OUT/part-first.hdr")"
code=$(api -o /dev/null -D "$OUT/part.hdr" -w '%{http_code}' -I "$url")
{ [ "$code" = 404 ] && grep -qi "^cozy-held: $mid-$((len - 1))" "$OUT/part.hdr"; } || check "HEAD of a partial file: $code, $(cat "$OUT/part.hdr")"
code=$(api -o "$OUT/part-last.json" -D "$OUT/part-last.hdr" -w '%{http_code}' -X PUT -H "Content-Range: bytes 0-$((mid - 1))/$len" --data-binary "@$OUT/parts.head" "$url")
{ [ "$code" = 201 ] && J --arg d "sha256:$hex" --argjson l "$len" '.digest == $d and .length == $l and .media_type == "video/mp4"' "$OUT/part-last.json"; } || check "the last part: $code, $(cat "$OUT/part-last.json")"
[ "$(api -o /dev/null -D "$OUT/part-whole.hdr" -w '%{http_code}' -I "$url")" = 200 ] || check "HEAD of a file whole by parts is not 200"
big=$OUT/killed.bin n=$((20 << 20)); head -c "$n" /dev/urandom > "$big"
hash=$(sha "$big"); url=http://$ADDR/v1/files/sha256:$hash
api -o /dev/null -X PUT --limit-rate 2M -H "Content-Range: bytes 0-$((n - 1))/$n" -T "$big" "$url" & local pid=$!
sleep 2; kill "$pid" 2> /dev/null || true; wait "$pid" 2> /dev/null || true
held=$(api -o /dev/null -D - -I "$url" | tr -d '\r' | sed -n 's/^[Cc]ozy-[Hh]eld: 0-//p')
[ -n "$held" ] && [ "$held" -lt $((n - 1)) ] || check "a killed upload: it held $held of $n"
tail -c +$((held + 2)) "$big" > "$big.rest"
echo "held 0-$held of $n after the kill" > "$OUT/resume.txt"
[ "$(api -o "$OUT/resume.json" -w '%{http_code}' -X PUT -H "Content-Range: bytes $((held + 1))-$((n - 1))/$n" -T "$big.rest" "$url")" = 201 ] || check "a killed upload did not resume from byte $((held + 1))"
J --arg d "sha256:$hash" --argjson n "$n" '.digest == $d and .length == $n' "$OUT/resume.json" || check "a resumed upload's answer: $(cat "$OUT/resume.json")"
head -c 100 "$big" > "$big.other"
[ "$(api -o "$OUT/part-mismatch.json" -w '%{http_code}' -X PUT -T "$big.other" "http://$ADDR/v1/files/sha256:$(sha "$OUT/parts.head")")" = 400 ] || check "bytes of another digest: $(cat "$OUT/part-mismatch.json")"
code=$(api -o "$OUT/by-hash.json" -w '%{http_code}' -X POST -H 'Idempotency-Key: place-1' -d "{\"source\":\"sha256:$hex\",\"name\":\"my clip\",\"at\":{\"x\":10.5,\"y\":-20,\"w\":320}}" "http://$ADDR/v1/projects/$ID/imports")
{ [ "$code" = 200 ] && J --arg s "sha256:$hex" --arg p "files/$hex.mp4" '.name == "my_clip" and .sha256 == $s and .field == "video" and .path == $p' "$OUT/by-hash.json"; } || check "import by hash: $code, $(cat "$OUT/by-hash.json")"
[ -f "$X/files/$hex.mp4" ] || check "import by hash: no file in the project"
[ "$(api -o /dev/null -w '%{http_code}' -I "http://$ADDR/v1/files/sha256:$hex")" = 404 ] || check "import by hash: the held file was copied, not moved"
api -o "$OUT/by-hash-2.json" -X POST -H 'Idempotency-Key: place-1' -d "{\"source\":\"sha256:$hex\",\"name\":\"my clip\",\"at\":{\"x\":10.5,\"y\":-20,\"w\":320}}" "http://$ADDR/v1/projects/$ID/imports" > /dev/null
{ diff -q "$OUT/by-hash.json" "$OUT/by-hash-2.json" > /dev/null && [ "$(grep -c my_clip <(cozy project -p "$X" export))" = 1 ]; } || check "the same Idempotency-Key made another element: $(cat "$OUT/by-hash-2.json")"
cozy project -p "$X" export > "$OUT/named-after-replay.yaml"
[ "$(api -o "$OUT/by-hash-conflict.json" -w '%{http_code}' -X POST -H 'Idempotency-Key: place-1' -d '{"source":"sha256:'"$hex"'","name":"other"}' "http://$ADDR/v1/projects/$ID/imports")" = 409 ] || check "one Idempotency-Key, another import: not refused"
api -o "$OUT/by-hash-again.json" -X POST -d "{\"source\":\"sha256:$hex\"}" "http://$ADDR/v1/projects/$ID/imports" > /dev/null
[ "$(jq -r '.name + " " + (.already | tostring)' "$OUT/by-hash-again.json")" = "file true" ] || check "the same file by hash: not a second element with no new bytes: $(cat "$OUT/by-hash-again.json")"
[ "$(ls "$X/files" | wc -l | tr -d ' ')" = 1 ] || check "two elements of one file are not one file: $(ls "$X/files")"
[ "$(api -o "$OUT/by-hash-unheld.json" -w '%{http_code}' -X POST -d "{\"source\":\"sha256:$(sha "$big.other")\"}" "http://$ADDR/v1/projects/$ID/imports")" = 404 ] || check "a hash held nowhere is not 404 file_not_held: $(cat "$OUT/by-hash-unheld.json")"
[ "$(api -o "$OUT/by-hash-no-media.json" -w '%{http_code}' -X POST -d "{\"source\":\"sha256:$hash\"}" "http://$ADDR/v1/projects/$ID/imports")" = 400 ] || check "a held file that is no media is not 400 import_failed: $(cat "$OUT/by-hash-no-media.json")"
! grep -q "$hash" "$OUT/by-hash-no-media.json" || check "refusing a dropped file shows its hash: $(cat "$OUT/by-hash-no-media.json")"

step "a project: the canvas moves elements and nothing else"
local inputs C=http://$ADDR/v1/projects/$ID/canvas E2
[ "$(sql "$X" "SELECT x, y, w FROM canvas WHERE name = 'my_clip'")" = '[10.5, -20.0, 320.0]' ] || check "canvas: import's at wrote no row: $(sql "$X" 'SELECT name, x, y, w FROM canvas')"
J '.cards.my_clip == {x: 10.5, y: -20, w: 320} and .seq > 0' <<< "$(api "$C")" || check "GET canvas: $(api "$C")"
inputs=$(status "$X" | jq -c '[.assets[] | [.name, .inputs]]')
E2=$OUT/canvas-events.txt; api -N --max-time 3 "http://$ADDR/v1/events" > "$E2" & pid=$!
sleep 1
J '.seq > 0' <<< "$(api -X PATCH -d '{"cards":{"zebra":{"x":1,"y":2},"my_clip":{"x":-5.125,"y":0},"apple":{"x":3,"y":4},"zebra2":null}}' "$C")" || check "PATCH canvas"
wait "$pid" 2> /dev/null || true
grep -q '"type":"canvas"' "$E2" && grep -q '"my_clip"' "$E2" || check "PATCH canvas: no canvas event: $(tail -c 300 "$E2")"
[ "$(sql "$X" "SELECT name FROM canvas ORDER BY name" | jq -r '.[0]' | paste -sd,)" = apple,my_clip,zebra ] || check "canvas: not one row per card: $(sql "$X" 'SELECT name, x, y, w FROM canvas')"
sql "$X" "SELECT x, y FROM canvas WHERE name = 'my_clip'" | grep -qxE '\[-5\.1(2|25|3), 0\.0\]' || check "canvas: a move: $(sql "$X" "SELECT x, y FROM canvas WHERE name = 'my_clip'")"
sql "$X" "SELECT name, x, y, w FROM canvas ORDER BY name" > "$OUT/named-canvas-moved.json"
[ "$(status "$X" | jq -c '[.assets[] | [.name, .inputs]]')" = "$inputs" ] || check "canvas: a move changed inputs"
api -o /dev/null -X PATCH -d '{"cards":{"zebra":null}}' "$C"; J '.cards | has("zebra") | not' <<< "$(api "$C")" || check "canvas: a null did not remove a row"
cozy project -p "$X" rename my_clip the_clip > "$OUT/rename-clip.txt" 2>&1 || check "rename of an element: $(cat "$OUT/rename-clip.txt")"
{ [ "$(sql1 "$X" "SELECT count(*) FROM canvas WHERE name = 'the_clip'")" = 1 ] && [ "$(sql1 "$X" "SELECT count(*) FROM canvas WHERE name = 'my_clip'")" = 0 ] && grep -q '^  the_clip:$' <(cozy project -p "$X" export) && ! grep -q my_clip <(cozy project -p "$X" export); } || check "rename did not move the key and the row: $(cozy project -p "$X" export) $(sql "$X" 'SELECT name FROM canvas')"
cozy project -p "$X" export > "$OUT/named-renamed.yaml"
cozy project -p "$X" reclaim --yes > "$OUT/reclaim-canvas.txt" 2>&1 || check "reclaim: $(cat "$OUT/reclaim-canvas.txt")"
[ "$(sql1 "$X" "SELECT count(*) FROM canvas WHERE name = 'apple'")" = 0 ] || check "reclaim did not prune a row nothing names: $(sql "$X" 'SELECT name FROM canvas')"
[ "$(sql1 "$X" "SELECT count(*) FROM canvas WHERE name = 'the_clip'")" = 1 ] || check "reclaim pruned a row something names"

step "a project: thumbnails, and what reading a project gives"
local T=http://$ADDR/v1/projects/$ID/thumbs thumb
ffmpeg -v error -y -f lavfi -i "color=c=red@0.5:size=1024x512,format=rgba" -frames:v 1 "$OUT/alpha.png"
ffmpeg -v error -y -f lavfi -i "sine=frequency=330:duration=2" "$OUT/sound.wav"
# A client's file goes by content: sent, then imported by its hash.
for file in alpha.png sound.wav; do api -o /dev/null -X PUT --data-binary "@$OUT/$file" "http://$ADDR/v1/files/sha256:$(sha "$OUT/$file")"; done
name=$(jq -nc --arg s "sha256:$(sha "$OUT/alpha.png")" '{source: $s, name: "alpha"}' | api -X POST -d @- "http://$ADDR/v1/projects/$ID/imports" | jq -r .sha256)
hash=$(jq -nc --arg s "sha256:$(sha "$OUT/sound.wav")" '{source: $s, name: "sound"}' | api -X POST -d @- "http://$ADDR/v1/projects/$ID/imports" | jq -r .sha256)
api -o "$OUT/thumb.png" -D "$OUT/thumb.hdr" "$T/$name?w=128" || true
{ grep -qi '^content-type: image/png' "$OUT/thumb.hdr" && grep -qi 'immutable' "$OUT/thumb.hdr" && [ "$(ffprobe -v error -select_streams v:0 -show_entries stream=width,pix_fmt -of csv=p=0 "$OUT/thumb.png")" = '128,rgba' ]; } \
  || check "a picture's thumbnail: $(cat "$OUT/thumb.hdr"); $(ffprobe -v error -select_streams v:0 -show_entries stream=width,pix_fmt -of csv=p=0 "$OUT/thumb.png" 2>&1)"
thumb=$(tr -d '\r' < "$OUT/thumb.hdr" | sed -n 's/^[Ee][Tt]ag: //p')
code=$(api -o /dev/null -w '%{http_code}' -H "If-None-Match: $thumb" "$T/$name?w=128"); echo "if-none-match $code" > "$OUT/thumb-codes.txt"; [ "$code" = 304 ] || check "a thumbnail's ETag: not 304"
code=$(api -o /dev/null -w '%{http_code}' -I "$T/$name?w=128"); echo "head $code" >> "$OUT/thumb-codes.txt"; [ "$code" = 200 ] || check "HEAD of a thumbnail"
api -o "$OUT/thumb.jpg" -D "$OUT/thumb.hdr" "$T/$hex?w=100" || true
{ grep -qi '^content-type: image/jpeg' "$OUT/thumb.hdr" && [ "$(ffprobe -v error -select_streams v:0 -show_entries stream=width -of csv=p=0 "$OUT/thumb.jpg")" = 100 ]; } || check "a video's poster: $(cat "$OUT/thumb.hdr")"
api -o "$OUT/strip.jpg" "$T/sha256:$hex?w=100&strip=4" || true
[ "$(ffprobe -v error -select_streams v:0 -show_entries stream=width -of csv=p=0 "$OUT/strip.jpg")" = 400 ] || check "a video's strip of 4 at 100 is not 400 wide"
api -o "$OUT/wave.png" -D "$OUT/thumb.hdr" "$T/$hash?w=200" || true
{ grep -qi '^content-type: image/png' "$OUT/thumb.hdr" && [ "$(ffprobe -v error -select_streams v:0 -show_entries stream=width -of csv=p=0 "$OUT/wave.png")" = 200 ]; } || check "a sound's waveform: $(cat "$OUT/thumb.hdr")"
code=$(api -o /dev/null -w '%{http_code}' "$T/$(sha "$OUT/parts.head")"); echo "no such file $code" >> "$OUT/thumb-codes.txt"; [ "$code" = 404 ] || check "a thumbnail of no such file is not 404"
step "a project: many thumbnails at once: each answers, ffmpegs stay few, and a read goes on meanwhile"
local w pids=() most=0 now read_took calcifer; calcifer=$(jq -r .pid "$lock")
: > "$OUT/thumb-many.txt"
for w in $(seq 200 20 440); do api -o /dev/null -w "$w %{http_code} %{time_total}\n" "$T/$hex?w=$w&strip=8" >> "$OUT/thumb-many.txt" & pids+=($!); done
read_took=$(api -o /dev/null -w '%{time_total}' "http://$ADDR/v1/projects/$ID") || read_took=failed
while [ -n "$(jobs -r -p | grep -xF -f <(printf '%s\n' "${pids[@]}"))" ]; do now=$(pgrep -c -x -P "$calcifer" ffmpeg || true); [ "${now:-0}" -le "$most" ] || most=$now; sleep 0.02; done
wait "${pids[@]}" || true
echo "most ffmpegs at once $most; a read meanwhile took $read_took s" | tee -a "$OUT/thumb-many.txt"
[ "$(grep -c ' 200 ' "$OUT/thumb-many.txt")" = "${#pids[@]}" ] || check "many thumbnails: not all answered: $(cat "$OUT/thumb-many.txt")"
{ [ "$most" -ge 1 ] && [ "$most" -le 2 ]; } || check "many thumbnails: $most ffmpegs at once, not one or two"
awk -v t="$read_took" 'BEGIN { exit !(t + 0 < 2) }' || check "many thumbnails held up a read: $read_took s"
! grep -q 'another cozy command is fetching' "$HOME/.cozy/calcifer/calcifer.log" || check "Calcifer raced itself for ffmpeg: $(grep -m1 'another cozy command' "$HOME/.cozy/calcifer/calcifer.log")"
J --arg s "$name" '[.assets[] | select(.name == "alpha")][0] | .sha256 == $s and .field == "image"' <<< "$(status "$X")" || check "reading: an element's hash and field: $(status "$X" | jq -c .assets)"
local tries=0; until J '[.assets[] | select(.name == "alpha" or .name == "sound" or .name == "file")] | map(.width // .duration_ms) | all' <<< "$(status "$X")" || [ "$tries" = 40 ]; do sleep 0.5; tries=$((tries + 1)); done
J '([.assets[] | select(.name == "alpha")][0] | .width == 1024 and .height == 512)
  and ([.assets[] | select(.name == "sound")][0] | .duration_ms > 1500)
  and ([.assets[] | select(.name == "file")][0] | .width == 854 and .height == 480 and .duration_ms > 1500)' <<< "$(status "$X")" || check "reading: what is measured: $(status "$X" | jq -c '[.assets[] | {name, width, height, duration_ms}]')"
added "$X" <<'YAML'
  made:
    tool: no-such-package-of-ours
    instructions: Anything.
YAML
hash=$(seq_of "$X"); status "$X" > "$OUT/named-status.json"; status "$X" > /dev/null
[ "$(seq_of "$X")" = "$hash" ] || check "reading a project wrote project.db"
cozy project -p "$X" export > "$OUT/named-final.yaml"
# The Hub reachable and not answering, as a stopped hubcheck is: packages never heard of, each of which a read once waited 3 s for.
if [ -n "${HUB_CHECK:-}" ]; then
  step "a project: reading it with the Hub not answering"
  added "$X" <<'YAML'
  frozen_one:
    tool: hub-frozen-one
    instructions: Anything.
  frozen_two:
    tool: hub-frozen-two
    instructions: Anything.
YAML
  local first second
  hash=$(seq_of "$X")
  kill -STOP "$HUB_CHECK" || true
  first=$(api --max-time 30 -o "$OUT/read-hub-frozen.json" -w '%{http_code} %{time_total}' "http://$ADDR/v1/projects/$ID") || first="failed $first"
  second=$(api --max-time 30 -o /dev/null -w '%{http_code} %{time_total}' "http://$ADDR/v1/projects/$ID") || second="failed $second"
  kill -CONT "$HUB_CHECK" || true
  echo "first $first; second $second" | tee "$OUT/read-hub-frozen.txt"
  [ "${first%% *}" = 200 ] && [ "${second%% *}" = 200 ] && awk -v a="${first##* }" -v b="${second##* }" 'BEGIN { exit !(a + 0 < 2 && b + 0 < 2) }' \
    || check "a read with the Hub not answering: $first; $second"
  J '.assets | any(.name == "frozen_one")' "$OUT/read-hub-frozen.json" || check "a read with the Hub not answering: no assets: $(head -c 300 "$OUT/read-hub-frozen.json")"
  [ "$(seq_of "$X")" = "$hash" ] || check "a read with the Hub not answering wrote project.db"
fi
api -o /dev/null -X DELETE "http://$ADDR/v1/projects/$ID"; api -o /dev/null -X DELETE "http://$ADDR/v1/projects/$(jq -r .id <<< "$(cozy --json project -p "$P2" status)")"

step "a project: a hash that reads as a number is that hash, whatever a person's file writes"
local V=$OUT/numbers over fits digits zero; mkdir -p "$V"; still "$V/base.png" teal; printf 'version: 4\nassets: {}\n' | yaml_in "$V"
# The same picture, with bytes after its end until its hash starts as each shape does. One prefix in about 150 has one of them.
python3 - "$V" > "$OUT/numbers.txt" <<'PY' || check "numbers: the fixtures: $(cat "$OUT/numbers.txt")"
import hashlib, pathlib, re, sys
v = pathlib.Path(sys.argv[1])
base = (v / "base.png").read_bytes()
shapes = {"over": r"[1-9][0-9]{0,6}e[0-9]{4,10}", "fits": r"[1-9][0-9]{8,9}e[0-9]{1,2}", "digits": r"[1-9][0-9]{11}", "zero": r"0[0-9]{11}"}
found, n = {}, 0
while len(found) < len(shapes):
    n += 1
    data = base + n.to_bytes(8, "big")
    prefix = hashlib.sha256(data).hexdigest()[:12]
    for name, shape in shapes.items():
        if name not in found and re.fullmatch(shape, prefix):
            found[name] = prefix
            (v / f"{name}.png").write_bytes(data)
for name, prefix in found.items():
    print(name, prefix)
PY
over=$(awk '$1 == "over" { print $2 }' "$OUT/numbers.txt") fits=$(awk '$1 == "fits" { print $2 }' "$OUT/numbers.txt")
digits=$(awk '$1 == "digits" { print $2 }' "$OUT/numbers.txt") zero=$(awk '$1 == "zero" { print $2 }' "$OUT/numbers.txt")
for name in over fits digits zero; do
  [ "$(cozy --json project -p "$V" import "$V/$name.png" "$name" | jq -r .name)" = "$name" ] || check "numbers: importing $name"
  approved_in "$V" "$name" | grep -Eq '^    approved: "?[0-9a-fe]{12}"?$' || check "numbers: cozy did not write $name: $(grep "$name" <(cozy project -p "$V" export))"
done
yaml_in "$V" <<YAML
version: 4
assets:
  over: {approved: $over}
  fits:
    approved: $fits
  digits:
    approved: $digits
  zero: {approved: $zero}
  reader:
    tool: $here/project/draw.py/draw
    instructions: "from <over>, <fits>, <digits> and <zero>"
YAML
status "$V" > "$OUT/numbers-status.json" || check "numbers: a person's file was refused: $(cozy project -p "$V" status 2>&1 | tail -c 300)"
for name in over fits digits zero; do
  J --arg n "$name" --arg p "sha256:$(eval echo "\$$name")" '[.assets[] | select(.name == $n)][0] | .pinned and (.files.image.sha256 | startswith($p))' "$OUT/numbers-status.json" || check "numbers: $name is not its file: $(jq -c --arg n "$name" '.assets[] | select(.name == $n) | {sha256, status}' "$OUT/numbers-status.json")"
done
J '[.assets[] | select(.name == "reader")][0].inputs != null' "$OUT/numbers-status.json" || check "numbers: what mentions them has no inputs: $(jq -c '.assets[] | select(.name == "reader")' "$OUT/numbers-status.json")"
! jq -c .warnings "$OUT/numbers-status.json" | grep -Eq "is not a hash|no file's hash" || check "numbers: warned: $(jq -c .warnings "$OUT/numbers-status.json")"
cozy project -p "$V" approve zero "$over" > "$OUT/numbers-approve.txt" 2>&1 || check "numbers: approving over one: $(cat "$OUT/numbers-approve.txt")"
grep -Eq "^  zero:|approved: \"$over\"" <(cozy project -p "$V" export) || check "numbers: approve wrote: $(grep -n -A2 'zero:' <(cozy project -p "$V" export))"

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
png=files/$(ls "$P/files" | grep -E '^[0-9a-f]{64}\.(png|mp4)$' | head -1)
api -D "$OUT/asset.hdr" -o "$OUT/asset.bin" "http://$ADDR/v1/projects/$ID/$png" || true
{ head -1 "$OUT/asset.hdr" | grep -q ' 200' && cmp -s "$OUT/asset.bin" "$P/$png" && grep -qi "etag: \"$(basename "${png%.*}")\"" "$OUT/asset.hdr"; } \
  || check "GET $png: $(tr -d '\r' < "$OUT/asset.hdr" | head -4 | paste -sd' ')"
grep -qi '^cache-control: .*immutable' "$OUT/asset.hdr" || check "a file named by its hash is not immutable: $(tr -d '\r' < "$OUT/asset.hdr" | paste -sd' ')"
grep -qi '^cache-control: no-store' "$OUT/head.hdr" || check "edit.mp4, named by revision, is not no-store: $(tr -d '\r' < "$OUT/head.hdr" | paste -sd' ')"
{ grep -qi '^content-security-policy: sandbox' "$OUT/head.hdr" && grep -qi '^content-disposition: inline' "$OUT/head.hdr"; } \
  || check "edit.mp4 is not sandboxed and shown in place: $(tr -d '\r' < "$OUT/head.hdr" | paste -sd' ')"
[ "$(api -o /dev/null -w '%{http_code}' -r 0-9 -H "If-Range: $et" "$U")" = 206 ] || check "a Range under edit.mp4's own If-Range is not 206"
[ "$(api -o /dev/null -w '%{http_code}' -r 0-9 -H 'If-Range: "another-revision"' "$U")" = 200 ] || check "a Range under another revision's If-Range is not the whole file"
hostile_outputs
# A rebinding Host, a foreign Origin, no bearer: each refused before anything is read or done.
[ "$(api -o /dev/null -w '%{http_code}' -H "Host: rebind.example:${ADDR##*:}" "http://$ADDR/v1/projects")" = 403 ] || check "a rebinding Host was not refused 403"
[ "$(api -o /dev/null -w '%{http_code}' -X POST -H 'Origin: http://evil.example' -H 'Content-Type: application/json' -d '{"names":["all"],"on":["local"]}' "http://$ADDR/v1/projects/$ID/runs")" = 403 ] \
  || check "a POST with a foreign Origin was not refused 403"
[ "$(curl -sS -o /dev/null -w '%{http_code}' "http://$ADDR/v1/projects")" = 401 ] || check "a request with no bearer was not refused 401"
[ "$(api -o "$OUT/calcifer-status.json" -w '%{http_code}' "http://$ADDR/v1/status")" = 200 ] || check "GET /v1/status: $(cat "$OUT/calcifer-status.json")"
J '(.version | length > 0) and (.os | length > 0) and (.local_usable | type == "boolean") and (.signed_in | type == "boolean")' "$OUT/calcifer-status.json" || check "GET /v1/status: $(cat "$OUT/calcifer-status.json")"
session_surface "$P" "$ID"
yaml_routes "$P" "$ID"
}

# What web-serves (e2e/cli/web.sh) leaves to a project: cozy open's link, a video's Range on ?media=, and what state.db keeps of a session.
session_surface() {
step "a project: its link, and what's kept of a session"
local P=$1 ID=$2 B=http://$ADDR code token media store=$HOME/.cozy/calcifer/state.db U
U=$B/v1/projects/$ID/cuts/edit.mp4
COZY_PROJECT=$ID cozy --json open --no-open > "$OUT/project-open.json" || check "session: open --no-open of COZY_PROJECT failed"
J --arg to "&to=/projects/$ID" --arg base "$B/#code=" '.url | startswith($base) and endswith($to)' "$OUT/project-open.json" || check "session: open's link of COZY_PROJECT: $(cat "$OUT/project-open.json")"
J '.url | contains("&to=") | not' <<< "$(cd "$P" && cozy --json open --no-open)" || check "session: open naming no project isn't the home, though it's run in a project's folder"
cozy --json open "$P/sub" --no-open > "$OUT/project-open-named.json" || check "session: open of a folder in it failed"
J --arg to "&to=/projects/$ID" '.url | endswith($to)' "$OUT/project-open-named.json" || check "session: open of a folder in it: $(cat "$OUT/project-open-named.json")"
mkdir -p "$OUT/open-none" "$OUT/open-old"; printf 'version: 1\nassets: {}\n' > "$OUT/open-old/project.yaml"
cozy open "$OUT/open-none" --no-open > "$OUT/open-none.txt" 2> "$OUT/open-none.err" || check "session: open of a folder of none failed"
{ grep -q '#code=' "$OUT/open-none.txt" && ! grep -q '&to=' "$OUT/open-none.txt" && grep -q 'cozy project new' "$OUT/open-none.err"; } || check "session: a folder of none: $(cat "$OUT/open-none.txt" "$OUT/open-none.err")"
code=0; cozy open "$OUT/open-old" --no-open > "$OUT/open-old.txt" 2>&1 || code=$?
{ [ "$code" != 0 ] && grep -qF 'cozy project new' "$OUT/open-old.txt" && ! grep -q '#code=' "$OUT/open-old.txt"; } || check "session: a format that can't convert says what to run: exit $code: $(cat "$OUT/open-old.txt")"
code=$(jq -r .url "$OUT/project-open.json" | sed -n 's/.*#code=\([0-9a-f]*\)&.*/\1/p')
[ "$(curl -sS -o "$OUT/session.json" -w '%{http_code}' -X POST -d "{\"code\":\"$code\"}" "$B/v1/session")" = 200 ] || check "session: its code was not traded: $(cat "$OUT/session.json")"
token=$(jq -r .token "$OUT/session.json") media=$(jq -r .media "$OUT/session.json")
{ [ "$(curl -sS -o "$OUT/media-range.bin" -w '%{http_code}' -r 1000-1999 "$U?media=$media")" = 206 ] && cmp -s "$OUT/media-range.bin" "$OUT/range.bin"; } || check "session: ?media= does not seek a cut"
[ "$(curl -sS -o /dev/null -w '%{http_code}' "$U")" = 401 ] || check "session: a cut answered with no credential, as to another site's <img>"
[ "$(curl -sS -o /dev/null -w '%{http_code}' "$U?media=$token")" = 401 ] || check "session: its token works as its media secret"
[ "$(stat -c %a "$store")" = 600 ] || check "session: state.db is $(stat -c %a "$store")"
[ "$(state1 "SELECT count(*) FROM sessions WHERE token_sha256 = '$(digest "$token")' AND media_sha256 = '$(digest "$media")'")" = 1 ] || check "session: state.db doesn't keep its digests"
! grep -aqE "$token|$media" "$store" "$store-wal" 2> /dev/null || check "session: state.db holds a secret, not its digest"
[ "$(curl -sS -o /dev/null -w '%{http_code}' -H "Authorization: Bearer $token" -X DELETE "$B/v1/session")" = 204 ] || check "session: DELETE did not answer 204"
}

# What a run wrote can't be script in Calcifer's origin: sandboxed, and a page is a download. The browser opens them too.
hostile_outputs() {
step "a project: a run's page and drawing are served sandboxed"
local output
cell hostile "$here/project/hostile.py/hostile" "prompt=hostile $RANDOM" || true
HOSTILE=$(cat "$OUT/hostile.number")
for output in page drawing; do
  api -I "http://$ADDR/v1/runs/$HOSTILE/outputs/$output" > "$OUT/hostile-$output.hdr" || true
  { head -1 "$OUT/hostile-$output.hdr" | grep -q ' 200' && grep -qi '^content-security-policy: sandbox' "$OUT/hostile-$output.hdr"; } \
    || check "hostile outputs: $output is not sandboxed: $(tr -d '\r' < "$OUT/hostile-$output.hdr" | paste -sd' ')"
done
grep -qi '^content-disposition: attachment' "$OUT/hostile-page.hdr" || check "hostile outputs: an HTML output is not a download: $(tr -d '\r' < "$OUT/hostile-page.hdr" | paste -sd' ')"
grep -qi '^content-disposition: inline' "$OUT/hostile-drawing.hdr" || check "hostile outputs: an SVG output is not an image: $(tr -d '\r' < "$OUT/hostile-drawing.hdr" | paste -sd' ')"
}

# The project's export over HTTP: read it, import over what was read, and never over a write made since.
yaml_routes() {
step "a project: its export read and imported over HTTP"
local P=$1 U=http://$ADDR/v1/projects/$2/yaml et next
cozy project -p "$P" export > "$OUT/yaml-kept.yaml"
api -D "$OUT/yaml.hdr" -o "$OUT/yaml.got" "$U" || true
et=$(grep -i '^etag:' "$OUT/yaml.hdr" | cut -d' ' -f2 | tr -d '\r')
{ head -1 "$OUT/yaml.hdr" | grep -q ' 200' && cmp -s "$OUT/yaml.got" "$OUT/yaml-kept.yaml" && [ -n "$et" ]; } || check "GET yaml: $(tr -d '\r' < "$OUT/yaml.hdr" | paste -sd' ')"
[ "$(api -o /dev/null -w '%{http_code}' -H "If-None-Match: $et" "$U")" = 304 ] || check "If-None-Match on yaml is not 304"
# Another's edit since it was read: a save naming what it read is refused, and changes nothing.
{ cat "$OUT/yaml.got"; printf '  theirs:\n    text: From another editor.\n'; } | yaml_in "$P"
cozy project -p "$P" export > "$OUT/yaml-theirs.yaml"
{ cat "$OUT/yaml.got"; printf '  mine:\n    text: Saved over HTTP.\n'; } > "$OUT/yaml-mine.yaml"
[ "$(api -o "$OUT/yaml-put.json" -w '%{http_code}' -X PUT -H "If-Match: $et" --data-binary @"$OUT/yaml-mine.yaml" "$U")" = 409 ] || check "a save over a project changed since it was read was not 409: $(cat "$OUT/yaml-put.json")"
J '.error.code == "project_changed"' "$OUT/yaml-put.json" || check "the stale save's refusal: $(cat "$OUT/yaml-put.json")"
cmp -s <(cozy project -p "$P" export) "$OUT/yaml-theirs.yaml" || check "a stale save changed the project"
et=$(api -I "$U" | grep -i '^etag:' | cut -d' ' -f2 | tr -d '\r')
printf 'version: 4\nassets: [\n' > "$OUT/yaml-broken.yaml"
[ "$(api -o "$OUT/yaml-put.json" -w '%{http_code}' -X PUT -H "If-Match: $et" --data-binary @"$OUT/yaml-broken.yaml" "$U")" = 400 ] || check "text that isn't a project was not refused 400: $(cat "$OUT/yaml-put.json")"
cmp -s <(cozy project -p "$P" export) "$OUT/yaml-theirs.yaml" || check "a refused save changed the project"
{ cat "$OUT/yaml-theirs.yaml"; printf '  mine:\n    text: Saved over HTTP.\n'; } > "$OUT/yaml-mine.yaml"
[ "$(api -D "$OUT/yaml-put.hdr" -o /dev/null -w '%{http_code}' -X PUT -H "If-Match: $et" --data-binary @"$OUT/yaml-mine.yaml" "$U")" = 204 ] || check "a good save was not 204"
grep -q '^  mine:$' <(cozy project -p "$P" export) || check "the save isn't in the project: $(cozy project -p "$P" export | tail -5)"
next=$(grep -i '^etag:' "$OUT/yaml-put.hdr" | cut -d' ' -f2 | tr -d '\r')
[ "$next" = "\"$(cozy project -p "$P" export | sha /dev/stdin)\"" ] || check "the save's ETag isn't its export's digest: $next"
yaml_in "$P" < "$OUT/yaml-kept.yaml"
}

# sse <file>: what a stream wrote, one JSON object a line.
sse() { sed -n 's/^data: //p' "$1"; }
# revs <file> <id>: each `rev` the stream told of that project.
revs() { sse "$1" | jq -r --arg i "$2" 'select(.type == "project.changed" and .id == $i) | .rev'; }
# soon <command…>: it comes true within 20 s.
soon() { for _ in $(seq 100); do "$@" && return 0; sleep 0.2; done; return 1; }
told() { sse "$1" | jq -e "select($2)" > /dev/null; }
told_more() { [ "$(revs "$1" "$2" | wc -l)" -gt "$3" ]; }

# `GET /v1/events`, read as the page reads it: the first event, an edit from outside, a landing that writes only the lock, the local
# machine, a comment when nothing happens, and the routes it replaced gone.
event_stream() {
step "a project: GET /v1/events"
local R=$OUT/stream log=$OUT/events.sse stream id n seen yaml real
cozy project new "$R" > /dev/null || check "stream: project new"
real=$(status "$R" | jq -r .id)
id=$(status "$R" | jq -r .id)
curl -sSN -H "Authorization: Bearer $TOKEN" "http://$ADDR/v1/events" > "$log" 2> "$OUT/events.err" & stream=$!
soon told "$log" '.type == "hello"' || check "stream: no hello: $(head -c 300 "$log"; cat "$OUT/events.err")"
sse "$log" | sed -n 1p | J --arg v "$(cozy --json status | jq -r .calcifer.version)" '.type == "hello" and .version == $v' || check "stream: its first event: $(sse "$log" | sed -n 1p)"

seen=$(revs "$log" "$id" | wc -l)
# A hand's edit, its export imported (project.db has no other writer to watch for, cl-803).
printf '  edited:\n    text: By hand.\n' | added "$R"
soon told_more "$log" "$id" "$seen" || check "stream: a hand's edit told nothing: $(sse "$log" | cut -c1-200 | tail -5)"

seen=$(revs "$log" "$id" | wc -l) yaml=$(cozy project -p "$R" export | sha /dev/stdin)
(cd "$R" && cozy run -p "$R" "$here/project/draw.py" instructions=stream) > "$OUT/stream-run.txt" 2>&1 || check "stream: a run in the project: $(tail -c 300 "$OUT/stream-run.txt")"
n=$(cd "$R" && cozy --json run ls -p "$R" --limit 1 | jq -r '.runs[0].number') || check "stream: run ls in the project failed"
[ "$(cozy project -p "$R" export | sha /dev/stdin)" = "$yaml" ] || check "stream: a landing changed the boxes, and this was to prove its own rows reach the stream"
soon told_more "$log" "$id" "$seen" || check "stream: a landing's lock told nothing"
soon told "$log" ".type == \"run.updated\" and .run.number == $n and .run.status == \"completed\" and .run.project == \"$real\"" \
  || check "stream: no run.updated says run $n completed in $real: $(sse "$log" | jq -c 'select(.type == "run.updated") | [.run.number, .run.status, .run.project]' | tail -5)"
soon told "$log" '.type == "machine.updated" and .machine.name == "local"' || check "stream: a run on local moved no machine.updated"
[ -z "$(revs "$log" "$id" | sort | uniq -d)" ] || check "stream: a rev told twice: $(revs "$log" "$id" | sort | uniq -d)"

soon grep -q '^:' "$log" || check "stream: no comment after 20 s"
kill -0 "$stream" 2> /dev/null || check "stream: it ended: $(cat "$OUT/events.err")"
kill "$stream" 2> /dev/null || true
for route in runs machines; do
  [ "$(api -o /dev/null -w '%{http_code}' "http://$ADDR/v1/$route/events")" = 404 ] || check "stream: /v1/$route/events still answers"
done
}

# running <screen>: what the local machine's row of `cozy machine ls` says under RUNNING.
running() { awk '!at { at = index($0, "RUNNING") } at && $1 == "local" { print substr($0, at, 7) + 0; exit }' "$1"; }
screen_says() { grep -qE "$2" "$1" 2> /dev/null; }
running_above() { [ "$(running "$1")" -gt "$2" ] 2> /dev/null; }
running_back() { [ "$(running "$1")" -le "$2" ] 2> /dev/null; }

# `cozy run ls` and `cozy machine ls` on a terminal: a change reaches them, and (when Calcifer here is this suite's) across a restart.
live_views() {
step "a project: the live views follow the stream"
local R=$OUT/live term n before code follower
cozy project new "$R" > /dev/null || check "live: project new"
python3 "$here/project/terminal.py" "$OUT/runs.screen" "$COZY" run ls --all > /dev/null & term=$!
soon screen_says "$OUT/runs.screen" 'TARGET' || check "live: cozy run ls drew nothing"
(cd "$R" && cozy run -p "$R" "$here/project/draw.py" instructions=live) > "$OUT/live-run.txt" 2>&1 || check "live: a run: $(tail -c 300 "$OUT/live-run.txt")"
n=$(cd "$R" && cozy --json run ls -p "$R" --limit 1 | jq -r '.runs[0].number') || check "live: run ls in the project failed"
soon screen_says "$OUT/runs.screen" "^ *$n .* completed" || check "live: cozy run ls does not show run $n completed: $(head -5 "$OUT/runs.screen")"

if [ -z "${ALONE:-}" ] && [ "$RUNNER_OS" = Linux ]; then
  step "a project: a live view across a Calcifer restart, the branch resync takes"
  # What the index knows and this Calcifer hasn't opened, as a page finds it: format 2 upgrades on its first read, and says so there, once.
  local G=$OUT/opened-by-id gid
  mkdir -p "$G"; printf 'version: 4\nassets: {}\n' | yaml_in "$G"
  gid=$(status "$G" | jq -r .id)
  # Its files from before, in place of its database, as a folder copied back from an old backup is.
  rm -f "$G"/project.db*
  printf 'version: 2\nassets:\n  a:\n    description: A.\n' > "$G/project.yaml"
  printf '{"format": 2, "assets": {}, "files": {}}\n' > "$G/project.lock"
  systemctl --user restart cozy-calcifer || check "live: systemctl --user restart cozy-calcifer failed"
  (cd "$R" && cozy run -p "$R" "$here/project/draw.py" instructions=restarted) > "$OUT/live-restarted.txt" 2>&1 || check "live: a run after the restart: $(tail -c 300 "$OUT/live-restarted.txt")"
  n=$(cd "$R" && cozy --json run ls -p "$R" --limit 1 | jq -r '.runs[0].number') || check "live: run ls after the restart failed"
  soon screen_says "$OUT/runs.screen" "^ *$n .* completed" || check "live: past a restart, cozy run ls does not show run $n: $(head -5 "$OUT/runs.screen")"
  ADDR=$(jq -r .address "$HOME/.cozy/calcifer/calcifer.lock.holder") TOKEN=$(jq -r .token "$HOME/.cozy/calcifer/calcifer.lock.holder")
  J '[.warnings[]?.message] | any(contains("project.yaml.v2"))' <<< "$(api "http://$ADDR/v1/projects/$gid")" || check "first open: a read by id doesn't say it upgraded: $(api "http://$ADDR/v1/projects/$gid" | jq -c .warnings)"
  grep -q '^version: 4$' <(cozy project -p "$G" export) || check "first open: a read by id didn't upgrade: $(cozy project -p "$G" export)"
  J '[.warnings[]?.message] | any(contains("project.yaml.v2")) | not' <<< "$(api "http://$ADDR/v1/projects/$gid")" || check "first open: a read by id said it twice"
fi
code=0; kill "$term" 2> /dev/null || true; wait "$term" || code=$?
[ "$code" = 0 ] || check "live: cozy run ls exited $code on q"

python3 "$here/project/terminal.py" "$OUT/machines.screen" "$COZY" machine ls > /dev/null & term=$!
soon screen_says "$OUT/machines.screen" '^ *local ' || check "live: cozy machine ls shows no local: $(head -5 "$OUT/machines.screen")"
before=$(running "$OUT/machines.screen")
(cd "$here" && cozy run -p "$R" ./weightless/wait seconds:=8 --on local > "$OUT/live-wait.txt" 2>&1) & follower=$!
# Its first run here may install first: it's judged while its follower lives.
until running_above "$OUT/machines.screen" "$before" || ! kill -0 "$follower" 2> /dev/null; do sleep 0.2; done
running_above "$OUT/machines.screen" "$before" || check "live: cozy machine ls never showed the run on local: $(head -5 "$OUT/machines.screen")"
wait "$follower" || check "live: the run on local failed: $(tail -c 300 "$OUT/live-wait.txt")"
soon running_back "$OUT/machines.screen" "$before" || check "live: cozy machine ls still shows the ended run: $(head -5 "$OUT/machines.screen")"
code=0; kill "$term" 2> /dev/null || true; wait "$term" || code=$?
[ "$code" = 0 ] || check "live: cozy machine ls exited $code on q"
}

# The browser session (project/browser.js) on the runner's Google Chrome, with playwright-core to drive it.
browser_session() {
step "a project: a browser on a Calcifer session, past a restart, and other origins refused"
local P=$1 ID=$2 port pw=$OUT/playwright server code_got lock=$HOME/.cozy/calcifer/calcifer.lock.holder was_addr was_token
CHROME=$(command -v google-chrome || true)
[ -n "$CHROME" ] || { check "no Google Chrome on this runner"; return; }
npm install -s --prefix "$pw" playwright-core@1.55.0 > "$OUT/npm.log" 2>&1 || { check "npm install playwright-core failed: $(tail -c 300 "$OUT/npm.log")"; return; }
port=$(python3 -c 'import socket; s = socket.socket(); s.bind(("127.0.0.1", 0)); print(s.getsockname()[1])')
python3 -m http.server "$port" --bind 127.0.0.1 --directory "$here/project" > "$OUT/cross-site.log" 2>&1 & server=$!
for _ in $(seq 50); do curl -fsS -o /dev/null "http://localhost:$port/cross-site.html" && break; sleep 0.1; done
# The re-cut the page follows: the edit a second shorter.
cozy project -p "$P" export | sed -E 's/^( +)- <a>$/\1- {video: <a>, start: 0, end: 120}/' > "$OUT/recut.yaml"
code_got=$(cd "$OUT" && cozy --json open --no-open | jq -r .url | sed -n 's/.*#code=\([0-9a-f]*\)$/\1/p')
local code=0 log=$HOME/.cozy/calcifer/calcifer.log seen pid
seen=$(stat -c %s "$log" 2>/dev/null || echo 0)
pid=$(jq -r .pid "$lock") was_addr=$(jq -r .address "$lock") was_token=$(jq -r .token "$lock")
CHROME=$CHROME PLAYWRIGHT=$pw/node_modules/playwright-core node "$here/project/browser.js" "$ADDR" "$code_got" "$ID" "$HOSTILE" "$port" \
  "'$COZY' project -p '$P' import '$OUT/recut.yaml' && '$COZY' project -p '$P' render all --on local" \
  "kill -TERM $pid; while kill -0 $pid 2> /dev/null; do sleep 0.1; done; '$COZY' project ls > /dev/null" > "$OUT/browser.json" 2> "$OUT/browser.err" || code=$?
kill "$server" 2> /dev/null || true
ADDR=$(jq -r .address "$lock") TOKEN=$(jq -r .token "$lock")
{ [ "$(jq -r .pid "$lock")" != "$pid" ] && [ "$TOKEN" != "$was_token" ]; } || check "browser: Calcifer did not restart: pid $pid is $(jq -r .pid "$lock")"
echo "restarted: pid $pid is now $(jq -r .pid "$lock"), at $ADDR"
[ "$ADDR" = "$was_addr" ] || check "browser: a restart moved Calcifer from $was_addr to $ADDR"
[ "$code" = 0 ] || { check "the browser failed: $(tail -c 400 "$OUT/browser.err")"; return; }
J '.session.status == 200 and .session.token and .session.media and .session.again == 400 and .session.again_code == "session_code_invalid"' "$OUT/browser.json" \
  || check "browser: a code did not work exactly once: $(jq -c .session "$OUT/browser.json")"
J '(.cookies | length == 0) and .read.status == 200' "$OUT/browser.json" || check "browser: no cookie and a read on the token: $(jq -c '{cookies, read}' "$OUT/browser.json")"
J '.video.seeked_to > 0 and (.ranges | any(.status == 206))' "$OUT/browser.json" \
  || check "browser: <video> did not seek edit.mp4 by Range on ?media=: $(jq -c '{video, ranges}' "$OUT/browser.json")"
J '.media_elsewhere | .machines == 401 and .project == 401 and .post == 401' "$OUT/browser.json" || check "browser: ?media= opened more than a file: $(jq -c .media_elsewhere "$OUT/browser.json")"
J '(.events | index("open")) and (.events | any(startswith("cut "))) and (.events | index("ended"))' "$OUT/browser.json" \
  || check "browser: the event stream did not follow the re-cut: $(jq -c .events "$OUT/browser.json")"
J '.after_restart == 200' "$OUT/browser.json" || check "browser: a Calcifer restart ended the session: $(jq -c .after_restart "$OUT/browser.json")"
J '.hostile.page.download and (.hostile.page.reached | length == 0)' "$OUT/browser.json" || check "browser: a run's HTML output: $(jq -c .hostile.page "$OUT/browser.json")"
J '(.hostile.drawing.download | not) and .hostile.drawing.status == 200 and (.hostile.drawing.reached | length == 0)' "$OUT/browser.json" || check "browser: a run's SVG output: $(jq -c .hostile.drawing "$OUT/browser.json")"
# What Calcifer saw of the others (its log names each refusal): each fetch and POST refused, none carried out, no credential sent.
tail -c +$((seen + 1)) "$log" | grep 'refused ' > "$OUT/browser-refused.txt" || true
{ grep -qE 'refused GET /v1/projects code="origin_not_allowed"' "$OUT/browser-refused.txt" \
  && [ "$(grep -cE "refused POST /v1/projects/$ID/runs code=\"origin_not_allowed\"" "$OUT/browser-refused.txt")" -ge 4 ] \
  && J '.other_pages | length == 2 and all(.results.fetch_get | startswith("not readable")) and all(.kept == 0) and all(.asked | length == 4 and all(.credential == false))' "$OUT/browser.json"; } \
  || check "browser: another origin was not refused by Calcifer: $(cat "$OUT/browser-refused.txt" | cut -c1-200 | paste -sd' '); pages $(jq -c .other_pages "$OUT/browser.json")"
[ "$(owed "$P")" = 0 ] && ! sql "$P" "SELECT record FROM walk" | grep -q nothing || check "another origin's POST started a render"
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
  run_project "$OUT/project" || fail "cozy project new failed"
  projects
  finish project
fi
