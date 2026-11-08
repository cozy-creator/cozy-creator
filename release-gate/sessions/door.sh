# door (cl-749, cl-774 Part D): a machine's own Calcifer, and the door from this computer's Calcifer to it.
# Before any rental, at $0, whichever `rent` says: a second user on this runner is an added machine, authorized and added, so the
# real door, TensorD's forward and that user's Calcifer all run here:
#   the door's refusals; SSE; a run's real outputs; keys not admitted and a play link's; a package's component, which the page never asks the
#   machine for; a hostile holder (fixtures/door) in curl and in a real Chrome, a module from it not running on this origin, whose control
#   shows the harness can see a script run; no secret of this computer reaching
#   the machine; a holder that's gone answering 503; a read after a restart's new token; TensorD starting the Calcifer of an
#   added machine when none runs; the archive; an upload; 1 GiB read through the door by a slow reader, memory flat.
# Then, with `rent`, one cpu pod with SSH on the session's cozy, sent from this runner with `--tensord`:
#   part idle     all of the above through a rental's door, and what only a rental has: its TensorD keeps one Calcifer, across
#                 kill -9, an update's restart and a crash of its service; `status.rental`; End refused while a job runs;
#                 40 s of SSE moving no deadline; an upload moving it. Then 1 GiB read at a throttle of `kib_s`, longer
#                 than the idle window, the pod not released under it; then an SSE stream and JSON polled every 5 s until
#                 the pod releases itself, its deadline not moving, within the window of the last bytes.
#   part release  the boot check, then End through the door: the pod's Calcifer relays, and the Hub ends it.
# Each ending ends its pod, so each part is its own session. A pod's boot on a released image hasn't this TensorD, so "at boot"
# here is its activation, which is the same start. params:
#   part       idle (the default) or release
#   kib_s      the slow read's throttle on a pod, KiB/s (default 1000: 1 GiB takes 17.9 min)
#   rehearse   false skips the $0 rehearsal (default true for idle, false for release)
. "$(dirname "$0")/lib.sh"
PART=$(param part idle) KIB_S=$(param kib_s 1000)
case $PART in idle) DEFAULT=true ;; release) DEFAULT=false ;; *) fail "part is idle or release, not $PART" ;; esac
REHEARSE=$(param rehearse "$DEFAULT")
FIX=$(cd "$(dirname "$0")/fixtures/door" && pwd) || fail "no fixtures"
J() { jq -e "$@" > /dev/null; }
# A check that fails is reported and the session goes on, so one attempt says everything. `fail` is for what can't go on.
check() { say "FAIL: $*"; echo "::error::$*"; RC=1; }
mark() { say "$*"; echo "::notice::$*"; }
for tool in jq node npm openssl google-chrome curl; do command -v "$tool" > /dev/null || fail "no $tool"; done
[ -f ~/.ssh/id_ed25519.pub ] || { mkdir -p ~/.ssh && ssh-keygen -q -t ed25519 -N '' -f ~/.ssh/id_ed25519; } || fail "no SSH key"
RENT_ARGS=--ssh
lock=$HOME/.cozy/calcifer/calcifer.lock.holder
ADDR=$(jq -r .address < "$lock") TOKEN=$(jq -r .token < "$lock")
[ -n "$TOKEN" ] || fail "this runner's Calcifer has no record"
BASE=http://$ADDR bearer=(-H "Authorization: Bearer $TOKEN")
api() { curl -sS --max-time 120 "${bearer[@]}" "$@"; }
# answers <what> <status, a regex> <error code or cause, or -> <curl args…>
answers() {
  local what=$1 want=$2 code=$3 got; shift 3
  got=$(curl -sS --max-time 120 -o "$OUT/reply.json" -w '%{http_code}' "$@") || got="no answer"
  { grep -Eqx "$want" <<< "$got" && { [ "$code" = - ] || J --arg c "$code" '.error.code == $c or .error.cause == $c' "$OUT/reply.json"; }; } \
    || check "$what: $got, not $want ${code/#-/}: $(head -c 300 "$OUT/reply.json")"
}
# The machine: X by name here; `pod` runs a shell there, `pod_in` with stdin, as its own user with home THERE.
RENTAL=0 U=cozydoor
pod() { timeout "${T:-300}" sudo -u "$U" -H env "PATH=/opt/$U:$PATH" bash -c "cd; $1" < /dev/null 2>> "$OUT/pod.err"; }
pod_in() { timeout "${T:-300}" sudo -u "$U" -H env "PATH=/opt/$U:$PATH" bash -c "cd; $1" 2>> "$OUT/pod.err"; }
THERE=/home/$U

# Each Calcifer or TensorD this user runs, by /proc: an image promises no procps.
PIDS='for p in /proc/[0-9]*; do [ "$(stat -c %u $p 2>/dev/null)" = "$(id -u)" ] && [ "$(cat $p/comm 2>/dev/null)" = "$WHO" ] && basename $p; done | sort -n | xargs echo'
calcifers() { pod "WHO=calcifer; $PIDS"; }
tensords() { pod "WHO=tensord; $PIDS"; }
holder_pid() { pod "grep -o '\"pid\":[0-9]*' $HOLDER | cut -d: -f2"; }
# one_calcifer <what>: exactly one, the one its record names, in CALCIFER.
one_calcifer() {
  local pids held
  for _ in $(seq 30); do
    pids=$(calcifers) held=$(holder_pid)
    [ -n "$pids" ] && [ "$pids" = "$held" ] && break
    sleep 1
  done
  [ -n "$pids" ] && [ "$pids" = "$held" ] || check "$1: Calcifers '$pids', the record names '$held'"
  CALCIFER=$pids
}
# reads <what>: the door answers a read, within 30 s of a restart.
reads() {
  local got
  for _ in $(seq 30); do
    got=$(curl -sS --max-time 30 -o "$OUT/reply.json" -w '%{http_code}' "${bearer[@]}" "$D/v1/status")
    [ "$got" = 200 ] && return 0
    sleep 1
  done
  check "$1: the door answers $got: $(head -c 300 "$OUT/reply.json")"
}
machine() { cozy machine show "$X" --json 2>/dev/null; }

