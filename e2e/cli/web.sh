# e2e/cli/web.sh: the page in a browser. The web suite (cl-973): web/<cell>.spec.js, a lane of them at once on this job's Calcifer
# (web_suite). On every OS, `web-serves` (web_serves, cl-985): the page as Calcifer's own with nothing minted or kept (web/serves.js),
# what the access rule refuses (design/calcifer-access.md), and what the gate costs. By hand, `--shard web-<cell>` also runs a
# web/<cell>.js outside the suite (web_alone): look and names (pictures to review), scale and pageidle (cl-885's benchmarks), and access
# (a second OS account, framing, restarts and dev mode through Calcifer).
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

# web_suite <lane> <cell...> (cl-973): each cell web/<cell>.spec.js, several at once through Playwright Test (web/playwright.config.js)
# on this job's Calcifer, each on a project and a browser context of its own. Then one check a failed cell, and each cell's time and the
# 1-minute load as it ended. <lane>: 1, 2, 3 (Chrome), engines (Firefox's `@engines` cells, and `@vault`), solo (`@solo`, one at a time),
# native (one at a time, as it holds `@solo` tests), or a cell's own name (by hand: whichever project takes it, one at a time).
web_suite() {
  local lane=$1 pw=$OUT/playwright-test chrome cell engine code line web=$OUT/web xvfb=
  local projects=() workers=() filters=()
  shift
  [ $# -gt 0 ] || { echo "web-$lane: no cells written yet"; return 0; }
  step "web-$lane: $*"
  chrome=$(web_chrome)
  [ -n "$chrome" ] && [ -x "$chrome" ] || { check "web-$lane: no Google Chrome on this runner (${chrome:-none})"; return; }
  for cell in "$@"; do
    [ -f "$here/web/$cell.spec.js" ] || { check "web-$lane: no web/$cell.spec.js"; return; }
    # A file pattern is a regex on its path: anchored, `create` can't take every file under cozy-creator/.
    filters+=("[\\/]$cell\\.spec\\.js\$")
  done
  npm install -s --prefix "$(web_path "$pw")" @playwright/test@1.55.0 > "$OUT/web-npm.log" 2>&1 \
    || { check "web-$lane: npm install @playwright/test failed: $(tail -c 300 "$OUT/web-npm.log")"; return; }
  case $lane in
    1 | 2 | 3) projects=(chrome) ;;
    engines)
      projects=(firefox vault)
      # H.264 is the system's libavcodec on Linux: with none, Firefox says "This browser can't play it" (41966978063).
      bash "$here/install-browser-deps.sh" "$pw" > "$OUT/web-firefox-deps.log" 2>&1 \
        || { check "web-$lane: Firefox's dependencies failed: $(tail -c 300 "$OUT/web-firefox-deps.log")"; return; }
      # Headless Firefox makes no WebGL context on Linux, so the canvas can't start: it runs on a virtual display, with Mesa's GL (config).
      if [ "$(uname -s)" = Linux ] && command -v Xvfb > /dev/null; then
        Xvfb :97 -screen 0 1920x1080x24 -nolisten tcp > "$OUT/web-xvfb.log" 2>&1 & xvfb=$!
      fi ;;
    solo) projects=(solo) workers=(--workers=1) ;;
    # A computer with no GPU (run.sh sets WEB_RENDERER=2d): the browser left as it is, so the canvas draws in 2D (fixtures.js names software
    # WebGL as hardware elsewhere).
    2d) projects=(chrome) ;;
    2d-solo) projects=(solo) workers=(--workers=1) ;;
    # Native and by hand, one test at a time: a `@solo` test stops the Calcifer the others share.
    native) projects=(chrome solo) workers=(--workers=1); [ "${RUNNER_OS:-}" != macOS ] || projects+=(webkit) ;;
    *) projects=(chrome vault solo) workers=(--workers=1) ;;
  esac
  for engine in firefox webkit; do
    [[ " ${projects[*]} " != *" $engine "* ]] || node "$pw/node_modules/playwright-core/cli.js" install "$engine" > "$OUT/web-$engine-install.log" 2>&1 \
      || { check "web-$lane: installing $engine failed: $(tail -c 300 "$OUT/web-$engine-install.log")"; return; }
  done
  rm -rf "$web/results" "$web/report.json"
  mkdir -p "$web"
  # A busy computer's sockets, held while the cells run (web/busy.js; web-opens-busy, cl-985).
  local busy=
  if [ -n "${BUSY_SOCKETS:-}" ]; then
    ulimit -n 16384 2> /dev/null || true
    node "$(web_path "$here/web/busy.js")" "$BUSY_SOCKETS" > "$OUT/web-busy.out" 2>&1 & busy=$!
    until grep -q '^ready' "$OUT/web-busy.out" 2> /dev/null; do kill -0 "$busy" 2> /dev/null || { check "web-$lane: busy.js ended: $(tail -c 300 "$OUT/web-busy.out")"; return; }; sleep 0.2; done
  fi
  {
    [ -z "$xvfb" ] || export FIREFOX_DISPLAY=:97
    HOLDER=$(web_path "$HOME/.cozy/calcifer/calcifer.lock.holder") COZY=$(web_path "$COZY") CHROME=$chrome OUT=$(web_path "$OUT") ARGV_LOG=$(web_path "$OUT/argv.log") \
      NODE_PATH=$(web_path "$pw/node_modules") node "$(web_path "$pw/node_modules/@playwright/test/cli.js")" test -c "$(web_path "$here/web/playwright.config.js")" \
      "${projects[@]/#/--project=}" ${workers[@]+"${workers[@]}"} "${filters[@]}" 2>&1 && echo 0 > "$web/$lane.code" || echo $? > "$web/$lane.code"
  } | tee "$web/$lane.log"
  code=$(cat "$web/$lane.code")
  [ -z "$busy" ] || { kill "$busy" 2> /dev/null; wait "$busy" 2> /dev/null || true; }
  [ -z "$xvfb" ] || { kill "$xvfb" 2> /dev/null; wait "$xvfb" 2> /dev/null || true; }
  # A trace is a zip, which e2e/publish's scrub can't read: unpacked, its bearers and codes are scrubbed like any text.
  find "$web/results" -name trace.zip 2> /dev/null | while read -r trace; do
    python3 -m zipfile -e "$trace" "${trace%.zip}" && rm -f "$trace"
  done || true
  [ -s "$web/report.json" ] || { check "web-$lane: Playwright wrote no report (exit $code): $(tail -c 600 "$web/$lane.log")"; return; }
  node "$(web_path "$here/web/report.js")" "$(web_path "$web/report.json")" "$@" > "$web/$lane.said" 2>&1 \
    || { check "web-$lane: its report doesn't read: $(tail -c 600 "$web/$lane.said")"; return; }
  local failed=0
  while IFS= read -r line; do
    case $line in
      FAILED\ *) check "${line#FAILED }"; failed=1 ;;
      *) echo "$line" ;;
    esac
  done < "$web/$lane.said"
  [ "$code" = 0 ] || [ "$failed" = 1 ] || check "web-$lane: Playwright exited $code with no cell failed: $(tail -c 600 "$web/$lane.log")"
  echo "web-$lane: load now $(cut -d' ' -f1-3 /proc/loadavg 2> /dev/null)"
}

