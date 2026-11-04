#!/usr/bin/env bash
# e2e/cli/door.sh <machine> <out dir> <idle|release>: cl-749 and cl-774 Part D on a rental of this computer's, rented with
# --ssh and on a cozy that has them, as this computer's is: a rental keeps one Calcifer; the door reaches it, streams, and
# refuses what it must; a hostile pod puts nothing on this computer's origin; a job holds the clock; files moving hold it; a
# page held open doesn't, and it releases itself (`idle`). `release` is a second pod, as each ends its pod: only the boot
# check, then End through the door. It rents and ends nothing else; the caller's guard verifies the pod gone at its provider.
here=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
. "$here/../lib.sh"
X=$1 HOW=$3
parse --out "$2"
hub_checked() { :; }
J() { jq -e "$@" > /dev/null; }
trap 'kill ${canary:-} ${sampler:-} ${stream:-} 2> /dev/null || true' EXIT
lock=$HOME/.cozy/calcifer/calcifer.lock.holder
ADDR=$(jq -r .address < "$lock") TOKEN=$(jq -r .token < "$lock")
BASE=http://$ADDR D=$BASE/v1/machines/$X/calcifer
api() { curl -sS -H "Authorization: Bearer $TOKEN" "$@"; }
pod() { cozy machine ssh "$X" -- "$1"; }
machine() { cozy --json machine show "$X"; }
# Google's own Chrome, and playwright-core, as the web cell has them.
CHROME=$(command -v google-chrome) PW=$OUT/playwright

# answers <what> <status, a regex> <error code, or -> <curl args…>
answers() {
  local what=$1 want=$2 code=$3 got; shift 3
  got=$(curl -sS -o "$OUT/reply.json" -w '%{http_code}' "$@") || got="no answer"
  { grep -Eqx "$want" <<< "$got" && { [ "$code" = - ] || J --arg c "$code" '.error.code == $c or .error.cause == $c' "$OUT/reply.json"; }; } \
    || check "$what: $got, not $want ${code/#-/}: $(head -c 300 "$OUT/reply.json")"
}
bearer=(-H "Authorization: Bearer $TOKEN")

# On the pod: Calcifers by /proc (an image has no procps to promise), the holder's pid, and a session's leader.
PIDS='for p in /proc/[0-9]*; do [ "$(cat $p/comm 2>/dev/null)" = "$NAME" ] && basename $p; done | sort -n | xargs echo'
HOLDER=/root/.cozy/calcifer/calcifer.lock.holder
calcifers() { pod "NAME=calcifer; $PIDS"; }
tensords() { pod "NAME=tensord; $PIDS"; }
holder_pid() { pod "grep -o '\"pid\":[0-9]*' $HOLDER | cut -d: -f2"; }
# one_calcifer <what>: exactly one, the one the record names; its pid in $CALCIFER.
one_calcifer() {
  local pids held
  for _ in $(seq 30); do
    pids=$(calcifers) held=$(holder_pid 2> /dev/null || true)
    [ -n "$pids" ] && [ "$pids" = "$held" ] && break
    sleep 1
  done
  [ -n "$pids" ] && [ "$pids" = "$held" ] || check "$1: Calcifers '$pids', the record names '$held'"
  CALCIFER=$pids
}

step "a rental keeps one Calcifer: at boot, before any cozy command here, detached"
one_calcifer "at boot"
[ "$(pod "cut -d' ' -f6 /proc/$CALCIFER/stat")" = "$CALCIFER" ] || check "Calcifer $CALCIFER leads no session of its own"
first=$CALCIFER

