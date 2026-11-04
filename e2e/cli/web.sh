# e2e/cli/web.sh: the `web-serves` cell, the part cli/run.sh calls `web_serves`, on every OS, with no machine or model. `cozy web
# --no-open`; Chrome redeems its code (web/serves.js), the page loads under its CSP with no violation, an event arrives on
# GET /v1/events; and what must be refused is: a foreign Host and Origin, a used or expired code, ?media= past a file route, a
# call with no holder or session. Then the page as a person uses it: Place (web/place.js), one stream across tabs and a
# `kill -9` of Calcifer (web/reconnect.js), and the Runs and Machines drawers with a real run from the CLI where this computer runs
# models (web/drawers.js).
# Alone (`e2e/cli/web.sh --out DIR`) it drives this computer's cozy as it is, in the default home. It runs nothing, forgets the
# projects it made, and leaves Calcifer running: this computer's is shared.
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

web_serves() {
step "web-serves: sign in, the page under its CSP, an event, and what is refused"
local lock=$HOME/.cozy/calcifer/calcifer.lock.holder P=$OUT/web-project pic=$OUT/web-picture.png pw=$OUT/playwright
local addr token base chrome id
chrome=$(web_chrome)
[ -n "$chrome" ] && [ -x "$chrome" ] || { check "web-serves: no Google Chrome on this runner (${chrome:-none})"; return; }
npm install -s --prefix "$(web_path "$pw")" playwright-core@1.55.0 > "$OUT/web-npm.log" 2>&1 || { check "web-serves: npm install playwright-core failed: $(tail -c 300 "$OUT/web-npm.log")"; return; }
cozy project new "$P" > "$OUT/web-new.txt" 2>&1 || { check "web-serves: project new failed: $(tail -c 300 "$OUT/web-new.txt")"; return; }
id=$(cozy --json project -p "$P" status 2> /dev/null | jq -r '.id // empty')
[ -n "$id" ] || { check "web-serves: the project has no id"; return; }
addr=$(jq -r .address < "$lock") token=$(jq -r .token < "$lock") base=http://$addr
# One red pixel.
base64 -d > "$pic" <<< 'iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR4nGP4z8DwHwAFAAH/q842iQAAAABJRU5ErkJggg=='
web_signed_in
curl -sS -o /dev/null -X DELETE -H "Authorization: Bearer $token" "$base/v1/projects/$id" || true
step "web-place: dropped and chosen files on the canvas, a rename, a move, a reload"
web_proof place "$chrome" "$pw" ''
step "web-reconnect: one stream across tabs, and kill -9 of Calcifer with no reload"
local restart=''
[ -n "${WEB_ALONE:-}" ] || restart=$(web_restart "$lock")
web_proof reconnect "$chrome" "$pw" "$restart"
step "web-drawers: a real run live to its end and a cancel, one log stream across tabs, the machines"
web_proof drawers "$chrome" "$pw" ''
}

# A command that kills this Calcifer and brings it back, as the page finds it: at its own port, its sessions kept.
web_restart() {
  local kill="kill -9 \$(jq -r .pid < '$1')"
  [ "${RUNNER_OS:-}" != Windows ] || kill="taskkill //F //PID \$(jq -r .pid < '$1') > /dev/null"
  echo "$kill; sleep 1; '$COZY' status > /dev/null 2>&1"
}

# A fresh sign-in link in $link, or a failed check.
web_link() {
  cozy web --no-open > "$OUT/web-open.txt" 2>&1 || { check "web-serves: cozy web --no-open failed: $(tail -c 300 "$OUT/web-open.txt")"; return 1; }
  link=$(grep -oE 'http://[^[:space:]]+#code=[0-9a-f]+[^[:space:]]*' "$OUT/web-open.txt" | head -1)
  [ -n "$link" ] || { check "web-serves: cozy web --no-open printed no link: $(head -c 300 "$OUT/web-open.txt")"; return 1; }
}

