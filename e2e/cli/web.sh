# e2e/cli/web.sh: the `web-serves` cell, the part cli/run.sh calls `web_serves`, on every OS, with no machine or model. `cozy open
#`; Chrome redeems its code (web/serves.js), the page loads under its CSP with no violation, an event arrives on
# GET /v1/events; and what must be refused is: a foreign Host and Origin, a used or expired code, ?media= past a file route, a
# call with no holder or session. Then the page as a person uses it: Place (web/place.js), twenty pictures dropped at once (web/dropped.js), one stream across tabs and a
# `kill -9` of Calcifer (web/reconnect.js), edits with Calcifer stopped that land when it's back (web/offline.js), undo and redo of the project's history from the page and the CLI (web/undo.js), the Runs and Machines drawers with a real run from the CLI where this computer runs
# models (web/drawers.js), what each machine is doing (web/activity.js), sign-in through AuthKit's form (web/signin.js), any project as live boxes (web/see.js), zoom from the canvas's corner (web/zoom.js), the background and dots at every zoom (web/canvas.js), how every box and what's around the canvas are laid out (web/layout.js), assets as a person works them (web/nodes.js), a render this computer would do poorly asked first (web/localfit.js), a box's models and LoRAs, open (web/addons.js), a box at rest as Paul drew it (web/tiles.js), versions by name and a tool on a box (web/versions.js), right click Download (web/download.js), a starred asset first in Files (web/star.js), boxes copied, pasted and duplicated (web/copy.js), a click selects and a double click opens, Delete deletes (web/select.js), a box from its tool's fields (web/tools.js), a shot's segments played as they land (web/segments.js), a tool on a box of the kind it makes (web/kinds.js), deleting by a trash can (web/trash.js), a video scrubbed in Focus (web/scrub.js), a pulled shape snapping to a ratio its tool makes (web/aspect.js), the canvas where it was after Focus (web/refocus.js), no New on an empty box (web/empty.js), the right-side panel closed by a click off it (web/drawer.js), the tool under a box (web/toolword.js), typed words that read over a picture (web/legible.js), a Shot's beats laid out (web/beats.js), pasted mentions marked as typed and loaded (web/paste.js), a press from inside never dismissing (web/dismiss.js), every tool the Hub lists in the palette (web/palette.js), an orbit video turned (web/orbit.js), an Edit rendered (web/timeline.js),
# the CLI and the open canvas as one project (web/live.js), 5,000 assets (web/scale.js), projects closed once nothing reads them (web/closes.js), a package's own UI (web/package_ui.js), the
# canvas in a second engine, and a save past project.db held by another program (web/held.js).
# Alone (`e2e/cli/web.sh --out DIR`) it drives this computer's cozy as it is, in the default home. It runs nothing, and leaves
# Calcifer running: this computer's is shared.
here=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
J() { jq -e "$@" > /dev/null; }

# A path as a program outside this shell reads it.
web_path() { if [ "${RUNNER_OS:-}" = Windows ]; then cygpath -m "$1"; else echo "$1"; fi; }
# Each runner image has Google Chrome where its OS puts it.
web_chrome() {
  case ${RUNNER_OS:-$(uname -s)} in
    Windows) echo 'C:/Program Files/Google/Chrome/Application/chrome.exe' ;;
    macOS | Darwin) echo '/Applications/Google Chrome.app/Contents/MacOS/Google Chrome' ;;
    *) command -v google-chrome || true ;;
  esac
}
# answers <what> <status, a regex> <error code, or -> <curl args…>
answers() {
  local what=$1 want=$2 code=$3 got; shift 3
  got=$(curl -sS -o "$OUT/web-reply.json" -w '%{http_code}' "$@") || got="no answer"
  { grep -Eqx "$want" <<< "$got" && { [ "$code" = - ] || J --arg c "$code" '.error.code == $c' "$OUT/web-reply.json"; }; } \
    || check "web-serves: $what: $got, not $want ${code/#-/}: $(head -c 300 "$OUT/web-reply.json")"
}

# redeem <what> <status> <error code, or -> <code>
redeem() { answers "$1" "$2" "$3" -X POST -H 'Content-Type: application/json' -d "{\"code\":\"$4\"}" "$base/v1/session"; }

# The merge check keeps functional journeys here; layout runs in parallel and scale is an explicit benchmark.
web_core() { web_serves core; }
web_native() { web_serves native; }
web_firefox() { web_serves firefox; }