door_refusals() {
  mark "the door's refusals, $X"
  answers "GET account through the door" 200 - "${bearer[@]}" "$D/v1/account"
  answers "POST account/codes" 403 calcifer_path_refused "${bearer[@]}" -X POST -H 'Content-Type: application/json' -d '{"email":"nobody@example.com"}' "$D/v1/account/codes"
  answers "PUT account/key" 403 calcifer_path_refused "${bearer[@]}" -X PUT -d '{}' "$D/v1/account/key"
  answers "account/codes spelled %2F" 403 calcifer_path_refused "${bearer[@]}" -X POST -d '{}' "$D/v1/account%2Fcodes"
  answers "POST down" 403 calcifer_path_refused "${bearer[@]}" -X POST "$D/v1/down"
  answers "down spelled %64" 403 calcifer_path_refused "${bearer[@]}" -X POST "$D/v1/%64own"
  answers "a path climbing out" 403 calcifer_path_refused "${bearer[@]}" --path-as-is "$D/v1/../../keepalive"
  answers "a path climbing out, spelled %2e" 403 calcifer_path_refused "${bearer[@]}" --path-as-is "$D/v1/%2e%2e/%2e%2e/keepalive"
  answers "outside /v1/" 403 calcifer_path_refused "${bearer[@]}" "$D/other"
  answers "a trailing slash" 403 calcifer_path_refused "${bearer[@]}" "$D/v1/runs/"
  answers "the door to itself" 400 machine_is_here "${bearer[@]}" "$BASE/v1/machines/local/calcifer/v1/status"
  answers "the door with no bearer" 401 unauthenticated "$D/v1/status"
}

door_stream() {
  mark "SSE through the door, $X"
  local held=''
  [ "$RENTAL" = 0 ] || held=$(deadline)
  timeout 40 curl -sSN "${bearer[@]}" -H 'Accept: text/event-stream' "$D/v1/events" > "$OUT/events.txt" 2> "$OUT/events.err"
  grep -Eq '"type": ?"hello"' "$OUT/events.txt" || check "no hello on the stream through the door: $(head -c 300 "$OUT/events.txt")"
  grep -q '^:' "$OUT/events.txt" || check "no keep-alive comment in 40 s through the door"
  [ "$RENTAL" = 0 ] || [ "$(deadline)" = "$held" ] || check "40 s of a stream moved the idle deadline from $held to $(deadline)"
}

# A cap minted here: run-scope when a run is named, signed by a key that was never admitted unless a PEM is named.
mint() {
  local machine=$1 scope=$2 pem=${3:-} d payload public key; d=$(mktemp -d)
  [ -n "$pem" ] || { pem=$d/key.pem; openssl genpkey -algorithm ed25519 -out "$pem" 2> /dev/null; }
  public=$(openssl pkey -in "$pem" -pubout -outform DER | tail -c 32 | openssl dgst -sha256 -binary | head -c 16 | basenc --base64url | tr -d '=\n')
  payload=$(jq -cn --arg m "$machine" --argjson s "$scope" --arg k "$public" --argjson e "$(( $(date +%s) + 300 ))" '{m: $m} + $s + {e: $e, k: $k}')
  { printf 'cozy-capability/1\0'; printf '%s' "$payload"; } > "$d/signed"
  openssl pkeyutl -sign -inkey "$pem" -rawin -in "$d/signed" -out "$d/signature" 2> /dev/null
  printf '%s.%s' "$(printf '%s' "$payload" | basenc --base64url | tr -d '=\n')" "$(basenc --base64url < "$d/signature" | tr -d '=\n')"
  rm -rf "$d"
}

door_keys() {
  mark "keys: none, not admitted, a run's, $X"
  local id; id=$(machine | jq -r '.status.worker_id // empty')
  [ -n "$id" ] || check "$X says no worker id"
  answers "no key" 401 capability_absent -k "https://$API/v1/calcifer/v1/status"
  answers "a key not admitted" 401 capability_invalid -k -H "Authorization: Cozy-Cap $(mint "$id" '{"a":"machine"}')" "https://$API/v1/calcifer/v1/status"
}