# web_proof <place|reconnect|drawers> <chrome> <playwright's prefix> <restart command, or nothing>: web/<it>.js, on a project of its own by
# name, which it forgets, folder and all, whatever happened.
web_proof() {
  local name=$1 chrome=$2 pw=$3 restart=$4 json=$OUT/web-$1.json link code=0 id folder lock addr token
  web_link || return
  ALONE=${WEB_ALONE:-} RESTART=$restart SHOT=$(web_path "$OUT/web-$name") CHROME=$chrome PLAYWRIGHT=$(web_path "$pw/node_modules/playwright-core") COZY=$(web_path "$COZY") \
    node "$(web_path "$here/web/$name.js")" "$link" > "$json" 2> "$OUT/web-$name.err" || code=$?
  [ "$code" = 0 ] || check "web-$name: failed $(jq -c .failures "$json" 2> /dev/null): $(jq -c '.seen | with_entries(select(.value != true))' "$json" 2> /dev/null | head -c 800) $(tail -c 500 "$OUT/web-$name.err")"
  lock=$HOME/.cozy/calcifer/calcifer.lock.holder
  addr=$(jq -r .address "$lock" 2> /dev/null) token=$(jq -r .token "$lock" 2> /dev/null)
  id=$(jq -r '.seen.project.id // empty' "$json" 2> /dev/null) folder=$(jq -r '.seen.project.folder // empty' "$json" 2> /dev/null)
  [ -z "$id" ] || curl -sS -o /dev/null -X DELETE -H "Authorization: Bearer $token" "http://$addr/v1/projects/$id" || true
  case $folder in */.cozy/projects/web-*) rm -rf "$folder" ;; esac
}

# web_serves's own, on its project, which it forgets whatever happens here.
web_signed_in() {
local link code=0 wait expires
web_link || return

CHROME=$chrome PLAYWRIGHT=$(web_path "$pw/node_modules/playwright-core") COZY=$(web_path "$COZY") \
  node "$(web_path "$here/web/serves.js")" "$link" "$id" "$(web_path "$P")" "$(web_path "$pic")" > "$OUT/web.json" 2> "$OUT/web.err" || code=$?
[ "$code" = 0 ] || check "web-serves: the browser failed: $(tail -c 500 "$OUT/web.err")"
J '.session.status == 200 and (.session.token | length > 0) and (.session.media | length > 0)' "$OUT/web.json" \
  || { check "web-serves: the page did not redeem its code: $(jq -c .session "$OUT/web.json")"; return; }
local session media
session=$(jq -r .session.token "$OUT/web.json") media=$(jq -r .session.media "$OUT/web.json")

J '.document.status == 200 and .document.referrer_policy == "no-referrer"' "$OUT/web.json" || check "web-serves: GET /: $(jq -c .document "$OUT/web.json")"
for directive in "default-src 'none'" "script-src 'self' 'wasm-unsafe-eval'" "style-src 'self'" "img-src 'self' blob: data:" "media-src 'self' blob:" "connect-src 'self'" "base-uri 'none'" "form-action 'none'" "frame-ancestors 'none'"; do
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
answers "the holder asks for another code" 200 - -X POST -H "Authorization: Bearer $token" "$base/v1/session-codes"
fresh=$(jq -r '.code // empty' "$OUT/web-reply.json") expires=$(jq -r '.expires_ms // 0' "$OUT/web-reply.json")

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

# A code lives 60 s: this one, kept until then. Last, as nothing else waits on it.
wait=$(( expires / 1000 - $(date +%s) + 1 ))
[ "$wait" -le 61 ] && [ -n "$fresh" ] || check "web-serves: a fresh code '$fresh' expires in $wait s, not 60 s"
[ "$wait" -le 0 ] || sleep "$wait"
redeem "an expired code" 400 session_code_invalid "$fresh"
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