web_serves() {
step "web-serves: sign in, the page under its CSP, an event, and what is refused"
local scope=${1:-full} lock=$HOME/.cozy/calcifer/calcifer.lock.holder P=$OUT/web-project pic=$OUT/web-picture.png pw=$OUT/playwright
local addr token base chrome id
chrome=$(web_chrome)
[ -n "$chrome" ] && [ -x "$chrome" ] || { check "web-serves: no Google Chrome on this runner (${chrome:-none})"; return; }
npm install -s --prefix "$(web_path "$pw")" playwright-core@1.55.0 > "$OUT/web-npm.log" 2>&1 || { check "web-serves: npm install playwright-core failed: $(tail -c 300 "$OUT/web-npm.log")"; return; }
starter_project "$P" > "$OUT/web-new.txt" 2>&1 || { check "web-serves: project new failed: $(tail -c 300 "$OUT/web-new.txt")"; return; }
id=$(status "$P" | jq -r '.id // empty')
[ -n "$id" ] || { check "web-serves: the project has no id"; return; }
addr=$(jq -r .address < "$lock") token=$(jq -r .token < "$lock") base=http://$addr
# One red pixel.
base64 -d > "$pic" <<< 'iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR4nGP4z8DwHwAFAAH/q842iQAAAABJRU5ErkJggg=='
web_signed_in
if [ "$scope" = firefox ]; then
  web_engines "$chrome" "$pw" place
  return
fi
step "web-place: dropped and chosen files on the canvas, a rename, a move, a reload"
web_proof place "$chrome" "$pw" ''
step "web-reconnect: one stream across tabs, and kill -9 of Calcifer with no reload"
local restart=''
[ -n "${WEB_ALONE:-}" ] || restart=$(web_restart "$lock")
web_proof reconnect "$chrome" "$pw" "$restart"
step "web-instant: a box in the frame of its drop with its POST held, answers lost, a refusal, kill -9 mid-queue, a render whose answer is lost"
web_proof instant "$chrome" "$pw" "$restart"
jq -c '{drop: .seen.drop, reads: .seen.reads_after_a_drop}' "$OUT/web-instant.json" 2> /dev/null || true
if [ -n "$restart" ]; then
  step "web-offline: Calcifer stopped: Turbo, a prompt twice, approve, two moves, a rename, a delete and a drop, each shown at once and counted in the top bar; started again, all in project.db in its order"
  STOP=$(web_halt "$lock") START=$(web_start) web_proof offline "$chrome" "$pw" "$restart"
  jq -c '{said: .seen.said_offline, after: .seen.after}' "$OUT/web-offline.json" 2> /dev/null || true
  step "web-durable: what's typed flushed and merged by field: read back before its answer, through a Calcifer kill -9 with no reload, the box left at once, the page reloaded at once, another writer's words meanwhile"
  web_proof durable "$chrome" "$pw" "$restart"
  jq -c '.seen.restart' "$OUT/web-durable.json" 2> /dev/null || true
  step "web-undo: a delete, a merge, a tool off, words, a move, approve and unapprove, a take discarded, each undone and redone; the CLI's undone from the page and the page's from the CLI; a clobbering undo refused; undo offline, then a restart"
  STOP=$(web_halt "$lock") START=$(web_start) web_proof undo "$chrome" "$pw" "$restart"
  jq -c '{refused: .seen.refused, list: .seen.list}' "$OUT/web-undo.json" 2> /dev/null || true
fi
if [ "$scope" = native ]; then
  # Keep native file and process paths here. The surrounding CLI shard checks local-execution refusal.
  local no_hub=''
  [ -n "${WEB_ALONE:-}" ] || no_hub=$(web_restart "$lock" http://127.0.0.1:9)
  NO_HUB=$no_hub web_proof see "$chrome" "$pw" "$restart"
  return
fi
step "web-dropped: twenty pictures dropped at once, each first drawn where and as it stays, with its picture; nothing moves, resizes or swaps through its import, measurement and 10 s after, nor after a reload"
web_proof dropped "$chrome" "$pw" ''
step "web-drawers: a real run live to its end and a cancel, one log stream across tabs, the machines"
web_proof drawers "$chrome" "$pw" ''
step "web-activity: each machine top right, an install and a download listed once under it with their waiters; a cold render's face, each phase in order with its numbers and time left; Cancel waiting and rendering"
web_proof activity "$chrome" "$pw" ''
step "web-signin: AuthKit's form to a stand-in Hub: a new account, a wrong code, too soon, either email's code, signed out, a deleted account restored"
if [ -z "${WEB_ALONE:-}" ]; then
  TO_HUB=$(web_restart "$lock" @HUB@) web_proof signin "$chrome" "$pw" "$restart"
  jq -c '.seen | {stated_wait}' "$OUT/web-signin.json" 2> /dev/null || true
fi
step "web-see: a real project as live boxes: elements, no edges, a landed take with no reload, a kept failure, 300 elements measured, the project tabs"
local no_hub=''
[ -n "${WEB_ALONE:-}" ] || no_hub=$(web_restart "$lock" http://127.0.0.1:9)
NO_HUB=$no_hub web_proof see "$chrome" "$pw" "$restart"
jq -c '.seen.scale // empty' "$OUT/web-see.json" 2> /dev/null || true
step "web-zoom: − the zoom + in the canvas's corner, its menu's fit and 100%, on a box worked on, drawn at the canvas's scale, and 300 boxes panned"
web_proof zoom "$chrome" "$pw" ''
jq -c '{pan_70: .seen.pan_70, pan_11: .seen.pan_11}' "$OUT/web-zoom.json" 2> /dev/null || true
step "web-canvas: the background zoomed with the canvas, its farther layers less, and the dots over it and under the boxes, at 0.1 to 3 on each theme; a pan's and a zoom's frames"
web_proof canvas "$chrome" "$pw" ''
jq -c '.seen.frames // empty' "$OUT/web-canvas.json" 2> /dev/null || true
if [ "$scope" = full ]; then
  step "web-layout: every kind of box in every state, far out to close, at 1280x720, 1600x1000 and 2560x1440 at 2x, and the frame around the canvas: nothing on another, cut off, past its edge, touching or too small to read"
  web_proof layout "$chrome" "$pw" ''
fi
step "web-nodes: the tools, a box's controls, chips, approve, delete and Undo, and Paul's example where this computer runs models"
web_proof nodes "$chrome" "$pw" ''
step "web-localfit: a render this computer would do poorly asks on its box first; Rent a <card> first, Run here anyway kept by Calcifer; the card chosen"
local on_hub=''
[ -n "${WEB_ALONE:-}" ] || on_hub=$(web_restart "$lock" @HUB@)
HUB_RESTART=$on_hub web_proof localfit "$chrome" "$pw" "$restart"
jq -c '{line: .seen.line, placed: .seen.placed}' "$OUT/web-localfit.json" 2> /dev/null || true
step "web-addons: model and adapter parameters, no chips at rest, listed open, strength and removal behind More, a hand's file kept"
web_proof addons "$chrome" "$pw" ''
jq -c '.seen.skipped // empty' "$OUT/web-addons.json" 2> /dev/null || true
step "web-versions: <name.N> and <self.N>, a tool on and off a box, a picture joining one, a version deleted with Undo, reclaim, a rename during a real render, takes approved by a double tap, a merge, a box's shape, Render while it makes"
web_proof versions "$chrome" "$pw" ''
step "web-download: right click, Download: a box as hei.png, another take as hei.2.png, a run's output, each its own bytes, as an attachment Calcifer names"
web_proof download "$chrome" "$pw" ''
step "web-menu: a box with a tool, right clicked: no Change the tool; its tool changes only by a drop from the palette"
web_proof menu "$chrome" "$pw" ''
step "web-selected: a selected box has a thick ring of its own colour with a glow, 3px or more on screen at zoom 0.25 and 1 on paper and dark, one and several; hover differs; the box-select rectangle shows"
web_proof selected "$chrome" "$pw" ''
step "web-star: Star from a box's menu: in project.db, nothing out of date, first in Files with a ★, kept by a reload, Undo and Redo, Unstar"
web_proof star "$chrome" "$pw" ''
step "web-select: a click selects a box and opens nothing, a double click opens it; Backspace or Delete deletes it, open or not, and Undo brings it back; Backspace in words edits only them"
web_proof select "$chrome" "$pw" ''
step "web-copy: two boxes copied and pasted: new names, the same versions, approval and words, a mention between them naming the copy, one Undo; Ctrl+D, Alt and a drag, Duplicate; Ctrl+V in words is words"
web_proof copy "$chrome" "$pw" ''
step "web-tools: a box drawn from its tool's fields by type, groups two deep, slots refusing what doesn't fit, a video's range, previews, Pick up from here"
web_proof tools "$chrome" "$pw" ''
step "web-segments: a shot rendered in segments plays all it has made so far, joined, with its sound, while the next renders; its take lands in its place playing on (cl-907)"
web_proof segments "$chrome" "$pw" ''
step "web-kinds: a tool goes on a box of the kind it makes: a video tool refused on a picture; a video tool on a picture and an image tool on a video change nothing and say why at the bottom left; BiRefNet on it starts from <self.1>"
web_proof kinds "$chrome" "$pw" ''
step "web-refocus: leaving Focus by Esc, its close, Back, a click off, after zooming its picture and by way of a second box: the canvas exactly where it was"
web_proof refocus "$chrome" "$pw" ''
step "web-aspect: a corner pulled to 1237:689 lands on 16:9, on the canvas and in Focus"
web_proof aspect "$chrome" "$pw" ''
step "web-scrub: a video in Focus scrubs a frame at a time and picks up from there; an empty box with no tool asks nothing"
web_proof scrub "$chrome" "$pw" ''
step "web-trash: a take's discard and a box's delete are trash cans, Undo brings a take back, and Focus's close is an X"
web_proof trash "$chrome" "$pw" ''
step "web-toolword: a box with a tool says it under its picture (cl-932), readable and shrinking with the box; one with none says nothing"
web_proof toolword "$chrome" "$pw" ''
step "web-drawer: the right-side panel closes on a click off it or Esc; a selection from inside let go outside keeps it"
web_proof drawer "$chrome" "$pw" ''
step "web-empty: an empty box in Focus offers no New, and does once it has a take; a blank box never shows its description"
web_proof empty "$chrome" "$pw" ''
step "web-dismiss: a selection or a press from inside a surface let go outside keeps it open; pressed and let go outside, it closes"
web_proof dismiss "$chrome" "$pw" ''
step "web-legible: typed words over a busy picture are bold and plain on a panel-coloured surface, on paper and on dark; every prompt is bold"
web_proof legible "$chrome" "$pw" ''
step "web-beats: H3's Shot with three beats: six lines of words each, growing with them; a short length slider; no running total"
web_proof beats "$chrome" "$pw" ''
step "web-paste: lines pasted into a beat, plain and rich, mark each mention at once (_ and digits, .N, a range, an angle); typed and loaded, marked the same"
web_proof paste "$chrome" "$pw" ''
step "web-palette: every tool the Hub lists under what it makes, one dragged out and one onto a video box, one taken off and back, and with no Hub said so"
NO_HUB=$no_hub web_proof palette "$chrome" "$pw" "$restart"
step "web-orbit: a function's file declared an orbit, by cozy run and a box's take, into project.db and the page: no play button, a drag turns it and wraps, off and back by Undo, on for a dropped video"
web_orbit
step "web-timeline: real clips and a still in an Edit, trims and a reorder, rendered free, frame-exact, read back in its .otio, its versions"
# Real clips, made and counted by the ffmpeg Calcifer fetched for itself for thumbnails: one on a runner's PATH would keep it from fetching.
local own; own=$(find "$HOME/.cozy/tools/ffmpeg" -name 'ffprobe*' 2> /dev/null | head -1)
PATH=${own:+$(dirname "$own"):}$PATH web_proof timeline "$chrome" "$pw" ''
step "web-live: the real CLI and a hand's saves on an open canvas: each within a second, what's typed kept, 200 edits, the stream fallen behind"
web_proof live "$chrome" "$pw" ''
jq -c '{times: .seen.times_ms, burst: .seen.burst, behind: .seen.behind}' "$OUT/web-live.json" 2> /dev/null || true
step "web-previews: a rendering box shows what its run is making, faded under how far it's got, and its take replaces it"
web_proof previews "$chrome" "$pw" ''
jq -c '.seen.previews // empty' "$OUT/web-previews.json" 2> /dev/null || true
if [ "$scope" = full ]; then
  step "web-scale: 5,000 assets: status, opening it, one edit, a landing, panning, memory"
  web_proof scale "$chrome" "$pw" ''
  jq -c '{scale: .seen.scale}' "$OUT/web-scale.json" 2> /dev/null || true
  web_scale_said
fi
step "web-closes: twenty projects of 1,000 assets by the CLI and the page, each closed once nothing reads it, memory back, a reopen as quick, a render holding its own"
web_proof closes "$chrome" "$pw" ''
jq -c '{times: .seen.times, memory: .seen.memory_mb, open: .seen.open_median_s, reopen: .seen.reopen_median_s, render: .seen.render}' "$OUT/web-closes.json" 2> /dev/null || true
step "web-package-ui: a package's component drawn, its field saved and read back; what can't draw falls back; a release held to its hash"
HUB_RESTART=$on_hub web_proof package_ui "$chrome" "$pw" "$restart"
[ -n "${WEB_ALONE:-}" ] || web_engines "$chrome" "$pw"
web_held "$chrome" "$pw"
}

# xs-180, where this computer runs the tool: what a function says of its file rides its run's result, and an orbit's every frame is a
# keyframe (ffprobe, Calcifer's own). Its box on the page is web/orbit.js.
web_orbit() {
[ -n "$LOCAL" ] || return 0
local probe file keys
if cell orbit "$here/project/orbit.py/turn" frames:=24; then
  J '.result.video.metadata.orbit == {"degrees": 360, "direction": "cw", "loops": true}' "$OUT/orbit.json" || check "web-orbit: cozy run's result says no orbit: $(jq -c .result "$OUT/orbit.json" | head -c 300)"
  probe=$(find "$HOME/.cozy/tools/ffmpeg" -name 'ffprobe*' 2> /dev/null | head -1) file=$(output orbit mp4)
  keys=$("$probe" -v error -select_streams v:0 -show_entries frame=key_frame -of csv=p=0 "$file" | cut -d, -f1 | sort | uniq -c | tr -s ' ')
  [ "$keys" = " 24 1" ] || check "web-orbit: not every frame of $file is a keyframe (count key_frame): ${keys:-no frames}"
fi
web_proof orbit "$chrome" "$pw" ''
}

# cl-919: a deleted box resolves nowhere, Undo brings it back for the undo window, and then its files leave the project whether Calcifer
# ran across the window or not (web/deleted.js). The window is cut to 75 seconds by config.toml for the cell, and put back after.
web_deleted() {
  local chrome=$1 pw=$2 lock=$3 undo=75
  web_undo_window "$lock" "$undo"
  UNDO_S=$undo STOP=$(web_halt "$lock") START=$(web_start) web_proof deleted "$chrome" "$pw" ''
  web_undo_window "$lock" ''
}

# Calcifer started again with config.toml's calcifer.undo_seconds at $2, else none: the old one gone first, so the new one reads it.
web_undo_window() {
  local config=$HOME/.cozy/config.toml pid
  awk -v n="$2" '/^undo_seconds *=/ { next } { print } /^\[calcifer\]/ && n != "" { print "undo_seconds = " n; put = 1 } END { if (n != "" && !put) printf "\n[calcifer]\nundo_seconds = %s\n", n }' "$config" > "$config.next" && mv "$config.next" "$config"
  pid=$(jq -r .pid < "$1")
  kill -TERM "$pid" 2> /dev/null || true
  while kill -0 "$pid" 2> /dev/null; do sleep 0.2; done
  bash -c "$(web_start)" || true
}

# web-scale's runner speed (web/speed.js), its factor and the bar each timed cell was held to, in the job's log.
web_scale_said() {
  jq -r '.seen | "web-scale: runner \(.runner.cpu) x\(.runner.cores): \(.runner.median_ms) ms (\(.runner_after.median_ms // "?") ms at the end) against \(.runner.reference_ms) ms on a fast runner, factor \(.runner.factor); bars \(.bars | tostring)"' "$OUT/web-scale.json" 2> /dev/null || true
}

# A save past project.db's write lock, held by another program longer than Calcifer waits, from the page (web/held.js).
web_held() {
  local T=$OUT/web-held id
  step "web-held: a save past project.db held by another program: said, kept, and saved when it's let go"
  # A new project, its boxes by `cozy import` (cl-803: a project is project.db, and YAML its import).
  starter_project "$T" > "$OUT/web-held-new.txt" 2>&1 || { check "web-held: project new: $(tail -c 300 "$OUT/web-held-new.txt")"; return; }
  printf 'version: 4\nassets:\n  stays:\n    tool: cozy/edit\n  goes:\n    tool: cozy/edit\n' > "$OUT/web-held.yaml"
  project_api "$T" /yaml -X PUT -H 'Content-Type: application/yaml' --data-binary "@$OUT/web-held.yaml" > "$OUT/web-held-import.txt" 2>&1 || { check "web-held: import: $(tail -c 300 "$OUT/web-held-import.txt")"; return; }
  id=$(status "$T" | jq -r '.id // empty')
  [ -n "$id" ] || { check "web-held: the project has no id"; return; }
  HELD_FOLDER=$(web_path "$T") HELD_ID=$id web_proof held "$1" "$2" ''
}

# The cells the canvas lives by in a second engine: WebKit on a Mac, Firefox on Linux. Each is Playwright's own build, and WebKit's is not
# Safari: it finds what differs in the engine, drag and drop above all, and nothing of Safari's own.
web_engines() {
  local chrome=$1 pw=$2 ENGINE cell
  case ${RUNNER_OS:-} in
    macOS) ENGINE=webkit ;;
    # H.264 is the system's libavcodec on Linux: with none, Firefox says "This browser can't play it" (41966978063).
    Linux)
      ENGINE=firefox
      bash "$here/install-browser-deps.sh" "$pw" > "$OUT/web-firefox-deps.log" 2>&1 \
        || { check "web-firefox: dependencies failed: $(tail -c 300 "$OUT/web-firefox-deps.log")"; return; }
      ;;
    *) return 0 ;;
  esac
  step "web-$ENGINE: place, see and nodes, in Playwright's $ENGINE"
  node "$pw/node_modules/playwright-core/cli.js" install "$ENGINE" > "$OUT/web-$ENGINE-install.log" 2>&1 \
    || { check "web-$ENGINE: install failed: $(tail -c 300 "$OUT/web-$ENGINE-install.log")"; return; }
  for cell in ${3:-place see nodes}; do web_proof "$cell" "$chrome" "$pw" ''; done
}

