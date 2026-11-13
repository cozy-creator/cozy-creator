# e2e/cli/web.sh: the `web-serves` cell, the part cli/run.sh calls `web_serves`, on every OS, with no machine or model. Chrome opens
# Calcifer's page with nothing minted or kept (web/serves.js), it loads under its CSP with no violation, an event arrives on
# GET /v1/events; and what must be refused is: a rebinding or foreign Host and Origin, another site's or port's request, an
# extension's, a WebSocket with no Origin, and a call with neither a label nor the bearer (design/calcifer-access.md). Then the page as a person uses it: Place (web/place.js), twenty pictures dropped at once (web/dropped.js), one stream across tabs and a
# `kill -9` of Calcifer (web/reconnect.js), edits with Calcifer stopped that land when it's back (web/offline.js), undo and redo of the project's history from the page and the CLI (web/undo.js), the Runs and Machines drawers with a real run from the CLI where this computer runs
# models (web/drawers.js), what each machine is doing (web/activity.js), sign-in through AuthKit's form (web/signin.js), any project as live boxes (web/see.js), zoom from the canvas's corner (web/zoom.js), the background and dots at every zoom (web/canvas.js), how every box and what's around the canvas are laid out (web/layout.js), assets as a person works them (web/nodes.js), a render this computer would do poorly asked first (web/localfit.js), one Models & LoRAs picker for every tool (web/models.js), a box at rest as Paul drew it (web/tiles.js), versions by name and a tool on a box (web/versions.js), right click Download (web/download.js), a starred asset first in Files (web/star.js), boxes copied, pasted and duplicated (web/copy.js), a click selects and a double click opens, Delete deletes (web/select.js), a box from its tool's fields (web/tools.js), a field that applies only while another has a value (web/when.js), a shot's segments played as they land (web/segments.js), a tool on a box of the kind it makes (web/kinds.js), deleting by a trash can (web/trash.js), a take's segments redone and another box's take continued (web/scrub.js), a pulled shape snapping to a ratio its tool makes (web/aspect.js), the canvas where it was after Focus (web/refocus.js), no New on an empty box (web/empty.js), the right-side panel closed by a click off it (web/drawer.js), the tool under a box (web/toolword.js), a mention its tool can't use crossed out (web/unused.js), typed words that read over a picture (web/legible.js), a Shot's beats laid out (web/beats.js), pasted mentions marked as typed and loaded (web/paste.js), a press from inside never dismissing (web/dismiss.js), every tool the Hub lists in the palette (web/palette.js), an orbit video turned (web/orbit.js), the video editor: a full-size player over a timeline, joined into versions as the cut changes (web/timeline.js),
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

# The merge check keeps functional journeys here; layout runs in parallel and scale is an explicit benchmark.
web_core() { web_serves core; }
web_native() { web_serves native; }
web_firefox() { web_serves firefox; }