whole() {
step "kill -9 brings a new one, and a shell's cozy uses it"
pod "kill -9 $first"
one_calcifer "after kill -9"
[ "$CALCIFER" != "$first" ] || check "kill -9 of $first left the same pid"
pod "cozy status" > "$OUT/pod-status.txt" 2>&1 || check "cozy status in a pod shell failed: $(tail -c 300 "$OUT/pod-status.txt")"
second=$CALCIFER
one_calcifer "after a shell's cozy"
[ "$CALCIFER" = "$second" ] || check "a shell's cozy moved Calcifer from $second to $CALCIFER"

step "a TensorD restart leaves it running: an update's, and a crash's"
before=$(tensords)
cozy machine update "$X" --gpu-memory-limit off -y > "$OUT/restart.txt" 2>&1 || check "machine update --gpu-memory-limit off: $(tail -c 300 "$OUT/restart.txt")"
one_calcifer "after an update"
[ "$CALCIFER" = "$second" ] || check "an update moved Calcifer from $second to $CALCIFER"
[ "$(tensords)" != "$before" ] || check "the update restarted no TensorD: '$before'"
service=$(tensords | awk '{print $NF}')
pod "kill -9 $service"
for _ in $(seq 60); do [ "$(machine | jq -r '.status.phase // empty')" = ready ] && [ "$(tensords | awk '{print $NF}')" != "$service" ] && break; sleep 2; done
one_calcifer "after TensorD's service was killed"
[ "$CALCIFER" = "$second" ] || check "a crash of TensorD moved Calcifer from $second to $CALCIFER"

step "the door: a remote Calcifer's API under a base path, this one's own refusals"
MACHINE_ID=$(machine | jq -r '.rental_id // empty')
api "$D/v1/status" > "$OUT/status.json" || check "GET status through the door"
J '.rental.machine_id | length > 0' "$OUT/status.json" || check "status on a rental has no rental: $(cat "$OUT/status.json")"
machine_id=$(jq -r .rental.machine_id "$OUT/status.json")
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
one_calcifer "after the refused down"
[ "$CALCIFER" = "$second" ] || check "POST down through the door moved Calcifer"

step "SSE through the door: a quiet stream opens, and its bytes hold no clock"
deadline() { api "$D/v1/status" | jq -r '.rental.idle_deadline_ms // 0'; }
held=$(deadline)
timeout 40 curl -sSN "${bearer[@]}" -H 'Accept: text/event-stream' "$D/v1/events" > "$OUT/events.txt" 2> "$OUT/events.err" || true
grep -q '"type":"hello"\|"type": "hello"' "$OUT/events.txt" || check "no hello on the stream through the door: $(head -c 300 "$OUT/events.txt")"
grep -q '^:' "$OUT/events.txt" || check "no keep-alive comment in 40 s through the door"
[ "$(deadline)" = "$held" ] || check "40 s of a stream moved the idle deadline from $held to $(deadline)"

step "keys: not admitted, and a play link's"
API=$(machine | jq -r .address)
python3 - "$machine_id" > "$OUT/foreign.cap" <<'PY'
import base64, hashlib, json, sys, time
from cryptography.hazmat.primitives.asymmetric.ed25519 import Ed25519PrivateKey
from cryptography.hazmat.primitives.serialization import Encoding, PublicFormat
b64 = lambda b: base64.urlsafe_b64encode(b).rstrip(b"=").decode()
key = Ed25519PrivateKey.generate()
public = key.public_key().public_bytes(Encoding.Raw, PublicFormat.Raw)
payload = json.dumps({"m": sys.argv[1], "a": "machine", "e": int(time.time()) + 300, "k": b64(hashlib.sha256(public).digest()[:16])}).encode()
print(b64(payload) + "." + b64(key.sign(b"cozy-capability/1\0" + payload)))
PY
answers "a key not admitted" 401 capability_invalid -k -H "Authorization: Cozy-Cap $(cat "$OUT/foreign.cap")" "https://$API/v1/calcifer/v1/status"
answers "no key" 401 capability_absent -k "https://$API/v1/calcifer/v1/status"

step "a run on the pod's Calcifer, from a pod shell; its outputs through the door"
tar -czf - -C "$here" weightless project/hostile.py | pod 'mkdir -p /root/door && tar -xzf - -C /root/door' || check "fixtures to the pod"
pod 'cozy project new /root/door/runs >/dev/null && cd /root/door/runs && cozy --json run ../project/hostile.py/hostile "prompt=through the door" --on local' > "$OUT/hostile-run.json" 2> "$OUT/hostile-run.err" \
  || check "a run in a pod shell failed: $(tail -c 300 "$OUT/hostile-run.err")"
api "$D/v1/runs?limit=1" > "$OUT/runs.json" || check "GET runs through the door"
RUN=$(jq -r '.runs[0].id // empty' "$OUT/runs.json")
[ -n "$RUN" ] && J '.runs[0].status == "completed"' "$OUT/runs.json" || check "the door shows no finished run: $(head -c 300 "$OUT/runs.json")"
api -D "$OUT/page.hdr" -o "$OUT/page.body" "$D/v1/runs/$RUN/outputs/page" || check "an HTML output through the door"
grep -qi '^content-disposition: attachment' "$OUT/page.hdr" || check "a real HTML output through the door is no download: $(tr -d '\r' < "$OUT/page.hdr" | paste -sd' ')"
grep -qi '^content-security-policy: sandbox' "$OUT/page.hdr" || check "a real HTML output through the door is not sandboxed"
api "$D/v1/runs/$RUN/play?output=page" > "$OUT/play.json" || check "GET play through the door"
RUNCAP=$(jq -r '.token // empty' "$OUT/play.json")
[ -n "$RUNCAP" ] || check "play answered no token: $(head -c 300 "$OUT/play.json")"
answers "a play link's run-scope cap" 403 capability_scope -k -H "Authorization: Cozy-Cap $RUNCAP" "https://$API/v1/calcifer/v1/status"
answers "a play link's cap on the machine's own API (control)" 403 capability_scope -k -H "Authorization: Cozy-Cap $RUNCAP" -X POST "https://$API/v1/keepalive"

step "a job holds the rental: End through the door refuses while one runs"
pod 'cd /root/door/runs && nohup cozy --json run ../weightless/wait seconds:=600 --on local > /root/door/wait.json 2>&1 &' || check "a waiting run"
for _ in $(seq 180); do [ "$(deadline)" = 0 ] && api "$D/v1/runs?limit=1" | J '.runs[0].status == "running"' && break; sleep 2; done
[ "$(deadline)" = 0 ] || check "with a job running, status names an idle deadline: $(deadline)"
answers "End while a job runs" 409 machine_working "${bearer[@]}" -X DELETE "$D/v1/machines/local"
WAIT=$(api "$D/v1/runs?limit=1" | jq -r '.runs[0].id')
answers "cancel the waiting run" 200 - "${bearer[@]}" -X POST "$D/v1/runs/$WAIT/cancel"
for _ in $(seq 60); do [ "$(deadline)" != 0 ] && break; sleep 2; done
[ "$(deadline)" != 0 ] || check "a cancelled job left no idle deadline"

step "a hostile pod: it names its own holder, and answers what it likes"
PORT=$(python3 -c 'import socket; s = socket.socket(); s.bind(("127.0.0.1", 0)); print(s.getsockname()[1])')
mkdir -p "$OUT/control"
printf '%s' "<!doctype html><title>control</title><script>document.documentElement.setAttribute('data-ran','yes');document.cookie='planted=1;path=/';fetch('/x');</script>" > "$OUT/control/index.html"
# This computer's canary: a redirect the door followed would land here.
timeout 600 python3 -m http.server --bind 127.0.0.1 "$PORT" --directory "$OUT/control" > "$OUT/canary.log" 2>&1 &
canary=$!
ELSEWHERE=http://127.0.0.1:$PORT/redirected
pod 'cat > /root/door/hostile_calcifer.py' < "$here/door/hostile_calcifer.py" || check "the stand-in to the pod"
pod "nohup python3 /root/door/hostile_calcifer.py 41999 $ELSEWHERE /root/door/hostile.log 600 > /root/door/hostile.err 2>&1 &" || check "the stand-in"
pod "for i in 1 2 3 4 5 6 7 8 9 10; do python3 -c 'import socket; socket.create_connection((\"127.0.0.1\", 41999))' 2>/dev/null && break; sleep 1; done"
named='{"address":"127.0.0.1:41999","token":"hostile","pid":1,"version":"0","started_at_ms":0}'
pod "printf '%s' '$named' > $HOLDER.new && mv $HOLDER.new $HOLDER" || check "naming the stand-in"
# A body makes the door read the record again, and it keeps what it read.
answers "the door reads the new holder" 200 - "${bearer[@]}" -X POST -H 'Content-Type: application/json' -d '{}' "$D/v1/door/arm"
for name in page listed two json video svg none redirect; do
  api -D "$OUT/door-$name.hdr" -o "$OUT/door-$name.body" "$D/v1/door/$name" || check "door $name"
  tr -d '\r' < "$OUT/door-$name.hdr" | tr A-Z a-z > "$OUT/door-$name.low"
  for gone in set-cookie location refresh clear-site-data access-control link www-authenticate; do
    grep -q "^$gone" "$OUT/door-$name.low" && check "$name: a pod's $gone came through: $(paste -sd' ' "$OUT/door-$name.low")"
  done
  grep -q '^content-security-policy: sandbox$' "$OUT/door-$name.low" || check "$name: not sandboxed, or the pod's CSP came through: $(paste -sd' ' "$OUT/door-$name.low")"
  grep -q '^x-content-type-options: nosniff' "$OUT/door-$name.low" || check "$name: no nosniff"
  grep -q '^cozy-said: kept' "$OUT/door-$name.low" || check "$name: Cozy-* did not pass"
done
for name in page listed two svg none redirect; do
  grep -q '^content-disposition: attachment$' "$OUT/door-$name.low" || check "$name: no attachment: $(paste -sd' ' "$OUT/door-$name.low")"
done
for name in json video; do
  grep -q '^content-disposition' "$OUT/door-$name.low" && check "$name: shown in place, yet it says $(grep '^content-disposition' "$OUT/door-$name.low")"
done
grep -q '^content-type: text/html$' "$OUT/door-page.low" || check "page's type: $(grep '^content-type' "$OUT/door-page.low")"
grep -q '^content-type: application/octet-stream$' "$OUT/door-listed.low" || check "a listed type was not made octets: $(grep '^content-type' "$OUT/door-listed.low")"
grep -q '^content-type: application/octet-stream$' "$OUT/door-two.low" || check "two types were not made octets: $(grep '^content-type' "$OUT/door-two.low")"
head -1 "$OUT/door-redirect.low" | grep -q ' 302' || check "the redirect's status: $(head -1 "$OUT/door-redirect.low")"
[ ! -s "$OUT/door-redirect.body" ] || check "the redirect's body: $(head -c 100 "$OUT/door-redirect.body")"
grep -q '/redirected' "$OUT/canary.log" && check "the door's client followed a pod's redirect to this computer"

# This computer's secrets stay here: a session's media token in the URL, and its bearer.
code=$(api -X POST "$BASE/v1/session-codes" | jq -r .code)
session=$(curl -sS -X POST -H 'Content-Type: application/json' -d "{\"code\":\"$code\"}" "$BASE/v1/session")
media=$(jq -r .media <<< "$session") session_token=$(jq -r .token <<< "$session")
answers "?media= on a door's read of a file" 200 - "$D/v1/runs/x/outputs/y?media=$media&kept=1"
answers "?media= past a file route through the door" 401 unauthenticated "$D/v1/status?media=$media"
answers "a session through the door" 200 - -H "Authorization: Bearer $session_token" -H 'Cookie: mine=1' -H "Origin: $BASE" -H 'Referer: http://x/' -H 'Sec-Fetch-Site: same-origin' -H 'Idempotency-Key: k1' -H 'Cozy-Mine: 1' "$D/v1/door/asked"
pod 'cat /root/door/hostile.log' > "$OUT/hostile.log" || check "the stand-in's log"
J -s 'map(select(.path == "/v1/runs/x/outputs/y?kept=1")) | length == 1' "$OUT/hostile.log" || check "?media= reached the pod, or its other query didn't: $(grep outputs/y "$OUT/hostile.log")"
grep -q "$media" "$OUT/hostile.log" && check "this computer's media token reached the pod"
grep -q "$TOKEN" "$OUT/hostile.log" && check "this computer's bearer reached the pod"
grep -q "$session_token" "$OUT/hostile.log" && check "this computer's session token reached the pod"
jq -e -s 'map(select(.path == "/v1/door/asked")) | .[0].headers
  | (.authorization == "Bearer hostile") and (.["cozy-via"] | length > 0) and .["idempotency-key"] == "k1" and .["cozy-mine"] == "1"
    and (has("cookie") or has("origin") or has("referer") or has("sec-fetch-site") | not)' "$OUT/hostile.log" > /dev/null \
  || check "what reached the pod: $(jq -c -s 'map(select(.path == "/v1/door/asked")) | .[0].headers' "$OUT/hostile.log")"

step "a hostile pod in a real Chrome: nothing runs, nothing sticks, and the harness can see what does"
npm install -s --prefix "$PW" playwright-core@1.55.0 > "$OUT/npm.log" 2>&1 || check "npm install playwright-core: $(tail -c 300 "$OUT/npm.log")"
CHROME=$CHROME PLAYWRIGHT=$PW/node_modules/playwright-core node "$here/door/browser.js" "$D" "$TOKEN" "http://127.0.0.1:$PORT/index.html" > "$OUT/browser.json" 2> "$OUT/browser.err" \
  || check "the browser: $(tail -c 500 "$OUT/browser.err")"
J '.control.cookies == ["planted"] and (.control.requests | length > 0) and .control.ran == "yes"' "$OUT/browser.json" \
  || check "the control's script did not show, so this check sees nothing: $(jq -c .control "$OUT/browser.json")"
for name in page listed two json video svg none redirect; do
  J --arg n "$name" '.door[$n] | .cookies == [] and .requests == [] and .ran != "yes"' "$OUT/browser.json" || check "$name ran or stuck: $(jq -c --arg n "$name" '.door[$n]' "$OUT/browser.json")"
done
J '.door.page.downloaded == true' "$OUT/browser.json" || check "the HTML page wasn't a download: $(jq -c .door.page "$OUT/browser.json")"
J '.door.redirect.navigated_to | contains("/redirected") | not' "$OUT/browser.json" || check "the browser went where the pod said"
kill "$canary" 2> /dev/null || true

step "the record again: a holder that's gone, then the real one's new token"
pod "printf '%s' '{\"address\":\"127.0.0.1:1\",\"token\":\"x\",\"pid\":1,\"version\":\"0\",\"started_at_ms\":0}' > $HOLDER.new && mv $HOLDER.new $HOLDER" || check "naming nobody"
answers "a holder that answers nothing" 503 calcifer_down "${bearer[@]}" -X POST -d '{}' "$D/v1/door/arm"
real=$(calcifers)
pod "kill -9 $real; pkill -f hostile_calcifer 2>/dev/null; true"
one_calcifer "after the stand-in"
[ "$CALCIFER" != "$real" ] || check "Calcifer $real did not restart"
answers "a read after the holder it kept went" 200 - "${bearer[@]}" "$D/v1/status"
real=$CALCIFER
pod "kill -9 $real"
one_calcifer "after another kill -9"
answers "a read after a restart minted a new token" 200 - "${bearer[@]}" "$D/v1/status"

step "archive: every file here, each its hash, naming what stayed elsewhere"
A=/root/door/archived
pod "cozy project new $A >/dev/null" || check "project new"
AID=$(pod "cozy --json project -p $A status" | jq -r '.id // empty')
[ -n "$AID" ] || check "the project has no id"
base64 -d <<< 'iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR4nGP4z8DwHwAFAAH/q842iQAAAABJRU5ErkJggg==' | pod 'cat > /root/door/pixel.png'
answers "import a picture through the door" 200 - "${bearer[@]}" -X POST -H 'Content-Type: application/json' -H 'Idempotency-Key: door-1' -d '{"source":"/root/door/pixel.png"}' "$D/v1/projects/$AID/imports"
pod "python3 - <<'PY'
import json
path = '$A/project.lock'
lock = json.load(open(path))
digest = 'sha256:' + 'ab' * 32
lock['files'][digest] = {'path': 'files/' + 'ab' * 32, 'sha256': digest, 'machine': 'another-pod', 'length': 7}
json.dump(lock, open(path, 'w'))
PY" || check "a file another machine keeps, in the lock"
api -D "$OUT/archive.hdr" -o "$OUT/archive.tar" "$D/v1/projects/$AID/archive" || check "GET archive"
tr -d '\r' < "$OUT/archive.hdr" | tr A-Z a-z > "$OUT/archive.low"
grep -q '^content-disposition: attachment' "$OUT/archive.low" || check "the archive is no download"
grep -q '^cozy-elsewhere: 1$' "$OUT/archive.low" || check "the archive counts what's elsewhere wrong: $(paste -sd' ' "$OUT/archive.low")"
mkdir -p "$OUT/archive" && tar -xf "$OUT/archive.tar" -C "$OUT/archive" || check "the archive isn't a tar"
J '.elsewhere[0].machine == "another-pod" and .elsewhere[0].length == 7' "$OUT"/archive/*/elsewhere.json || check "elsewhere.json: $(cat "$OUT"/archive/*/elsewhere.json)"
[ -f "$OUT"/archive/*/project.yaml ] || check "the archive has no project.yaml"
n=0
for f in "$OUT"/archive/*/files/*; do
  n=$((n + 1)); b=$(basename "$f")
  [ "$(sha256sum "$f" | cut -d' ' -f1)" = "${b%%.*}" ] || check "$b isn't what its name says"
done
[ "$n" = 1 ] || check "$n files in the archive's files/, not the imported 1"

step "an upload through the door moves the clock, and lands whole"
dd if=/dev/urandom of="$OUT/up.bin" bs=1M count=100 status=none
up=$(sha256sum "$OUT/up.bin" | cut -d' ' -f1)
held=$(deadline)
sleep 3
answers "PUT 100 MiB" '20[014]' - "${bearer[@]}" -T "$OUT/up.bin" -H 'Content-Type: application/octet-stream' "$D/v1/files/sha256:$up"
answers "HEAD what landed" 200 - "${bearer[@]}" -I "$D/v1/files/sha256:$up"
for _ in $(seq 5); do [ "$(deadline)" -gt "$held" ] && break; sleep 1; done
[ "$(deadline)" -gt "$held" ] || check "100 MiB uploaded moved no clock: $held"
rm -f "$OUT/up.bin"

step "1 GiB through the door: ranges, the whole at a slow reader's pace past one idle window, and no one buffers it"
G=/root/door/big
pod "cozy project new $G >/dev/null" || check "project new"
GID=$(pod "cozy --json project -p $G status" | jq -r '.id // empty')
big=$(pod "mkdir -p $G/files && head -c 1073741824 /dev/urandom > /root/door/big.bin && sha256sum /root/door/big.bin | cut -d' ' -f1")
pod "mv /root/door/big.bin $G/files/$big.mp4"
U=$D/v1/projects/$GID/files/$big.mp4
api -D "$OUT/range.hdr" -o "$OUT/range.bin" -H 'Range: bytes=0-1048575' "$U" || check "a range"
head -1 "$OUT/range.hdr" | grep -q ' 206' && grep -qi '^content-range: bytes 0-1048575/1073741824' "$OUT/range.hdr" && [ "$(stat -c %s "$OUT/range.bin")" = 1048576 ] \
  || check "a range through the door: $(tr -d '\r' < "$OUT/range.hdr" | paste -sd' ')"
api -D "$OUT/tail.hdr" -o "$OUT/tail.bin" -H 'Range: bytes=-1024' "$U" || check "the last KiB"
[ "$(stat -c %s "$OUT/tail.bin")" = 1024 ] || check "the last KiB's size"
# Its own sampler: a minute's bytes moved, the deadline moving, and what each side holds in memory.
( exec 9>&-
  while :; do
    echo "$(date +%s) deadline $(deadline) x_rss_kb $(pod "NAME=tensord; for p in \$($PIDS) $CALCIFER; do awk '/VmRSS/{print \$2}' /proc/\$p/status; done | xargs echo") here_rss_kb $(awk '/VmRSS/{print $2}' "/proc/$(jq -r .pid < "$lock")/status")"
    sleep 60
  done ) > "$OUT/samples.txt" 2>&1 &
sampler=$!
began=$(date +%s)
# A stall of 5 minutes is a failure; a slow line is only slow. 1000 KiB/s is 17.5 min, longer than the window it must outlast.
got=$(curl -sS --limit-rate 1000k --speed-time 300 --speed-limit 1024 "${bearer[@]}" "$U" | sha256sum | cut -d' ' -f1)
LAST_BYTE=$(date +%s)
kill "$sampler" 2> /dev/null || true
[ "$got" = "$big" ] || check "the whole read through the door: $got, not $big"
[ "$((LAST_BYTE - began))" -gt 900 ] || check "the read took $((LAST_BYTE - began)) s, under one idle window, so it proved nothing about holding the clock"
J '.state != "ended"' <(machine) || check "the pod ended under a moving download"
# What its watch last said, once the last bytes' clock has reached it.
sleep 3
last=$(deadline)
[ "$last" -ge "$(( (LAST_BYTE - 5) * 1000 + 840000 ))" ] || check "the deadline $last isn't 15 min past the last bytes at $LAST_BYTE"
# Memory: flat under a reader slower than the link, to well under a copy of the file. This computer's is shared, so loosely.
python3 - "$OUT/samples.txt" <<'PY' || check "memory grew under the slow read: $(tail -3 "$OUT/samples.txt")"
import re, sys
rows = [[int(x) for x in re.findall(r"\d+", line.split("x_rss_kb")[1])] for line in open(sys.argv[1]) if "x_rss_kb" in line]
rows = [r for r in rows if len(r) == len(rows[0])]
sys.exit(0 if rows and all(max(c) - min(c) < 300 * 1024 for c in zip(*rows)) else 1)
PY
}

case $HOW in
idle)
  whole
  step "a page held open and no job: the pod still releases itself, within 15 minutes of the last bytes"
  ( exec 9>&-; while :; do curl -sSN "${bearer[@]}" -H 'Accept: text/event-stream' "$D/v1/events" >> "$OUT/page-events.txt" 2>&1; sleep 1; done ) &
  stream=$!
  moved=0 state=''
  while [ $(( $(date +%s) - LAST_BYTE )) -lt 1080 ]; do
    now=$(api --max-time 5 "$D/v1/status" | jq -r '.rental.idle_deadline_ms // empty' 2> /dev/null || true)
    api --max-time 5 "$D/v1/machines" > /dev/null 2>&1 || true
    [ -z "$now" ] || [ "$now" = "$last" ] || moved=$now
    state=$(machine | jq -r '.state // empty' 2> /dev/null || true)
    case $state in ended | ending) break ;; esac
    sleep 5
  done
  kill "$stream" 2> /dev/null || true
  gone=$(date +%s)
  echo "state $state, $((gone - LAST_BYTE)) s after the last bytes" | tee "$OUT/released.txt"
  [ "$moved" = 0 ] || check "polling and a stream moved the idle deadline from $last to $moved"
  case $state in ended | ending) ;; *) check "still $state $((gone - LAST_BYTE)) s after the last bytes: an open page held it" ;; esac
  [ "$((gone - LAST_BYTE))" -ge 840 ] || check "released $((gone - LAST_BYTE)) s after the last bytes: earlier than its window"
  ;;
release)
  step "End through the door: the pod's Calcifer relays, and the Hub ends it"
  answers "End through the door" 202 - "${bearer[@]}" -X DELETE "$D/v1/machines/local"
  J '.machine.state == "ending"' "$OUT/reply.json" || check "End's answer: $(head -c 300 "$OUT/reply.json")"
  for _ in $(seq 60); do J '.state == "ended"' <(machine) && break; sleep 2; done
  J '.state == "ended"' <(machine) || check "the Hub did not end it within 2 min: $(machine | jq -c '{state, reason}')"
  ;;
*) check "unknown ending '$HOW'" ;;
esac
finish door