# A command that kills this Calcifer and brings it back, as the page finds it: at its own port, its sessions kept. `cozy status` only
# reads, and Windows has no service manager to restart it, so this is a verb that needs Calcifer. It comes back on the Hub $2, else on this
# suite's: config.toml's, since a service hears nothing of a caller's environment, and it starts before that verb asks.
# The Hub goes in config.toml before the kill: a service manager starts a Calcifer killed so at once, reading it then.
web_restart() { echo "$(web_hub "${2:-}")$(web_stop "$1"); sleep 1; $(web_start "${2:-}")"; }
# config.toml names the Hub $1, else this suite's, for the next Calcifer to start: first, as a top-level key (after a [table] it
# would be that table's, ignored, and Calcifer would fall back to tensorhub.com).
web_hub() {
  local config=$HOME/.cozy/config.toml hub=${1:-${COZY_HUB_URL:-}}
  [ -z "$hub" ] || echo "{ echo 'hub_url = \"$hub\"'; grep -v '^hub_url' '$config'; } > '$config.next'; mv '$config.next' '$config'; "
}
web_stop() {
  if [ "${RUNNER_OS:-}" = Windows ]; then echo "taskkill //F //PID \$(jq -r .pid < '$1') > /dev/null"; else echo "kill -9 \$(jq -r .pid < '$1')"; fi
}
# Stopped until started again, as a service manager stops it: SIGTERM is a clean exit, which neither systemd's on-failure nor launchd's
# KeepAlive restarts, where a kill -9 is back at once. Windows has no service manager to restart it.
web_halt() {
  if [ "${RUNNER_OS:-}" = Windows ]; then web_stop "$1"; else echo "kill -TERM \$(jq -r .pid < '$1')"; fi
}
web_start() {
  local hub=${1:-${COZY_HUB_URL:-}}
  echo "${hub:+COZY_HUB_URL=$hub }'$COZY' machine ls > /dev/null 2>&1"
}