# web_alone [cell]: web/<cell>.js ($WEB_ALONE_CELL), by hand, the page opened as its own (page.js: signIn) on a project of its own,
# whose folder it deletes whatever happened. $RESTART kills this Calcifer and brings it back.
web_alone() {
  local name=${1:-$WEB_ALONE_CELL} pw=$OUT/playwright chrome json code=0 folder lock=$HOME/.cozy/calcifer/calcifer.lock.holder
  step "web-$name, by hand"
  chrome=$(web_chrome)
  [ -n "$chrome" ] && [ -x "$chrome" ] || { check "web-$name: no Google Chrome on this runner (${chrome:-none})"; return; }
  npm install -s --prefix "$(web_path "$pw")" playwright-core@1.55.0 > "$OUT/web-npm.log" 2>&1 \
    || { check "web-$name: npm install playwright-core failed: $(tail -c 300 "$OUT/web-npm.log")"; return; }
  json=$OUT/web-$name.json
  RESTART=$(web_restart "$lock") HOLDER=$(web_path "$lock") ARGV_LOG=$(web_path "$OUT/argv.log") LAST_LOOK=$(web_path "$OUT/web-$name") SHOT=$(web_path "$OUT/web-$name") \
    CHROME=$chrome PLAYWRIGHT=$(web_path "$pw/node_modules/playwright-core") COZY=$(web_path "$COZY") \
    node "$(web_path "$here/web/$name.js")" > "$json" 2> "$OUT/web-$name.err" || code=$?
  [ "$code" = 0 ] || check "web-$name: failed $(jq -c .failures "$json" 2> /dev/null): $(jq -c '(.failures // []) as $f | .seen | with_entries(select(.key as $k | $f | index($k)))' "$json" 2> /dev/null | head -c 2000) $(tail -c 500 "$OUT/web-$name.err")"
  folder=$(jq -r '.seen.project.folder // empty' "$json" 2> /dev/null || true)
  case $folder in */web-*) drop_folder "$folder" ;; esac
}