door_runs() {
  mark "a run in a shell there, its real outputs through the door, $X"
  pod "mkdir -p $P/runs" ; put "$FIX/fixture.py" "$P/fixture.py"
  T=1200 pod "cozy project new $P/runs/p > /dev/null; cd $P/runs/p && cozy --json run -p $P/runs/p ../../fixture.py/hostile 'prompt=through the door $RANDOM' --on local" > "$OUT/hostile-run-$X.json"
  local c=$?; [ "$c" = 0 ] || check "a run in a shell there exited $c: $(tail -c 300 "$OUT/hostile-run-$X.json") $(tail -c 300 "$OUT/pod.err")"
  api "$D/v1/runs?limit=1" > "$OUT/runs.json" || check "GET runs through the door"
  RUN=$(jq -r '.runs[0].id // empty' "$OUT/runs.json")
  [ -n "$RUN" ] && J '.runs[0].status == "completed"' "$OUT/runs.json" || check "the door shows no finished run: $(head -c 300 "$OUT/runs.json")"
  api -D "$OUT/page.hdr" -o "$OUT/page.body" "$D/v1/runs/$RUN/outputs/page" || check "an HTML output through the door"
  grep -qi '^content-disposition: attachment' "$OUT/page.hdr" || check "a real HTML output through the door is no download: $(tr -d '\r' < "$OUT/page.hdr" | paste -sd' ')"
  grep -qi '^content-security-policy: sandbox' "$OUT/page.hdr" || check "a real HTML output through the door is not sandboxed"
  grep -qa 'data-ran' "$OUT/page.body" || check "the HTML output's bytes did not come through"
  api "$D/v1/runs/$RUN/play?output=page" > "$OUT/play.json" || check "GET play through the door"
  local cap; cap=$(jq -r '.token // empty' "$OUT/play.json")
  [ -n "$cap" ] || { check "play answered no token: $(head -c 300 "$OUT/play.json")"; return; }
  answers "a play link's cap at the door" 403 capability_scope -k -H "Authorization: Cozy-Cap $cap" "https://$API/v1/calcifer/v1/status"
  answers "a play link's cap on a write there" 403 capability_scope -k -H "Authorization: Cozy-Cap $cap" -X POST -d '{}' "https://$API/v1/calcifer/v1/projects"
}

# A package's component, looking at the machine's Calcifer through the door. The machine's word says its tool has one, as its root can make it
# say (fixtures/door/component.js), and the page asks this computer's Calcifer, never the machine, which has no such code: the box is the declared one.
door_component() {
  mark "a package's component through the door, $X"
  local a=$P/boxed id link
  pod "cozy project new $a > /dev/null" || check "project new"
  put "$FIX/../../../../e2e/cli/project/draw.py" "$a/draw.py"
  T=1200 pod "cd $a && cozy --json run -p $a ./draw.py/draw instructions=primed --on local" > /dev/null || check "a run of the tool there: $(tail -c 300 "$OUT/pod.err")"
  id=$(pod "cozy --json project -p $a status" | jq -r '.id // empty')
  [ -n "$id" ] || { check "the project has no id: $(tail -c 300 "$OUT/pod.err")"; return; }
  answers "a box of it through the door" 201 - "${bearer[@]}" -X POST -H 'Content-Type: application/json' -H "Idempotency-Key: door-component-$RANDOM" -d '{"tool":"./draw.py/draw","name":"drawn","at":{"x":0,"y":0,"w":300}}' "$D/v1/projects/$id/assets"
  link=$(cozy open --no-open 2>&1 | grep -oE 'http://[^[:space:]]+#code=[0-9a-f]+[^[:space:]]*' | head -1)
  [ -n "$link" ] || { check "cozy open --no-open printed no link"; return; }
  CHROME=$(command -v google-chrome) PLAYWRIGHT=$PW/node_modules/playwright-core timeout 300 node "$FIX/component.js" "$link" "$X" "$id" > "$OUT/component.json" 2> "$OUT/component.err" \
    || check "the component's browser: $(tail -c 500 "$OUT/component.err")"
  J '.declared == 1' "$OUT/component.json" || check "the box isn't the declared one: $(jq -c . "$OUT/component.json")"
  J '(.tool_ui | length > 0) and (.tool_ui | all(startswith("/v1/tools/")))' "$OUT/component.json" || check "the page asked the machine for a component, or nobody: $(jq -c .tool_ui "$OUT/component.json")"
  J '.violations == []' "$OUT/component.json" || check "the page broke its CSP: $(jq -c .violations "$OUT/component.json")"
}