# web_proof <place|reconnect|drawers> <chrome> <playwright's prefix> <restart command, or nothing>: web/<it>.js, signed in by its own
# `cozy open` once its browser is up (page.js: signIn), on a project of its own by name, whose folder it deletes whatever
# happened.
web_proof() {
  local name=$1 chrome=$2 pw=$3 restart=$4 pass=$1${ENGINE:+-$ENGINE} json shot code=0 folder
  json=$OUT/web-$pass.json shot=$(web_path "$OUT/web-$pass")
  # A screenshot in WebKit sets off the CSP on any page, with an inline style of Playwright's: its pass keeps its last look, past caring, and no others.
  [ "${ENGINE:-}" != webkit ] || shot=''
  HOLDER=$(web_path "$HOME/.cozy/calcifer/calcifer.lock.holder") ARGV_LOG=$(web_path "$OUT/argv.log") ALONE=${WEB_ALONE:-} RESTART=$restart ENGINE=${ENGINE:-} LAST_LOOK=$(web_path "$OUT/web-$pass") SHOT=$shot CHROME=$chrome PLAYWRIGHT=$(web_path "$pw/node_modules/playwright-core") COZY=$(web_path "$COZY") \
    node "$(web_path "$here/web/$name.js")" > "$json" 2> "$OUT/web-$pass.err" || code=$?
  [ "$code" = 0 ] || check "web-$pass: failed $(jq -c .failures "$json" 2> /dev/null): $(jq -c '.seen | with_entries(select(.value != true))' "$json" 2> /dev/null | head -c 800) $(tail -c 500 "$OUT/web-$pass.err")"
  # What died on another Hub doesn't leave the next proof on it.
  [ "$code" = 0 ] || [ -z "$restart" ] || bash -c "$restart" || true
  folder=$(jq -r '.seen.project.folder // empty' "$json" 2> /dev/null)
  case $folder in */web-*) rm -rf "$folder" ;; esac
}