# answers <what> <status, a regex> <error code, or -> <curl args…>
answers() {
  local what=$1 want=$2 code=$3 got; shift 3
  got=$(curl -sS -o "$OUT/web-reply.json" -w '%{http_code}' "$@") || got="no answer"
  { grep -Eqx "$want" <<< "$got" && { [ "$code" = - ] || J --arg c "$code" '.error.code == $c' "$OUT/web-reply.json"; }; } \
    || check "web-serves: $what: $got, not $want ${code/#-/}: $(head -c 300 "$OUT/web-reply.json")"
}

# web_serves (cl-985): the page as Calcifer's own, under its CSP, an event, and what is refused, on a project of its own.
web_serves() {
step "web-serves: the page as its own, under its CSP, an event, and what is refused"
local lock=$HOME/.cozy/calcifer/calcifer.lock.holder P=$OUT/web-project pic=$OUT/web-picture.png pw=$OUT/playwright
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
J '(.proofs | length) == 0' "$OUT/web.json" || check "web-serves: the page asked Calcifer for a storage proof first (cl-1066): $(jq -c .proofs "$OUT/web.json")"

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
# A sandboxed frame with no same-origin: an opaque origin, `Origin: null`, cross-site.
answers "an opaque origin (a sandboxed frame)" 403 origin_not_allowed -H 'Origin: null' -H 'Sec-Fetch-Site: cross-site' -H 'Sec-Fetch-Mode: cors' "$base/v1/status"
answers "an opaque origin's <img>" 403 not_own_page -H 'Sec-Fetch-Site: cross-site' -H 'Sec-Fetch-Mode: no-cors' -H 'Sec-Fetch-Dest: image' "$base/v1/status"
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
  answers "the CLI's read of a file" 200 - "${B[@]}" "$base/v1/projects/$id/files/$file"
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
web_gate_cost
}

# What the gate costs (Paul, 11-14), from Calcifer's own Server-Timing ([calcifer] diagnostics): per request, its checks on a kept
# connection (guard, admit), and per connection, the peer lookup; idle, then with 6,000 other loopback sockets open in a newer process
# (web/gate.js, web/busy.js). Linux also says what reading /proc/net/tcp{,6} costs, the scan sock_diag spares. Only a missing number fails.
web_gate_cost() {
local one i rows
ulimit -n 16384 2> /dev/null || true
node "$(web_path "$here/web/gate.js")" "$base" "$token" 3000 > "$OUT/web-gate.json" 2> "$OUT/web-gate.err" \
  || { check "web-gate: gate.js failed: $(tail -c 300 "$OUT/web-gate.err")"; return; }
if [ -r /proc/net/tcp ]; then
  one=$(date +%s%N); for rows in $(seq 20); do cat /proc/net/tcp /proc/net/tcp6 > /dev/null; done
  rows=$(cat /proc/net/tcp /proc/net/tcp6 | wc -l)
  jq --argjson us "$(( ($(date +%s%N) - one) / 20000 ))" --argjson rows "$rows" '. + {proc_read_us_idle: $us, proc_rows_idle: $rows}' "$OUT/web-gate.json" > "$OUT/web-gate.next" && mv "$OUT/web-gate.next" "$OUT/web-gate.json"
  node "$(web_path "$here/web/busy.js")" 3000 > "$OUT/web-busy.out" 2>&1 & i=$!
  until grep -q '^ready' "$OUT/web-busy.out" 2> /dev/null; do kill -0 "$i" 2> /dev/null || break; sleep 0.2; done
  one=$(date +%s%N); for rows in $(seq 20); do cat /proc/net/tcp /proc/net/tcp6 > /dev/null; done
  rows=$(cat /proc/net/tcp /proc/net/tcp6 | wc -l)
  jq --argjson us "$(( ($(date +%s%N) - one) / 20000 ))" --argjson rows "$rows" '. + {proc_read_us_busy: $us, proc_rows_busy: $rows}' "$OUT/web-gate.json" > "$OUT/web-gate.next" && mv "$OUT/web-gate.next" "$OUT/web-gate.json"
  kill "$i" 2> /dev/null || true; wait "$i" 2> /dev/null || true
fi
J '.idle.lookup_ms and .busy.lookup_ms and .idle.admit_ms and .busy.admit_ms' "$OUT/web-gate.json" || check "web-gate: Calcifer said no Server-Timing: $(jq -c . "$OUT/web-gate.json")"
echo "web-gate: $(jq -c . "$OUT/web-gate.json")"
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
web_start() {
  local hub=${1:-${COZY_HUB_URL:-}}
  echo "${hub:+COZY_HUB_URL=$hub }'$COZY' machine ls > /dev/null 2>&1"
}