# The stand-in names itself the home's holder, as a pod's root can; a read through the door then gets its worst.
door_hostile() {
  mark "a hostile holder, in curl and in a real Chrome, $X"
  local port elsewhere canary name named code session media session_token pw
  port=$(python3 -c 'import socket; s = socket.socket(); s.bind(("127.0.0.1", 0)); print(s.getsockname()[1])')
  mkdir -p "$OUT/control"
  printf '%s' "<!doctype html><title>control</title><script>document.documentElement.setAttribute('data-ran','yes');document.cookie='planted=1;path=/';fetch('/x');</script>" > "$OUT/control/index.html"
  # This computer's canary: a redirect the door followed would land here.
  timeout 900 python3 -m http.server --bind 127.0.0.1 "$port" --directory "$OUT/control" > "$OUT/canary.log" 2>&1 & canary=$!
  elsewhere=http://127.0.0.1:$port/redirected
  put "$FIX/hostile_calcifer.py" "$P/hostile_calcifer.py"
  pod "nohup python3 $P/hostile_calcifer.py 41999 $elsewhere $P/hostile.log 900 > $P/hostile.err 2>&1 & echo \$! > $P/hostile.pid"
  pod "for i in 1 2 3 4 5 6 7 8 9 10; do python3 -c 'import socket; socket.create_connection((\"127.0.0.1\", 41999))' 2>/dev/null && break; sleep 1; done"
  named='{"address":"127.0.0.1:41999","token":"hostile","pid":1,"version":"0","started_at_ms":0}'
  pod "printf '%s' '$named' > $HOLDER.new && mv $HOLDER.new $HOLDER" || check "naming the stand-in"
  # A body makes the door read the record again, and it keeps what it read.
  answers "the door reads the new holder" 200 - "${bearer[@]}" -X POST -H 'Content-Type: application/json' -d '{}' "$D/v1/door/arm"
  for name in page listed two json video svg none module redirect; do
    api -D "$OUT/door-$name.hdr" -o "$OUT/door-$name.body" "$D/v1/door/$name" || check "door $name"
    tr -d '\r' < "$OUT/door-$name.hdr" | tr A-Z a-z > "$OUT/door-$name.low"
    for gone in set-cookie location refresh clear-site-data access-control link www-authenticate; do
      grep -q "^$gone" "$OUT/door-$name.low" && check "$name: a pod's $gone came through: $(paste -sd' ' "$OUT/door-$name.low")"
    done
    grep -q '^content-security-policy: sandbox$' "$OUT/door-$name.low" || check "$name: not sandboxed, or the pod's CSP came through: $(paste -sd' ' "$OUT/door-$name.low")"
    grep -q '^x-content-type-options: nosniff' "$OUT/door-$name.low" || check "$name: no nosniff"
    grep -q '^cozy-said: kept' "$OUT/door-$name.low" || check "$name: Cozy-* did not pass"
  done
  for name in page listed two svg none module redirect; do
    grep -q '^content-disposition: attachment$' "$OUT/door-$name.low" || check "$name: no attachment: $(paste -sd' ' "$OUT/door-$name.low")"
  done
  for name in json video; do
    grep -q '^content-disposition' "$OUT/door-$name.low" && check "$name: shown in place, yet it says $(grep '^content-disposition' "$OUT/door-$name.low")"
  done
  for name in page module svg; do
    grep -q '^content-type: application/octet-stream$' "$OUT/door-$name.low" || check "$name's type was not made octets: $(grep '^content-type' "$OUT/door-$name.low")"
  done
  grep -q '^content-type: application/octet-stream$' "$OUT/door-listed.low" || check "a listed type was not made octets: $(grep '^content-type' "$OUT/door-listed.low")"
  grep -q '^content-type: application/octet-stream$' "$OUT/door-two.low" || check "two types were not made octets: $(grep '^content-type' "$OUT/door-two.low")"
  head -1 "$OUT/door-redirect.low" | grep -q ' 302' || check "the redirect's status: $(head -1 "$OUT/door-redirect.low")"
  grep -q '/redirected' "$OUT/canary.log" && check "the door's client followed a pod's redirect to this computer"

  # This computer's secrets stay here: a session's media token in the URL, its token, this computer's bearer.
  code=$(api -X POST "$BASE/v1/session-codes" | jq -r .code)
  session=$(curl -sS --max-time 30 -X POST -H 'Content-Type: application/json' -d "{\"code\":\"$code\"}" "$BASE/v1/session")
  media=$(jq -r .media <<< "$session") session_token=$(jq -r .token <<< "$session")
  answers "?media= on a door's read of a file" 200 - "$D/v1/runs/x/outputs/y?media=$media&kept=1"
  answers "?media= past a file route through the door" 401 unauthenticated "$D/v1/status?media=$media"
  answers "a session through the door" 200 - -H "Authorization: Bearer $session_token" -H 'Cookie: mine=1' -H "Origin: $BASE" -H 'Referer: http://x/' -H 'Sec-Fetch-Site: same-origin' -H 'Idempotency-Key: k1' -H 'Cozy-Mine: 1' "$D/v1/door/asked"
  pod "cat $P/hostile.log" > "$OUT/hostile.log" || check "the stand-in's log"
  J -s 'map(select(.path == "/v1/runs/x/outputs/y?kept=1")) | length == 1' "$OUT/hostile.log" || check "?media= reached the machine, or its other query didn't: $(grep outputs/y "$OUT/hostile.log")"
  grep -qF "$media" "$OUT/hostile.log" && check "this computer's media token reached the machine"
  grep -qF "$TOKEN" "$OUT/hostile.log" && check "this computer's bearer reached the machine"
  grep -qF "$session_token" "$OUT/hostile.log" && check "this computer's session token reached the machine"
  J -s 'map(select(.path == "/v1/door/asked")) | .[0].headers
    | (.authorization == "Bearer hostile") and (.["cozy-via"] | length > 0) and .["idempotency-key"] == "k1" and .["cozy-mine"] == "1"
      and (has("cookie") or has("origin") or has("referer") or has("sec-fetch-site") | not)' "$OUT/hostile.log" \
    || check "what reached the machine: $(jq -c -s 'map(select(.path == "/v1/door/asked")) | .[0].headers' "$OUT/hostile.log")"

  CHROME=$(command -v google-chrome) PLAYWRIGHT=$PW/node_modules/playwright-core timeout 300 node "$FIX/browser.js" "$D" "$TOKEN" "http://127.0.0.1:$port/index.html" > "$OUT/browser.json" 2> "$OUT/browser.err" \
    || check "the browser: $(tail -c 500 "$OUT/browser.err")"
  J '.control.cookies == ["planted"] and (.control.requests | length > 0) and .control.ran == "yes"' "$OUT/browser.json" \
    || check "the control's script did not show, so this check sees nothing: $(jq -c .control "$OUT/browser.json")"
  for name in page listed two json video svg none redirect; do
    J --arg n "$name" '.door[$n] | .cookies == [] and .requests == [] and .ran != "yes"' "$OUT/browser.json" || check "$name ran or stuck: $(jq -c --arg n "$name" '.door[$n]' "$OUT/browser.json")"
  done
  J '.module | .control == "loaded" and .control_ran == "control"' "$OUT/browser.json" || check "the control module did not run, so this check sees nothing: $(jq -c .module "$OUT/browser.json")"
  J '.module | .door != "loaded" and .door_ran == null' "$OUT/browser.json" || check "a module from a pod ran on this origin: $(jq -c .module "$OUT/browser.json")"
  J '.door.page.downloaded == true' "$OUT/browser.json" || check "the HTML page wasn't a download: $(jq -c .door.page "$OUT/browser.json")"
  J '.door.redirect.navigated_to | contains("/redirected") | not' "$OUT/browser.json" || check "the browser went where the machine said"
  kill "$canary" 2> /dev/null

  # A holder that's gone, then the real one, whose restart mints a new token: on a rental its TensorD starts it, elsewhere the door.
  pod "printf '%s' '{\"address\":\"127.0.0.1:1\",\"token\":\"x\",\"pid\":1,\"version\":\"0\",\"started_at_ms\":0}' > $HOLDER.new && mv $HOLDER.new $HOLDER" || check "naming nobody"
  answers "a holder that answers nothing" 503 calcifer_down "${bearer[@]}" -X POST -d '{}' "$D/v1/door/arm"
  local real; real=$(calcifers)
  pod "kill -9 $real; kill \$(cat $P/hostile.pid 2>/dev/null) 2>/dev/null; true"
  reads "after the holder it kept went"
  one_calcifer "after the stand-in"
  [ "$CALCIFER" != "$real" ] || check "Calcifer $real did not restart"
  real=$CALCIFER
  pod "kill -9 $real"
  reads "after a restart minted a new token"
  one_calcifer "after another kill -9"
  [ "$CALCIFER" != "$real" ] || check "Calcifer $real did not restart"
}