# web_serves's own, on its project.
web_signed_in() {
local code=0 wait expires
HOLDER=$(web_path "$lock") ARGV_LOG=$(web_path "$OUT/argv.log") CHROME=$chrome PLAYWRIGHT=$(web_path "$pw/node_modules/playwright-core") COZY=$(web_path "$COZY") \
  node "$(web_path "$here/web/serves.js")" "$id" "$(web_path "$P")" "$(web_path "$pic")" > "$OUT/web.json" 2> "$OUT/web.err" || code=$?
[ "$code" = 0 ] || check "web-serves: the browser failed: $(tail -c 500 "$OUT/web.err")"
J '.session.status == 200 and (.session.token | length > 0) and (.session.media | length > 0)' "$OUT/web.json" \
  || { check "web-serves: the page did not redeem its code: $(jq -c '{session, signed_in}' "$OUT/web.json")"; return; }
local session media
session=$(jq -r .session.token "$OUT/web.json") media=$(jq -r .session.media "$OUT/web.json")

J '.document.status == 200 and .document.referrer_policy == "no-referrer"' "$OUT/web.json" || check "web-serves: GET /: $(jq -c .document "$OUT/web.json")"
for directive in "default-src 'none'" "script-src 'self' 'wasm-unsafe-eval'" "style-src 'self'" "img-src 'self' blob: data:" "media-src 'self' blob:" "font-src 'self'" "connect-src 'self'" "base-uri 'none'" "form-action 'none'" "frame-ancestors 'none'"; do
  J --arg d "$directive" '.document.csp // "" | contains($d)' "$OUT/web.json" || check "web-serves: the page's CSP has no $directive: $(jq -c .document.csp "$OUT/web.json")"
done
J ".document.csp // \"\" | contains(\"'unsafe-eval'\") | not" "$OUT/web.json" || check "web-serves: the page's CSP allows eval: $(jq -c .document.csp "$OUT/web.json")"
J '(.violations | length) == 0 and (.page_errors | length) == 0' "$OUT/web.json" || check "web-serves: the page broke under its CSP: $(jq -c '{violations, page_errors}' "$OUT/web.json")"
J '.console_errors | length == 0' "$OUT/web.json" || check "web-serves: the page's console has errors, such as a route Calcifer lacks: $(jq -c .console_errors "$OUT/web.json")"
J '[.document.set_cookie, .session.set_cookie] == [null, null] and (.cookies | length) == 0' "$OUT/web.json" || check "web-serves: a cookie was set: $(jq -c '{document, session, cookies}' "$OUT/web.json")"
J '(.assets | length) > 0 and (.assets | all(.status == 200 and (.cache_control // "" | contains("immutable"))))' "$OUT/web.json" || check "web-serves: /assets/* is not 200 and immutable: $(jq -c .assets "$OUT/web.json")"
J --arg b "Bearer $session" '.page_stream.status == 200 and (.page_stream.content_type // "" | startswith("text/event-stream")) and .page_stream.authorization == $b' "$OUT/web.json" \
  || check "web-serves: the page did not open GET /v1/events on its session: status $(jq -c .page_stream.status "$OUT/web.json")"
J --arg id "$id" '.stream_status == 200 and .streamed[0].type == "hello" and (.streamed | any(.type == "project.changed" and .id == $id))' "$OUT/web.json" \
  || check "web-serves: no hello then project.changed on GET /v1/events: $(jq -c '{stream_status, streamed}' "$OUT/web.json")"

# The Host a person's SSH tunnel sends passes, a foreign one and a foreign Origin do not.
local port=${addr##*:} file
answers "the session on GET /v1/status" 200 - -H "Authorization: Bearer $session" "$base/v1/status"
answers "a loopback Host on another port" 200 - -H "Authorization: Bearer $session" -H 'Host: localhost:1' "$base/v1/status"
answers "a foreign Host on the page" 403 host_not_allowed -H "Host: rebind.example:$port" "$base/"
answers "a foreign Host with the holder's bearer" 403 host_not_allowed -H "Authorization: Bearer $token" -H "Host: rebind.example:$port" "$base/v1/status"
answers "a foreign Origin" 403 origin_not_allowed -H "Authorization: Bearer $token" -H 'Origin: http://evil.example' "$base/v1/status"
answers "another port's Origin" 403 origin_not_allowed -H "Authorization: Bearer $token" -H 'Origin: http://127.0.0.1:1' "$base/v1/status"
answers "the page's own Origin" 200 - -H "Authorization: Bearer $session" -H "Origin: $base" "$base/v1/status"

# A code works once, and a fresh one only until it expires (below).
local once other fresh
answers "the holder asks for a code" 200 - -X POST -H "Authorization: Bearer $token" "$base/v1/session-codes"
once=$(jq -r '.code // empty' "$OUT/web-reply.json")
redeem "a code" 200 - "$once"
other=$(jq -r '.token // empty' "$OUT/web-reply.json")
redeem "a used code" 400 session_code_invalid "$once"
redeem "an unknown code" 400 session_code_invalid 00
answers "ending that session" 204 - -X DELETE -H "Authorization: Bearer $other" "$base/v1/session"

answers "no credential" 401 unauthenticated "$base/v1/machines"
answers "a code asked for with no credential" 401 unauthenticated -X POST "$base/v1/session-codes"
answers "a code asked for by a session, not the holder" 403 session_code_needs_bearer -X POST -H "Authorization: Bearer $session" "$base/v1/session-codes"
issued_before=$(date +%s)
answers "the holder asks for another code" 200 - -X POST -H "Authorization: Bearer $token" "$base/v1/session-codes"
issued_after=$(date +%s)
fresh=$(jq -r '.code // empty' "$OUT/web-reply.json") expires=$(jq -r '.expires_ms // 0' "$OUT/web-reply.json")
issued_at=$(( expires / 1000 - 900 ))
[ "$issued_at" -ge "$issued_before" ] && [ "$issued_at" -le "$issued_after" ] && [ -n "$fresh" ] || check "web-serves: sign-in code lifetime is not 15 minutes"

file=$(jq -r '.imported.path // empty' "$OUT/web.json"); file=${file##*/}
if [ -n "$file" ]; then
  answers "?media= on GET of a file" 200 - "$base/v1/projects/$id/files/$file?media=$media"
  answers "?media= on HEAD of a file" 200 - -I "$base/v1/projects/$id/files/$file?media=$media"
  answers "?media= on a write to a file route" 401 unauthenticated -X PUT --data-binary "@$pic" "$base/v1/files/sha256:$(jq -r .imported.sha256 "$OUT/web.json")?media=$media"
else
  check "web-serves: the import named no file: $(jq -c .imported "$OUT/web.json")"
fi
answers "?media= past a file route" 401 unauthenticated "$base/v1/machines?media=$media"
answers "?media= on a write" 401 unauthenticated -X POST -H 'Content-Type: application/json' -d '{"path":"/nowhere"}' "$base/v1/projects?media=$media"
answers "?media= as a bearer" 401 unauthenticated -H "Authorization: Bearer $media" "$base/v1/machines"

answers "ending the session" 204 - -X DELETE -H "Authorization: Bearer $session" "$base/v1/session"
answers "an ended session" 401 unauthenticated -H "Authorization: Bearer $session" "$base/v1/machines"
answers "an ended session's ?media=" 401 unauthenticated "$base/v1/projects/$id/files/$file?media=$media"

# Expiry boundaries and restart durability are checked without sleeping in sessions::tests.

}

# Alone: this computer's cozy, as it is, in the default home.
if [ "${BASH_SOURCE[0]}" = "$0" ]; then
  . "$here/../lib.sh"
  parse "$@"
  hub_checked() { :; }
  WEB_ALONE=1
  web_serves
  finish web-serves
fi