web_serves() {
step "web-serves: the page as its own, under its CSP, an event, and what is refused"
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
step "web-zoom: − the zoom + in the canvas's corner, its menu's fit and 100%, none over Focus, drawn at the canvas's scale, and 300 boxes panned"
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
step "web-models: one Models & LoRAs picker for H3 and an image: H3's Base Model once, a LoRA picked, its strength, a second model replacing the first, its triggers; a VAE slot of its family (cl-934, se-131)"
web_proof models "$chrome" "$pw" ''
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
step "web-videotiles: every video box at rest plays its preview: an imported file, an Edit's cut, a tool's take and one picked up, an H3 take, one with no preview yet, one played in Focus (cl-964)"
web_proof videotiles "$chrome" "$pw" ''
step "web-when: a field that applies only while another has a value (When): cozy run says it's ignored and its function gets the default; in Focus it's dulled at once, kept, and back"
web_proof when "$chrome" "$pw" ''
step "web-kinds: a tool goes on a box of the kind it makes: a video tool refused on a picture; a video tool on a picture and an image tool on a video change nothing and say why at the bottom left; BiRefNet on it starts from <self.1>"
web_proof kinds "$chrome" "$pw" ''
step "web-refocus: leaving Focus by Esc, its close, Back, a click off, after zooming its picture, by way of a second box, while a scroll or pinch glides, reloaded and by Forward: the canvas exactly where it was"
web_proof refocus "$chrome" "$pw" ''
step "web-aspect: a corner pulled to 1237:689 lands on 16:9, on the canvas and in Focus"
web_proof aspect "$chrome" "$pw" ''
# Its renders need this computer's machine: Linux's.
if [ -n "$LOCAL" ]; then
  step "web-ratios: a box says its shape as film and media do: a 16:9 take of 1344x768 16:9, a 1.37:1 take of 1024x768 1.37:1, files their pixels' snapped, a corner near 4:3 4:3"
  web_proof ratios "$chrome" "$pw" ''
fi
# Its renders need this computer's machine: Linux's.
if [ -n "$LOCAL" ]; then
  step "web-frame: H3's Shot in Focus: Summary and Beats as sections in the frame, behind it what each mentions; New a draft; Render and Cmd/Ctrl+Enter render; Render where an image's is"
  web_proof frame "$chrome" "$pw" ''
fi
step "web-scrub: a take's segments under its player, Redo from one, another box continuing it from its end; an empty box with no tool asks nothing"
web_proof scrub "$chrome" "$pw" ''
step "web-agent: the agent API: nodes listed, and read whole as a file with every default, their takes and revs; explain in words and as JSON Schema (cl-956)"
web_proof agent "$chrome" "$pw" ''
step "web-trash: a take's discard and a box's delete are trash cans, Undo brings a take back, and Focus's close is an X"
web_proof trash "$chrome" "$pw" ''
step "web-toolword: a box with a tool says it under its picture (cl-932), readable and shrinking with the box; one with none says nothing"
web_proof toolword "$chrome" "$pw" ''
# Its run needs this computer's machine: Linux's.
if [ -n "$LOCAL" ]; then
  step "web-unused: a mention its tool can't use is crossed out, saying why, and never sent: MOSS Image, Qwen-Image-2 and H3; cozy status and run watch say it once (cl-944)"
  web_proof unused "$chrome" "$pw" ''
  step "web-ignored: past the binder, the Runtime keeps what a prompt can take and leaves the rest out as names, one ignored row each; run watch says each once (cl-944)"
  web_proof ignored "$chrome" "$pw" ''
fi
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
step "web-timeline: the video editor: three clips in, reorder, trim, split, delete with Undo, the player live, each change joined free into a frame-exact version"
# Real clips, made and counted by the ffmpeg Calcifer fetched for itself for thumbnails: one on a runner's PATH would keep it from fetching.
local own; own=$(find "$HOME/.cozy/tools/ffmpeg" -name 'ffprobe*' 2> /dev/null | head -1)
PATH=${own:+$(dirname "$own"):}$PATH web_proof timeline "$chrome" "$pw" ''
step "web-live: the real CLI and another writer's saves on an open canvas: each within a second, what's typed kept, 200 edits, the stream fallen behind"
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
# $3, a command, runs while it's stopped.
web_undo_window() {
  local config=$HOME/.cozy/config.toml pid
  awk -v n="$2" '/^undo_seconds *=/ { next } { print } /^\[calcifer\]/ && n != "" { print "undo_seconds = " n; put = 1 } END { if (n != "" && !put) printf "\n[calcifer]\nundo_seconds = %s\n", n }' "$config" > "$config.next" && mv "$config.next" "$config"
  local waited=0
  pid=$(jq -r .pid < "$1")
  kill -TERM "$pid" 2> /dev/null || true
  # One still running 120 s on is a failure, not a hang.
  while kill -0 "$pid" 2> /dev/null; do
    [ "$waited" -lt 600 ] || { check "Calcifer $pid still runs 120 s after SIGTERM"; kill -9 "$pid" 2> /dev/null; break; }
    sleep 0.2; waited=$((waited + 1))
  done
  [ -z "${3:-}" ] || "$3"
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
  # A new project of two boxes.
  jq -n '[{name: "stays", tool: "cozy/edit"}, {name: "goes", tool: "cozy/edit"}]' | boxes_in "$T" || return
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

# A command that kills this Calcifer and brings it back, as the page finds it: at its own port, with nothing to sign in again. `cozy status` only
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
  # What each failed check saw first: the rest of what it saw is cut short.
  [ "$code" = 0 ] || check "web-$pass: failed $(jq -c .failures "$json" 2> /dev/null): $(jq -c '(.failures // []) as $f | .seen | with_entries(select(.key as $k | $f | index($k)))' "$json" 2> /dev/null | head -c 2000) $(jq -c '.seen | with_entries(select(.value != true))' "$json" 2> /dev/null | head -c 800) $(tail -c 500 "$OUT/web-$pass.err")"
  # What died on another Hub doesn't leave the next proof on it.
  [ "$code" = 0 ] || [ -z "$restart" ] || bash -c "$restart" || true
  folder=$(jq -r '.seen.project.folder // empty' "$json" 2> /dev/null)
  # A project's machine storage (`.vault-machine`) may still be settling a canceled run: its leftovers never end the suite.
  case $folder in */web-*) rm -rf "$folder" 2> /dev/null || { sleep 5; rm -rf "$folder"; } || true ;; esac
}