door_archive() {
  mark "the archive, $X"
  local a=$P/archived id n=0 f b
  pod "cozy project new $a > /dev/null" || check "project new"
  id=$(pod "cozy --json project -p $a status" | jq -r '.id // empty')
  [ -n "$id" ] || { check "the project has no id: $(tail -c 300 "$OUT/pod.err")"; return; }
  pod "printf '%s' iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR4nGP4z8DwHwAFAAH/q842iQAAAABJRU5ErkJggg== | base64 -d > $P/pixel.png"
  answers "import a picture through the door" 200 - "${bearer[@]}" -X POST -H 'Content-Type: application/json' -H "Idempotency-Key: door-$RANDOM" -d "{\"source\":\"$P/pixel.png\"}" "$D/v1/projects/$id/imports"
  pod "python3 - <<'PY'
path = '$a/project.lock'
text = open(path).read()
row = '  \"sha256:' + 'ab' * 32 + '\":\n    length: 7\n    machine: another-pod\n'
text = text.replace('files: {}\n', 'files:\n' + row) if 'files: {}\n' in text else text.replace('files:\n', 'files:\n' + row, 1)
open(path, 'w').write(text)
PY" || check "a file another machine keeps, in the lock"
  rm -rf "$OUT/archive"; mkdir -p "$OUT/archive"
  api -D "$OUT/archive.hdr" -o "$OUT/archive.tar" "$D/v1/projects/$id/archive" || check "GET archive"
  tr -d '\r' < "$OUT/archive.hdr" | tr A-Z a-z > "$OUT/archive.low"
  grep -q '^content-disposition: attachment' "$OUT/archive.low" || check "the archive is no download"
  grep -q '^cozy-elsewhere: 1$' "$OUT/archive.low" || check "the archive counts what's elsewhere wrong: $(paste -sd' ' "$OUT/archive.low")"
  tar -xf "$OUT/archive.tar" -C "$OUT/archive" || check "the archive isn't a tar"
  J '.elsewhere[0].machine == "another-pod" and .elsewhere[0].length == 7' "$OUT"/archive/*/elsewhere.json || check "elsewhere.json: $(cat "$OUT"/archive/*/elsewhere.json)"
  [ -f "$OUT"/archive/*/project.yaml ] || check "the archive has no project.yaml"
  for f in "$OUT"/archive/*/files/*; do
    n=$((n + 1)); b=$(basename "$f")
    [ "$(sha256sum "$f" | cut -d' ' -f1)" = "${b%%.*}" ] || check "$b isn't what its name says"
  done
  [ "$n" = 1 ] || check "$n files in the archive's files/, not the imported 1"
}

door_upload() {
  mark "100 MiB up through the door, $X"
  local up held=''
  dd if=/dev/urandom of="$OUT/up.bin" bs=1M count=100 status=none
  up=$(sha256sum "$OUT/up.bin" | cut -d' ' -f1)
  [ "$RENTAL" = 0 ] || { held=$(deadline); sleep 3; }
  answers "PUT 100 MiB" '20[014]' - --max-time 900 "${bearer[@]}" -T "$OUT/up.bin" -H 'Content-Type: application/octet-stream' "$D/v1/files/sha256:$up"
  answers "HEAD what landed" 200 - "${bearer[@]}" -I "$D/v1/files/sha256:$up"
  rm -f "$OUT/up.bin"
  [ "$RENTAL" = 0 ] && return
  for _ in $(seq 5); do [ "$(deadline)" -gt "$held" ] && break; sleep 1; done
  [ "$(deadline)" -gt "$held" ] || check "100 MiB uploaded moved no clock: $held"
}

# door_big <throttle, KiB/s> <seconds between samples>: 1 GiB at a real throttle on the reader's side: what a slow reader
# holds back, the door and TensorD must not buffer.
door_big() {
  local rate=$1 every=$2 g=$P/big gid big url sampler got began
  mark "1 GiB through the door at $rate KiB/s, $X"
  pod "cozy project new $g > /dev/null" || check "project new"
  gid=$(pod "cozy --json project -p $g status" | jq -r '.id // empty')
  [ -n "$gid" ] || { check "the big project has no id: $(tail -c 300 "$OUT/pod.err")"; return; }
  big=$(T=900 pod "mkdir -p $g/files && head -c 1073741824 /dev/urandom > $P/big.bin && sha256sum $P/big.bin | cut -d' ' -f1")
  pod "mv $P/big.bin $g/files/$big.mp4"
  url=$D/v1/projects/$gid/files/$big.mp4
  api -D "$OUT/range.hdr" -o "$OUT/range.bin" -H 'Range: bytes=0-1048575' "$url" || check "a range"
  head -1 "$OUT/range.hdr" | grep -q ' 206' && grep -qi '^content-range: bytes 0-1048575/1073741824' "$OUT/range.hdr" && [ "$(stat -c %s "$OUT/range.bin")" = 1048576 ] \
    || check "a range through the door: $(tr -d '\r' < "$OUT/range.hdr" | paste -sd' ')"
  api -D "$OUT/tail.hdr" -o "$OUT/tail.bin" -H 'Range: bytes=-1024' "$url" || check "the last KiB"
  [ "$(stat -c %s "$OUT/tail.bin")" = 1024 ] || check "the last KiB's size"
  rm -f "$OUT/range.bin" "$OUT/tail.bin"
  # What each side holds as it goes: TensorD's, the machine's Calcifer's and this computer's, the deadline, and the most any connection
  # there has in its kernel's send queue (hex, /proc/net/tcp): bytes the idle clock counted that haven't left.
  ( while :; do
      echo "$(date +%s) deadline $([ "$RENTAL" = 0 ] && echo 0 || deadline) queued $(pod "awk '\$4 == \"01\" {print substr(\$5, 1, 8)}' /proc/net/tcp /proc/net/tcp6 2> /dev/null | sort | tail -1") rss_kb $(pod "for p in \$(WHO=tensord; $PIDS) $CALCIFER; do awk '/VmRSS/{print \$2}' /proc/\$p/status; done | xargs echo") $(awk '/VmRSS/{print $2}' "/proc/$(jq -r .pid < "$lock")/status")"
      sleep "$every"
    done ) > "$OUT/samples-$X.txt" 2>&1 & sampler=$!
  BG="$BG $sampler"
  began=$(date +%s)
  # Stopped for 5 minutes is a failure; slow is only slow.
  got=$(curl -sS --limit-rate "${rate}k" --speed-time 300 --speed-limit 1024 "${bearer[@]}" "$url" | sha256sum | cut -d' ' -f1)
  LAST_BYTE=$(date +%s)
  kill "$sampler" 2> /dev/null
  say "1 GiB took $((LAST_BYTE - began)) s; samples: $(wc -l < "$OUT/samples-$X.txt"); most queued in the pod's kernel: $(awk '{print $5}' "$OUT/samples-$X.txt" | sort | tail -1) bytes, hex"
  [ "$got" = "$big" ] || check "the whole read through the door: $got, not $big"
  python3 - "$OUT/samples-$X.txt" <<'PY' || check "memory grew under the slow read: $(sed -n '1p;$p' "$OUT/samples-$X.txt")"
import re, sys
rows = [[int(x) for x in re.findall(r"\d+", line.split("rss_kb")[1])] for line in open(sys.argv[1]) if "rss_kb" in line]
rows = [r for r in rows if rows and len(r) == len(rows[0])]
sys.exit(0 if len(rows) > 1 and all(max(c) - min(c) < 200 * 1024 for c in zip(*rows)) else 1)
PY
  BIG_ELAPSED=$((LAST_BYTE - began))
}

# The harness, then the $0 rehearsal. A pod's first answer to the door is too late to learn it doesn't work.
PW=$(mktemp -d)
npm install -s --prefix "$PW" playwright-core@1.55.0 > "$OUT/npm.log" 2>&1 || fail "npm install playwright-core: $(tail -c 300 "$OUT/npm.log")"
door=$(curl -sS --max-time 30 "${bearer[@]}" "$BASE/v1/machines/no-such-pod/calcifer/v1/status" | jq -r '.error.code // empty')
[ "$door" = machine_not_found ] || fail "this cozy's Calcifer has no door: a door to nothing answers '$door'"
answers "End of local, no rental" 400 machine_local "${bearer[@]}" -X DELETE "$BASE/v1/machines/local"
J 'has("rental") | not' <(api "$BASE/v1/status") || check "this runner's status says it is a rental"
port=$(python3 -c 'import socket; s = socket.socket(); s.bind(("127.0.0.1", 0)); print(s.getsockname()[1])')
timeout 170 python3 -I "$FIX/hostile_calcifer.py" "$port" none "$OUT/harness.log" 160 & standin=$!
for _ in $(seq 20); do curl -s -o /dev/null "http://127.0.0.1:$port/v1/health" && break; sleep 0.5; done
CHROME=$(command -v google-chrome) PLAYWRIGHT=$PW/node_modules/playwright-core timeout 150 node "$FIX/browser.js" "http://127.0.0.1:$port" none "http://127.0.0.1:$port/v1/door/page" > "$OUT/harness.json" 2> "$OUT/harness.err" \
  || fail "the browser harness: $(tail -c 300 "$OUT/harness.err")"
kill "$standin" 2> /dev/null
J '.control.ran == "yes" and (.control.cookies | index("planted")) and .door.page.ran == "yes"' "$OUT/harness.json" || fail "the harness cannot see a script run: $(jq -c '{control, page: .door.page}' "$OUT/harness.json")"
mark "harness: with no door in front, the stand-in's page ran its script and planted its cookie"

if [ "$REHEARSE" = true ]; then
  mark "rehearsal: a second user here, an added machine, at no cost"
  sudo useradd -m "$U" && sudo install -D -m 755 "$(command -v cozy)" "/opt/$U/cozy" || fail "no second user"
  # From a folder it can read: what it starts stays there, and uv reads that folder's uv.toml.
  second() { ( cd / && sudo -u "$U" -H env "PATH=/opt/$U:$PATH" "/opt/$U/cozy" "$@" < /dev/null ); }
  put() { pod_in "cat > $2" < "$1"; }
  second --json machine show local > "$OUT/second.json" 2>> "$OUT/pod.err" || fail "the second user's machine: $(tail -c 300 "$OUT/pod.err")"
  there=$(jq -r .address "$OUT/second.json") print=$(jq -r .fingerprint "$OUT/second.json")
  key=$(cozy machine show local --json | jq -r .controller_key)
  [ -n "$there" ] && [ -n "$print" ] && [ -n "$key" ] || fail "no address, print or key: $(head -c 300 "$OUT/second.json")"
  second machine authorize "$key" >> "$OUT/pod.err" 2>&1 || fail "authorize: $(tail -c 300 "$OUT/pod.err")"
  X=rehearsal D=$BASE/v1/machines/$X/calcifer P=$THERE/door API=$there HOLDER=$THERE/.cozy/calcifer/calcifer.lock.holder
  # Once authorized, its TensorD restarts to listen as it must: the add waits that out.
  for _ in $(seq 60); do cozy machine add "$X" "$there" --fingerprint "$print" > "$OUT/add.txt" 2>&1 && break; sleep 2; done
  J '.state == "ready"' <(machine) || fail "the added machine: $(tail -c 300 "$OUT/add.txt")"
  pod "mkdir -p $P"
  one_calcifer "the second user's"
  answers "End of another computer's local through the door" 400 machine_local "${bearer[@]}" -X DELETE "$D/v1/machines/local"
  J 'has("rental") | not' <(api "$D/v1/status") || check "an added machine's status says it is a rental"
  door_refusals
  door_stream
  door_keys
  door_runs
  door_component
  door_hostile
  door_archive
  door_upload
  door_big 30000 5
  cozy machine end "$X" -y >> "$OUT/end-rehearsal.txt" 2>&1 || check "removing the added machine: $(tail -c 300 "$OUT/end-rehearsal.txt")"
  [ "$RC" = 0 ] || fail "the rehearsal failed: no rental"
  mark "rehearsal passed"
fi

case $PART in
idle) quote 3300 ;;
release) quote 480 ;;
esac

# The rental id, the moment there is one.
( until [ -s "$OUT/rental" ]; do sleep 1; done; echo "::notice::rental $(head -1 "$OUT/rental")" ) & BG="$BG $!"
rent
jq -e '.rental.ssh // empty' "$OUT/show.json" > /dev/null || fail "the rental has no ssh address"
mark "rental $RID is $NAME"
RENTAL=1 X=$NAME D=$BASE/v1/machines/$X/calcifer P=/root/door API=$(jq -r .address "$OUT/show.json") HOLDER=/root/.cozy/calcifer/calcifer.lock.holder THERE=/root
pod() { timeout "${T:-300}" cozy machine ssh "$NAME" -- "$1" < /dev/null 2>> "$OUT/pod.err"; }
pod_in() { timeout "${T:-300}" cozy machine ssh "$NAME" -- "$1" 2>> "$OUT/pod.err"; }
put() { pod_in "cat > $2" < "$1"; }
deadline() { api "$D/v1/status" | jq -r '.rental.idle_deadline_ms // 0'; }
step update machine update "$NAME" --tensord "$(command -v cozy)" -y || fail "update: $(tail -3 "$OUT/update.txt")"
want=$(cozy --version | awk '{print $2}')
[ "$(machine | jq -r .status.version)" = "$want" ] || fail "the pod's TensorD is $(machine | jq -r .status.version), not $want"
pod "mkdir -p $P" || fail "no shell on the pod: $(tail -c 300 "$OUT/pod.err")"
mark "a rental keeps one Calcifer: started by its TensorD, before any cozy command in a shell"
one_calcifer "at the start"
first=$CALCIFER
[ "$(pod "cut -d' ' -f6 /proc/$first/stat")" = "$first" ] || check "Calcifer $first leads no session of its own"

if [ "$PART" = release ]; then
  reads "the door to a new rental"
  J --arg m "$(machine | jq -r .status.worker_id)" '.rental.machine_id == $m' "$OUT/reply.json" || check "status.rental: $(cat "$OUT/reply.json")"
  mark "End through the door"
  answers "End through the door" 202 - "${bearer[@]}" -X DELETE "$D/v1/machines/local"
  J '.machine.state == "ending"' "$OUT/reply.json" || check "End's answer: $(head -c 300 "$OUT/reply.json")"
  began=$(date +%s)
  for _ in $(seq 60); do [ "$(state "$RID")" = ended ] && break; sleep 2; done
  [ "$(state "$RID")" = ended ] || check "the Hub did not end it within 2 min: $(machine | jq -c '{state, reason}')"
  mark "ended at the Hub $(( $(date +%s) - began )) s after End: $(machine | jq -c '{state, reason}')"
  exit "$RC"
fi

mark "kill -9 brings a new one, and a shell's cozy uses it"
pod "kill -9 $first"
one_calcifer "after kill -9"
[ "$CALCIFER" != "$first" ] || check "kill -9 of $first left the same pid"
pod "cozy status" > "$OUT/pod-status.txt" 2>&1 || check "cozy status in a pod shell failed: $(tail -c 300 "$OUT/pod.err")"
second_pid=$CALCIFER
one_calcifer "after a shell's cozy"
[ "$CALCIFER" = "$second_pid" ] || check "a shell's cozy moved Calcifer from $second_pid to $CALCIFER"
mark "a TensorD restart leaves it running: an update's, and its service's crash"
before=$(tensords)
step restart machine update "$NAME" --gpu-memory-limit off -y || check "machine update --gpu-memory-limit off: $(tail -3 "$OUT/restart.txt")"
one_calcifer "after an update"
[ "$CALCIFER" = "$second_pid" ] || check "an update moved Calcifer from $second_pid to $CALCIFER"
[ "$(tensords)" != "$before" ] || check "the update restarted no TensorD: '$before'"
service=$(tensords | awk '{print $NF}')
pod "kill -9 $service"
for _ in $(seq 60); do [ "$(tensords | awk '{print $NF}')" != "$service" ] && [ "$(machine | jq -r '.status.phase // empty')" = ready ] && break; sleep 2; done
[ "$(tensords | awk '{print $NF}')" != "$service" ] || check "TensorD's service $service did not come back"
one_calcifer "after TensorD's service was killed"
[ "$CALCIFER" = "$second_pid" ] || check "a crash of TensorD moved Calcifer from $second_pid to $CALCIFER"
reads "after TensorD's restarts"
J --arg m "$(machine | jq -r .status.worker_id)" '.rental.machine_id == $m' "$OUT/reply.json" || check "status.rental: $(cat "$OUT/reply.json")"

door_refusals
one_calcifer "after the refused down"
[ "$CALCIFER" = "$second_pid" ] || check "POST down through the door moved Calcifer"
door_stream
door_keys
door_runs
mark "a job holds the rental: End through the door refuses while one runs"
pod "cd $P/runs/p && nohup cozy --json run -p $P/runs/p ../../fixture.py/wait seconds:=600 --on local > $P/wait.json 2>&1 &"
for _ in $(seq 180); do [ "$(deadline)" = 0 ] && break; sleep 2; done
[ "$(deadline)" = 0 ] || check "with a job queued or running, status still names an idle deadline: $(deadline)"
answers "End while a job runs" 409 machine_working "${bearer[@]}" -X DELETE "$D/v1/machines/local"
WAIT=$(api "$D/v1/runs?limit=1" | jq -r '.runs[0].id')
answers "cancel the waiting run" 202 - "${bearer[@]}" -X POST "$D/v1/runs/$WAIT/cancel"
for _ in $(seq 60); do [ "$(deadline)" != 0 ] && break; sleep 2; done
[ "$(deadline)" != 0 ] || check "a cancelled job left no idle deadline"
door_hostile
door_archive
door_upload
door_big "$KIB_S" 60
[ "$BIG_ELAPSED" -gt 900 ] || check "the read took $BIG_ELAPSED s, under one idle window, so it proved nothing about holding the clock"
[ "$(state "$RID")" = ready ] || check "the pod is $(state "$RID") after a download longer than its window"
# What its watch last said, once the last bytes' clock has reached it.
sleep 3
last=$(deadline)
[ "$last" -ge "$(( (LAST_BYTE - 5) * 1000 + 840000 ))" ] || check "the deadline $last isn't 15 min past the last bytes at $LAST_BYTE"

mark "a page held open and no job: the deadline holds at $last, and the pod releases itself"
( while :; do curl -sSN "${bearer[@]}" -H 'Accept: text/event-stream' "$D/v1/events" >> "$OUT/page-events.txt" 2>&1; sleep 1; done ) & BG="$BG $!"
moved=0 s='' polls=0
while [ $(( $(date +%s) - LAST_BYTE )) -lt 1080 ]; do
  now=$(curl -sS --max-time 5 "${bearer[@]}" "$D/v1/status" 2> /dev/null | jq -r '.rental.idle_deadline_ms // empty' 2> /dev/null)
  curl -sS --max-time 5 "${bearer[@]}" -o /dev/null "$D/v1/machines" 2> /dev/null
  polls=$((polls + 1))
  [ -z "$now" ] || [ "$now" = "$last" ] || moved=$now
  s=$(state "$RID")
  case $s in ended | ending) break ;; esac
  sleep 5
done
released=$(( $(date +%s) - LAST_BYTE ))
mark "state $s, $released s after the last bytes, after $polls polls and a stream of $(wc -c < "$OUT/page-events.txt") bytes"
[ "$moved" = 0 ] || check "polling and a stream moved the idle deadline from $last to $moved"
case $s in ended | ending) ;; *) check "still $s $released s after the last bytes: an open page held it" ;; esac
[ "$released" -ge 840 ] || check "released $released s after the last bytes: earlier than its window"
exit "$RC"