# web_serves's own, on its project: the page with nothing minted or kept, and each request the access rule refuses
# (design/calcifer-access.md). The second OS account, a browser's cross-site requests, framing, restarts and dev mode are web/access.js's.
web_signed_in() {
local code=0 port=${addr##*:} file mode
HOLDER=$(web_path "$lock") ARGV_LOG=$(web_path "$OUT/argv.log") CHROME=$chrome PLAYWRIGHT=$(web_path "$pw/node_modules/playwright-core") COZY=$(web_path "$COZY") \
  node "$(web_path "$here/web/serves.js")" "$id" "$(web_path "$P")" "$(web_path "$pic")" > "$OUT/web.json" 2> "$OUT/web.err" || code=$?
[ "$code" = 0 ] || check "web-serves: the browser failed: $(tail -c 500 "$OUT/web.err")"
J '(.minted | length) == 0 and (.authorization | length) == 0 and .gate == 0' "$OUT/web.json" \
  || check "web-serves: the page asked for or sent a credential, or wasn't answered: $(jq -c '{minted, authorization, gate}' "$OUT/web.json")"

J '.document.status == 200 and .document.referrer_policy == "no-referrer"' "$OUT/web.json" || check "web-serves: GET /: $(jq -c .document "$OUT/web.json")"
for directive in "default-src 'none'" "script-src 'self' 'wasm-unsafe-eval'" "style-src 'self'" "img-src 'self' blob: data:" "media-src 'self' blob:" "font-src 'self'" "connect-src 'self'" "base-uri 'none'" "form-action 'none'" "frame-ancestors 'none'"; do
  J --arg d "$directive" '.document.csp // "" | contains($d)' "$OUT/web.json" || check "web-serves: the page's CSP has no $directive: $(jq -c .document.csp "$OUT/web.json")"
done
J ".document.csp // \"\" | contains(\"'unsafe-eval'\") | not" "$OUT/web.json" || check "web-serves: the page's CSP allows eval: $(jq -c .document.csp "$OUT/web.json")"
J '(.violations | length) == 0 and (.page_errors | length) == 0' "$OUT/web.json" || check "web-serves: the page broke under its CSP: $(jq -c '{violations, page_errors}' "$OUT/web.json")"
J '.console_errors | length == 0' "$OUT/web.json" || check "web-serves: the page's console has errors, such as a route Calcifer lacks: $(jq -c .console_errors "$OUT/web.json")"
J '.document.set_cookie == null and (.cookies | length) == 0 and (.stored | length) == 0' "$OUT/web.json" || check "web-serves: the browser keeps something of Calcifer's: $(jq -c '{document, cookies, stored}' "$OUT/web.json")"
J '(.assets | length) > 0 and (.assets | all(.status == 200 and (.cache_control // "" | contains("no-store"))))' "$OUT/web.json" || check "web-serves: /assets/* is not 200 and no-store: $(jq -c .assets "$OUT/web.json")"
J '.page_stream.status == 200 and (.page_stream.content_type // "" | startswith("text/event-stream")) and .page_stream.authorization == null and .page_stream.site == "same-origin"' "$OUT/web.json" \
  || check "web-serves: the page did not open GET /v1/events as its own page: $(jq -c .page_stream "$OUT/web.json")"
J --arg id "$id" '.stream_status == 200 and .streamed[0].type == "hello" and (.streamed | any(.type == "project.changed" and .id == $id))' "$OUT/web.json" \
  || check "web-serves: no hello then project.changed on GET /v1/events: $(jq -c '{stream_status, streamed}' "$OUT/web.json")"
if [ "${RUNNER_OS:-}" = Windows ]; then
  J '.storage.version == 3 and (.storage.principal | startswith("user:S-1-"))' "$OUT/web.json" || check "web-serves: project storage isn't this account's: $(jq -c .storage "$OUT/web.json")"
else
  J --arg p "user:$(id -u)" '.storage.version == 3 and .storage.principal == $p' "$OUT/web.json" || check "web-serves: project storage isn't this account's: $(jq -c .storage "$OUT/web.json")"
fi
J '.typed.status == 403 and .typed.site == "none" and .typed.page == 200' "$OUT/web.json" || check "web-serves: a /v1 URL opened in the address bar isn't refused, or the page's is: $(jq -c .typed "$OUT/web.json")"

# What isn't Calcifer's own page, or this computer's cozy with its bearer, is refused in one line.
local B=(-H "Authorization: Bearer $token") page=(-H 'Sec-Fetch-Site: same-origin' -H 'Sec-Fetch-Mode: cors') ws=(-H 'Connection: Upgrade' -H 'Upgrade: websocket' -H 'Sec-WebSocket-Version: 13' -H 'Sec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==')
answers "the CLI's bearer" 200 - "${B[@]}" "$base/v1/status"
answers "the listener's other name, localhost" 200 - "${B[@]}" -H "Host: localhost:$port" "$base/v1/status"
answers "a loopback Host on another port" 403 host_not_allowed "${B[@]}" -H 'Host: localhost:1' "$base/v1/status"
answers "a rebinding Host on the page" 403 host_not_allowed -H "Host: 127.0.0.1.nip.io:$port" "$base/"
answers "a rebinding Host on /v1, with the bearer" 403 host_not_allowed "${B[@]}" -H "Host: 127.0.0.1.nip.io:$port" "$base/v1/status"
answers "a rebinding page's own request" 403 host_not_allowed "${page[@]}" -H "Host: 127.0.0.1.nip.io:$port" -H "Origin: http://127.0.0.1.nip.io:$port" "$base/v1/status"
answers "a foreign Origin" 403 origin_not_allowed "${B[@]}" -H 'Origin: http://evil.example' "$base/v1/status"
answers "another port's Origin" 403 origin_not_allowed "${B[@]}" -H 'Origin: http://127.0.0.1:1' "$base/v1/status"
answers "the page's own request" 200 - "${page[@]}" -H "Origin: $base" "$base/v1/status"
answers "another site's request" 403 not_own_page -H 'Sec-Fetch-Site: cross-site' -H 'Sec-Fetch-Mode: no-cors' "$base/v1/status"
answers "another site's request, with the bearer" 403 not_own_page "${B[@]}" -H 'Sec-Fetch-Site: cross-site' -H 'Sec-Fetch-Mode: cors' "$base/v1/status"
answers "another port's page (same-site)" 403 not_own_page -H 'Sec-Fetch-Site: same-site' -H 'Sec-Fetch-Mode: cors' "$base/v1/status"
answers "an extension's fetch (none, not a navigation)" 403 not_own_page -H 'Sec-Fetch-Site: none' -H 'Sec-Fetch-Mode: cors' "$base/v1/status"
# A URL typed or opened by another program (an agent's browser told to go there too) opens the page, never /v1.
answers "a URL opened: the page" 200 - -H 'Sec-Fetch-Site: none' -H 'Sec-Fetch-Mode: navigate' -H 'Sec-Fetch-Dest: document' "$base/"
answers "a URL opened: /v1 data" 403 not_own_page -H 'Sec-Fetch-Site: none' -H 'Sec-Fetch-Mode: navigate' -H 'Sec-Fetch-Dest: document' "$base/v1/projects/$id?assets=false"
answers "a URL opened: a run's output" 403 not_own_page -H 'Sec-Fetch-Site: none' -H 'Sec-Fetch-Mode: navigate' -H 'Sec-Fetch-Dest: document' "$base/v1/runs/run-0/outputs/image"
answers "a URL opened that writes" 403 not_own_page -X POST -H 'Sec-Fetch-Site: none' -H 'Sec-Fetch-Mode: navigate' -H 'Content-Type: application/json' -d '{}' "$base/v1/run-estimates"
answers "neither a label nor the bearer" 401 unauthenticated "$base/v1/machines"
answers "a wrong bearer" 401 unauthenticated -H 'Authorization: Bearer 00' "$base/v1/machines"
answers "a wrong bearer from the page" 401 unauthenticated "${page[@]}" -H 'Authorization: Bearer 00' "$base/v1/machines"
answers "a WebSocket with no Origin" 403 origin_not_allowed "${ws[@]}" "$base/__cozy_hmr"
answers "a WebSocket from another site" 403 origin_not_allowed "${ws[@]}" -H 'Origin: http://localhost:1' "$base/__cozy_hmr"

# Media as the page's <img> asks for it: no header of its own, no secret in the URL.
file=$(jq -r '.imported.path // empty' "$OUT/web.json"); file=${file##*/}
if [ -n "$file" ]; then
  local img=(-H 'Sec-Fetch-Mode: no-cors' -H 'Sec-Fetch-Dest: image')
  answers "the page's <img> of a file" 200 - -H 'Sec-Fetch-Site: same-origin' "${img[@]}" "$base/v1/projects/$id/files/$file"
  answers "the page's HEAD of a file" 200 - -I -H 'Sec-Fetch-Site: same-origin' "${img[@]}" "$base/v1/projects/$id/files/$file"
  answers "another site's <img> of a file" 403 not_own_page -H 'Sec-Fetch-Site: cross-site' "${img[@]}" "$base/v1/projects/$id/files/$file"
  answers "another port's <img> of a file" 403 not_own_page -H 'Sec-Fetch-Site: same-site' "${img[@]}" "$base/v1/projects/$id/files/$file"
  answers "the CLI's read of a file" 200 - "${B[@]}" -H 'cozy-project-storage: 3' "$base/v1/projects/$id/files/$file"
  answers "a file read with neither" 401 unauthenticated "$base/v1/projects/$id/files/$file"
else
  check "web-serves: the import named no file: $(jq -c .imported "$OUT/web.json")"
fi

# The bearer lives in the 0600 lock record alone: no file route serves it, and it's in neither Calcifer's argv nor its environment.
local route
for route in "/assets/..%2F..%2Fcalcifer.lock.holder" "/v1/projects/$id/files/..%2F..%2F..%2F.cozy%2Fcalcifer%2Fcalcifer.lock.holder" "/@fs$HOME/.cozy/calcifer/calcifer.lock.holder"; do
  curl -sS -o "$OUT/web-reply.json" -H 'Sec-Fetch-Site: same-origin' -H 'Sec-Fetch-Mode: no-cors' "$base$route" || true
  ! grep -qF "$token" "$OUT/web-reply.json" || check "web-serves: $route served the bearer"
done
case ${RUNNER_OS:-$(uname -s)} in
  Linux) mode=$(stat -c %a "$lock") ;;
  macOS | Darwin) mode=$(stat -f %Lp "$lock") ;;
  *) mode=600 ;;
esac
[ "$mode" = 600 ] || check "web-serves: the lock record is mode $mode, not 600"
if [ -r "/proc/$(jq -r .pid < "$lock")/environ" ]; then
  ! tr '\0' '\n' < "/proc/$(jq -r .pid < "$lock")/cmdline" | grep -qF "$token" || check "web-serves: the bearer is in Calcifer's argv"
  ! tr '\0' '\n' < "/proc/$(jq -r .pid < "$lock")/environ" | grep -qF "$token" || check "web-serves: the bearer is in Calcifer's environment"
fi
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
