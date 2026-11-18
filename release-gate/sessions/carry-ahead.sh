# carry-ahead (cl-1123; Paul, 11-18: "uploading files might be a good pre-load to do too, like if I open a node, and it has a video that
# is a reference video that is like 100MBs, uploading that to the remote machine will take a few minutes … so pre-loading files on the
# machine makes sense as well. Although I would stop short at actually doing the full run", and "even for encrypted projects, sending
# ahead of time is fine. What really matters is that we can trust the remote machine as safe"): in an encrypted project, a box's input
# files and the project's own code are carried ahead to the rental its render would go to, behind every run's carry; its render finds
# them there and sends nothing more, and nothing is run ahead.
# Qwen Image 2 from a packages checkout (a dev home), every step through this runner's cozy and its Calcifer, on the session's TensorD;
# evidence is the machine's own log and records.
# 0. An encrypted project (`cozy new`, its passphrase from a file), unlocked with no idle lock. `photo` and `wide`, pictures of noise
#    (2048 and 4096 pixels square, so each goes in ranges), imported into it; its own code (e2e/cli/project/boxed, weightless) copied
#    into it, and `own`, a box of that code. On this computer that code stays in its vault: a page shows the project and Calcifer
#    says so. All of it before anything is rented. Then the rental, with qwen-image-2 installed.
# 1. A page shows the project: own's guess goes to the rental, its code carried there (the machine's log lands its objects) and its
#    item ready; own's render adopts the item.
# 2. photo gets Qwen and an edit: Calcifer says photo's guess carried its picture ahead to the rental, an encrypted project's, and the
#    machine says it landed whole, before any render. Its request is `loaded` and stays so, its draft unchanged: nothing runs ahead.
# 3. The runner's uplink shaped to `mbit`: wide gets Qwen and an edit, and while its picture goes ahead, photo is rendered. The carry
#    waits for the render's ("a run's inputs go first") and goes on after; the machine lands wide's picture whole once, and the runner
#    sends about its size, not more.
# 4. Each render (photo in 3, then wide) reused its picture there ("reused … already on"), the machine landed none of its picture while it
#    was sent, and it adopted its item. No predicted request ran: the machine's log names no speculative run, and its Status never
#    said `running` or `done` for one.
# 5. yield (load-ahead.sh's): `other`'s weights fetched by one call, TensorD restarted, and `other` sent while a predicted request is
#    `loading`: TensorD's log says how long the call waited for the GPU. Said, not judged; skipped when the cap leaves too little.
# params:
#   tree    the packages_refs checkout whose qwen-image-2 runs (`-f packages_refs=pk=<packages ref>`, tree pk)
#   need_s  the seconds the cap must buy (default 2400)
#   mbit    the shaped uplink in step 3 (default 16)
#   yield   false: no step 5 (default true)
#   other   the call that takes the GPU during a load (default sdxl/generate)
. "$(dirname "$0")/lib.sh"
fail() {
  [ -z "${DEV:-}" ] || sudo tc qdisc del dev "$DEV" root 2> /dev/null || true
  [ -z "${NAME:-}" ] || cozy machine logs "$NAME" --since 2h -n 0 > "$OUT/tensord-$NAME.log" 2>&1
  cozy machine logs local calcifer --lines 20000 > "$OUT/calcifer.log" 2>&1
  say "FAIL: $*"; RC=1; exit 1
}
TREE=$(param tree '') MBIT=$(param mbit 16) YIELD=$(param yield true) OTHER=$(param other sdxl/generate)
I2I=darrellbest/qwen-image-2.1-pe-i2i-abliterated
[ -n "$TREE" ] && [ -f "$PACKAGES/$TREE/qwen-image-2/pyproject.toml" ] || fail "dispatch with -f packages_refs=<name>=<packages ref> and params.tree=<name>"
[[ $MBIT =~ ^[0-9]+$ && $MBIT -ge 4 ]] || fail "mbit $MBIT"
[[ $YIELD =~ ^(true|false)$ ]] || fail "yield is true or false"
SOURCE=$(cd "$PACKAGES/$TREE" && pwd)
V=$(cozy --version | awk '{print $2}')
[[ $V == *-dev* ]] || fail "cozy $V: the change runs from a dev build (pod-session cozy=<dev-build run id>)"
for tool in jq curl ffmpeg python3 sha256sum tc; do command -v "$tool" > /dev/null || fail "no $tool"; done
DEV=$(ip route get 1.1.1.1 | awk '{for (i = 1; i < NF; i++) if ($i == "dev") print $(i + 1)}')
[ -n "$DEV" ] && [ -r "/sys/class/net/$DEV/statistics/tx_bytes" ] || fail "no uplink device"
# Encryption's filesystem driver and its provider, as user-flow.sh puts them on a runner.
ROOT=$(cd "$(dirname "$0")/../.." && pwd)
bash "$ROOT/e2e/cli/install-vault-driver.sh" fuse3 > "$OUT/fuse.log" 2>&1 || fail "no FUSE: $(tail -c 300 "$OUT/fuse.log")"
SECUREFS=$HOME/.cozy/tools/securefs/$(jq -r .version "$ROOT/crates/project-mount/helper/manifest.json")
if [ ! -x "$SECUREFS/securefs" ]; then
  archive=$(python3 -I "$ROOT/scripts/prepare-securefs.py" --target x86_64-unknown-linux-gnu --out-dir "$RUNNER_TEMP/securefs") || fail "no securefs"
  mkdir -p "$SECUREFS" && chmod 700 "$SECUREFS" &&
    python3 -I -c 'import sys, zipfile; open(sys.argv[2], "wb").write(zipfile.ZipFile(sys.argv[1]).read("securefs"))' "$archive" "$SECUREFS/securefs" &&
    chmod 755 "$SECUREFS/securefs" || fail "securefs not put in $SECUREFS"
fi
# A dev home, its first-party packages from the checkout; Calcifer started again on it.
cfg=$HOME/.cozy/config.toml
mkdir -p "${cfg%/*}" && touch "$cfg" || fail "no $cfg"
{ echo 'dev = true'; grep -v '^dev *=' "$cfg" || true; } > "$cfg.new" && mv "$cfg.new" "$cfg" || fail "dev = true not written"
awk -v source="$SOURCE" '
  function ours() { print "diagnostics = true"; print "packages = \"" source "\""; put = 1 }
  /^(diagnostics|packages) *=/ { next } { print } /^\[calcifer\]/ { ours() } END { if (!put) { printf "\n[calcifer]\n"; ours() } }' \
  "$cfg" > "$cfg.next" && mv "$cfg.next" "$cfg" || fail "[calcifer] not written"
cozy restart > "$OUT/restart.txt" 2>&1 || fail "cozy restart: $(tail -c 300 "$OUT/restart.txt")"
kernels "$SOURCE/qwen-image-2"
# The encrypted project: its passphrase and recovery secret outside it and out of what's published.
PASS=$RUNNER_TEMP/carry-ahead-pass
head -c 24 /dev/urandom | base64 | tr -d '\n' > "$PASS" && chmod 600 "$PASS" || fail "no passphrase"
cozy --json new "$OUT/sealed" --passphrase-file "$PASS" --recovery-out "$RUNNER_TEMP/carry-ahead-recovery" > "$OUT/sealed.json" 2>&1 ||
  fail "cozy new (encrypted): $(tail -c 300 "$OUT/sealed.json")"
cozy --json vault --directory "$OUT/sealed" lock > "$OUT/lock.json" 2>&1 || fail "vault lock: $(tail -c 300 "$OUT/lock.json")"
cozy --json vault --directory "$OUT/sealed" unlock --passphrase-file "$PASS" --idle 0 > "$OUT/unlock.json" 2>&1 ||
  fail "vault unlock: $(tail -c 300 "$OUT/unlock.json")"
cozy --json vault --directory "$OUT/sealed" status > "$OUT/vault.json" 2>&1 || fail "vault status: $(tail -c 300 "$OUT/vault.json")"
jq -e '.encrypted == true and .state == "unlocked"' "$OUT/vault.json" > /dev/null || fail "the project isn't encrypted and unlocked: $(cat "$OUT/vault.json")"
RUN_PROJECT=$(cd "$OUT/sealed" && pwd)
noise() { nice -n 19 ffmpeg -v error -y -f lavfi -i "nullsrc=s=$1:d=1,format=rgb24,geq=r='random(1)*255':g='random(2)*255':b='random(3)*255'" -frames:v 1 "$2"; }
noise 2048x2048 "$OUT/photo.png" && noise 4096x4096 "$OUT/wide.png" || fail "no pictures"
for box in photo wide; do
  run_page import "$OUT/$box.png" --name "$box" > "$OUT/$box.json" 2>&1 || fail "import $box: $(tail -c 300 "$OUT/$box.json")"
done
PHOTO=sha256:$(sha256sum "$OUT/photo.png" | cut -c1-64) WIDE=sha256:$(sha256sum "$OUT/wide.png" | cut -c1-64)
WIDE_B=$(stat -c %s "$OUT/wide.png")
say "cozy $V, a dev home, an encrypted project ($(grep -o '"id": *"[^"]*"' "$OUT/sealed.json" | head -1)); photo $(stat -c %s "$OUT/photo.png") bytes ($PHOTO), wide $WIDE_B bytes ($WIDE)"

shown() { cozy --json machine show "$NAME" > "$1" 2> /dev/null || true; }
whole() { jq -e --arg m "$2" '[.status.models[]? | select(.repository == $m) | .checkpoints[]?.complete] | any' "$1" > /dev/null 2>&1; }
record() {
  local n
  n=$(run_page asset "$1" | jq -r '.versions[-1].runs[-1] // empty')
  [ -n "$n" ] && (cd "$RUN_PROJECT" && cozy --json --full run watch "$n" --once) > "$2" 2>&1 &&
    (cd "$RUN_PROJECT" && cozy --full run watch "$n" --once) > "${2%.json}.txt" 2>&1
}
row() { run_page board | jq -c --arg n "$1" '[.ahead[]? | select((.names // []) | index($n))] | max_by(.run) // empty' 2> /dev/null; }
version() { cozy --json machine show "$NAME" 2> /dev/null | jq -r '.status.version // empty'; }
ours() {
  local tries=0 runs
  step rc machine update "$NAME" -y || fail "$NAME didn't move to the open RC's builds"
  while runs=$(version); [ "$runs" != "$V" ]; do
    [ "$tries" -lt 3 ] || fail "$NAME doesn't keep the session's TensorD $V: it runs ${runs:-nothing it says}"
    step tensord machine update "$NAME" --tensord "$(command -v cozy)" -y || fail "$NAME refused the session's TensorD"
    sleep 30; tries=$((tries + 1))
  done
  say "$NAME runs the session's TensorD $V"
}
# kept <what>: $NAME still runs the session's TensorD; one moved off it is put back.
kept() {
  local runs
  runs=$(version)
  [ "$runs" != "$V" ] || return 0
  say "$1: $NAME runs ${runs:-nothing it says}, not the session's $V: updated again"
  step "tensord-$1" machine update "$NAME" --tensord "$(command -v cozy)" -y || return 1
  sleep 30
  [ "$(version)" = "$V" ]
}
PAGE=''
# unshown: the page closed, and the project's guesses with it.
unshown() { [ -z "$PAGE" ] || kill "$PAGE" 2> /dev/null; PAGE=''; }
page() {
  local addr id stream='' headers
  unshown
  addr=$(jq -r .address "$HOME/.cozy/calcifer/calcifer.lock.holder")
  headers=(-H "Host: $addr" -H 'Sec-Fetch-Site: same-origin' -H 'Sec-Fetch-Mode: cors' -H 'Sec-Fetch-Dest: empty' -H 'cozy-project-storage: 3')
  curl -sSN "${headers[@]}" -H 'Accept: text/event-stream' "http://$addr/v1/events" > "$OUT/page.sse" 2>&1 & PAGE=$!; BG="$BG $PAGE"
  for _ in $(seq 100); do
    stream=$(sed -n 's/^data: //p' "$OUT/page.sse" | jq -r 'select(.type == "hello") | .stream' 2> /dev/null | head -1); [ -n "$stream" ] && break; sleep 0.1
  done
  [ -n "$stream" ] || fail "the page's stream said no hello: $(head -c 300 "$OUT/page.sse")"
  id=$(python3 -c 'import sys; sys.path.insert(0, sys.argv[1]); import page; print(page.read(sys.argv[2])["id"])' "$(dirname "$RUN_PAGE")" "$RUN_PROJECT" 2> "$OUT/id.err")
  [ -n "$id" ] || fail "the project's id: $(tail -c 300 "$OUT/id.err")"
  curl -fsS -X PUT "${headers[@]}" -H "Origin: http://$addr" -H 'Content-Type: application/json' -d "$(jq -nc --arg id "$id" '{projects: [$id]}')" \
    "http://$addr/v1/events/$stream" > "$OUT/show.json" 2>&1 || fail "the page couldn't show the project: $(tail -c 300 "$OUT/show.json")"
}
logs() {
  cozy machine logs "$NAME" --since 2h -n 0 > "$OUT/tensord-$NAME.log" 2>&1 || true
  cozy machine logs local calcifer --lines 20000 > "$OUT/calcifer.log" 2>&1 || true
}
# landed <digest> [from ms] [to ms]: the times (Unix ms, the pod's clock) TensorD's log says <digest> landed whole in parts.
landed() {
  python3 -I - "$OUT/tensord-$NAME.log" "$1" "${2:-0}" "${3:-99999999999999}" <<'PY'
import datetime, sys
log, digest, since, until = sys.argv[1], sys.argv[2], int(sys.argv[3]), int(sys.argv[4])
for line in open(log, errors="replace"):
    stamp = line.split(" ", 1)[0]
    if stamp[:4].isdigit() and f"objects: {digest} landed whole" in line:
        at = int(datetime.datetime.fromisoformat(stamp.replace("Z", "+00:00")).timestamp() * 1000)
        if since <= at <= until:
            print(at)
PY
}
# objects <from ms> [to ms]: what TensorD's log says landed on $NAME in that time (Unix ms, the pod's clock): each `objects:` line.
objects() {
  python3 -I - "$OUT/tensord-$NAME.log" "$1" "${2:-99999999999999}" <<'PY'
import datetime, sys
log, since, until = sys.argv[1], int(sys.argv[2]), int(sys.argv[3])
for line in open(log, errors="replace"):
    stamp = line.split(" ", 1)[0]
    if stamp[:4].isdigit() and " objects: " in line and " landed " in line:
        at = int(datetime.datetime.fromisoformat(stamp.replace("Z", "+00:00")).timestamp() * 1000)
        if since <= at <= until:
            print(stamp[11:23], line.split(" objects: ", 1)[1].strip()[:160])
PY
}
# sample: $NAME's Status as Calcifer relays it, each second: each predicted request's stage (status.ndjson).
sample() {
  ( while :; do
      printf '{"at_ms":%s,"status":%s}\n' "$(date +%s%3N)" \
        "$(cozy --json machine show "$NAME" 2> /dev/null | jq -c '{ahead: [(.status.ahead // [])[] | {id, state, stage: (.readiness.stage // "")}]}' 2> /dev/null || echo null)" >> "$OUT/status.ndjson"
      sleep 1
    done ) & BG="$BG $!"
}
# stages: each predicted request's stage on $NAME, as its Status last said.
stages() { tail -n 1 "$OUT/status.ndjson" 2> /dev/null | jq -r '[.status.ahead[]? | select(.id | startswith("cozy:prepare:")) | .stage] | join(" ")' 2> /dev/null; }
# staged <stage> <s>: within <s> s, a predicted request on $NAME is at <stage>.
staged() {
  local began=$SECONDS now
  while [ $((SECONDS - began)) -lt "$2" ]; do
    now=$(stages)
    if [[ " $now " == *" $1 "* ]]; then say "a predicted request on $NAME is $1 ($((SECONDS - began)) s; all of them: $now)"; return 0; fi
    sleep 0.5
  done
  return 1
}
# yielded <from ms>: since then, each call TensorD's log says waited for a free GPU and how long until it went, then what a load
# ahead said of giving its GPU up.
yielded() {
  python3 -I - "$OUT/tensord-$NAME.log" "$1" <<'PY'
import datetime, sys
since = int(sys.argv[2])
def at(line):
    try:
        return datetime.datetime.fromisoformat(line.split(" ", 1)[0].replace("Z", "+00:00")).timestamp() * 1000
    except ValueError:
        return 0.0
lines = [line.rstrip("\n") for line in open(sys.argv[1], errors="replace") if line[:4].isdigit()]
lines = [line for line in lines if at(line) >= since - 2000]
for wait in (line for line in lines if "queue: waiting for a free GPU" in line):
    went = [line for line in lines if at(line) >= at(wait) and "queue: goes after" in line and "free GPU" in line][:1]
    if went:
        print(f"a call waited for the GPU at {wait.split(' ', 1)[0][11:26]} and went {at(went[0]) - at(wait):.1f} ms later")
for line in lines:
    if "load ahead: ended for a run" in line or "load ahead: its GPU went to a run" in line:
        print(line.split(" ", 1)[0][11:26], "load ahead: " + line.split("load ahead: ", 1)[1][:200])
PY
}
edit() {
  local written
  run_page set "$1" --tool qwen-image-2/image --fields '{}' > "$OUT/$1-attach.json" 2>&1 || fail "Qwen on $1: $(tail -c 300 "$OUT/$1-attach.json")"
  written=$(run_page asset "$1" | jq -r '.asset.fields.instructions // empty')
  [[ $written == "<self."* ]] || fail "$1's instructions don't start from its picture: '$written'"
  run_page set "$1" --fields "$(jq -nc --arg w "$written The same scene at night, under a full moon." '{instructions: $w, megapixels: 1}')" > "$OUT/$1-edit.json" 2>&1 ||
    fail "$1's edit: $(tail -c 300 "$OUT/$1-edit.json")"
}
# carried <box> <digest> <s>: Calcifer says <box>'s guess carried its picture ahead to $NAME and TensorD that it landed, within <s>.
carried() {
  local began=$SECONDS
  while [ $((SECONDS - began)) -lt "$3" ]; do
    logs
    if grep -q "$1's guess carried 1 files .* ahead to $NAME .*, an encrypted project's" "$OUT/calcifer.log" && [ -n "$(landed "$2")" ]; then
      say "$1: $(grep -o "$1's guess carried 1 files .* ahead to $NAME .*" "$OUT/calcifer.log" | tail -1); TensorD: landed at $(landed "$2" | paste -sd, -) (Unix ms)"
      return 0
    fi
    sleep 2
  done
  fail "$1's picture wasn't carried ahead to $NAME in $3 s: $(grep -E "$1|goes ahead" "$OUT/calcifer.log" | tail -5)"
}
MISMATCH=() ADOPTED=''
# adopted: what TensorD's log last said of a run naming a predicted request.
adopted() { grep -E "run run-[0-9a-f]+ (is the request|names)" "$OUT/tensord-$NAME.log" | tail -1 | sed 's/.*INFO tensord::ahead: //'; }
# rendered <box> <digest> [&]: rendered on $NAME, it reused its picture there, the machine landed none of it while it was sent, and
# it adopted its item.
rendered() {
  local began ended took
  began=$(date +%s%3N)
  timeout 3000 python3 "$RUN_PAGE" render "$RUN_PROJECT" --on "$NAME" "$1" > "$OUT/$1.txt" 2>&1 || fail "$1: $(tail -c 400 "$OUT/$1.txt")"
  ended=$(date +%s%3N)
  record "$1" "$OUT/$1-run.json" || fail "$1 has no run record: $(tail -c 300 "$OUT/$1.txt")"
  logs
  grep -ho "reused [^\"]* already on $NAME" "$OUT/$1-run.json" "$OUT/$1-run.txt" | head -1 > "$OUT/$1-reused.txt"
  say "$1: rendered in $((ended - began)) ms; its run: $(cat "$OUT/$1-reused.txt")"
  [ -s "$OUT/$1-reused.txt" ] || MISMATCH+=("$1's run didn't find its picture on $NAME")
  [ -z "$(landed "$2" "$began" "$ended")" ] || MISMATCH+=("$NAME landed $1's picture again while it rendered")
  took=$(adopted)
  say "$1's run on $NAME: ${took:-named no predicted request}"
  [[ $took == *"is the request"*"takes what it made ready"* && $took != "$ADOPTED" ]] || MISMATCH+=("$1's render didn't adopt its item: ${took:-it named none}")
  ADOPTED=$took
}
# own_ahead: step 1. A page shows the project, and the box of its own code is guessed onto $NAME: its code carried there at the guess,
# its item ready, and its render adopting the item. What doesn't match is said at the end; the session goes on.
own_ahead() {
  local at now got='' began ended took landed_n
  at=$(date +%s%3N)
  page
  for _ in $(seq 150); do
    logs; got=$(row own)
    grep -q "own guessed on $NAME" "$OUT/calcifer.log" && jq -e --arg m "$NAME" '.ready == true and .machine == $m' <<< "${got:-null}" > /dev/null 2>&1 && break
    sleep 2
  done
  now=$(date +%s%3N)
  if ! grep -q "own guessed on $NAME" "$OUT/calcifer.log"; then
    MISMATCH+=("own wasn't guessed on $NAME: $(grep -E "own('s| isn't| guessed)" "$OUT/calcifer.log" | tail -2 | cut -c1-300)")
    return 0
  fi
  say "own: $(grep -o "own guessed on $NAME in [0-9]* ms.*" "$OUT/calcifer.log" | head -1 | cut -c1-260)"
  grep -o "[0-9]* objects ([^)]*) landed in one request in [0-9]* ms" "$OUT/calcifer.log" | tail -1 | sed 's/^/  Calcifer: /' | tee -a "$R"
  objects $((at - 2000)) "$now" > "$OUT/own-objects.txt"
  landed_n=$(wc -l < "$OUT/own-objects.txt")
  sed "s/^/  $NAME: objects: /" "$OUT/own-objects.txt" | tee -a "$R"
  [ "$landed_n" -ge 1 ] || MISMATCH+=("$NAME's log lands nothing of own's code at its guess")
  if jq -e --arg m "$NAME" '.ready == true and .machine == $m' <<< "${got:-null}" > /dev/null 2>&1; then
    say "own ready on $NAME $((now - at)) ms after the page showed it: $got"
  else
    MISMATCH+=("own's item wasn't ready on $NAME in 300 s: ${got:-no row}")
  fi
  began=$(date +%s%3N)
  if ! timeout 1800 python3 "$RUN_PAGE" render "$RUN_PROJECT" --on "$NAME" own > "$OUT/own.txt" 2>&1; then
    MISMATCH+=("own's render: $(tail -c 300 "$OUT/own.txt")")
    return 0
  fi
  ended=$(date +%s%3N)
  record own "$OUT/own-run.json" || true
  logs
  took=$(adopted)
  say "own: rendered in $((ended - began)) ms; $NAME landed $(objects "$began" "$ended" | wc -l) object writes meanwhile; its run there: ${took:-named no predicted request}"
  [[ $took == *"is the request"*"takes what it made ready"* ]] || MISMATCH+=("own's render didn't adopt its item: ${took:-it named none}")
  ADOPTED=$took
}
# yield_arm: step 5. $OTHER's weights onto $NAME by one call of it, then TensorD restarted and $OTHER sent while a predicted request
# loads: how long it waited for the GPU, by TensorD's log. Said, never judged; three tries to meet a load.
YIELDED=''
yield_arm() {
  local began=$SECONDS try sent code
  (cd "$RUN_PROJECT" && timeout 900 cozy run "$OTHER" --on "$NAME" prompt="a brass compass on a walnut desk" seed:=4) > "$OUT/other-fetch.txt" 2>&1; code=$?
  case $code in
    0) say "$OTHER's first call, its weights fetched: $((SECONDS - began)) s" ;;
    124) say "step 5 skipped: $OTHER's first call and its fetch took over 900 s"; return 0 ;;
    *) say "step 5 skipped: $OTHER's first call failed: $(tail -c 300 "$OUT/other-fetch.txt")"; return 0 ;;
  esac
  for try in 1 2 3; do
    step "restart-$try" machine update "$NAME" --mode dedicated -y || { say "step 5 ended: the restart failed: $(tail -c 300 "$OUT/restart-$try.txt")"; return 0; }
    kept "restart-$try" || { say "step 5 ended: $NAME didn't keep the session's TensorD"; return 0; }
    staged loading 300 || { say "step 5 ended: no predicted request was loading within 300 s of the restart: $(stages)"; return 0; }
    sent=$(date +%s%3N)
    (cd "$RUN_PROJECT" && timeout 600 cozy run "$OTHER" --on "$NAME" prompt="a brass compass on a walnut desk" seed:=$((4 + try))) > "$OUT/other-$try.txt" 2>&1 ||
      { say "step 5 ended: $OTHER's call failed: $(tail -c 300 "$OUT/other-$try.txt")"; return 0; }
    logs
    yielded "$sent" > "$OUT/yield-$try.txt"
    YIELDED=$(grep -m1 "went " "$OUT/yield-$try.txt")
    [ -z "$YIELDED" ] || break
    say "step 5, try $try: $OTHER went with no wait for the GPU (the load had ended before it came)"
  done
  if [ -z "$YIELDED" ]; then say "step 5: $OTHER met no load in 3 tries: no yield measured"; return 0; fi
  say "yield: $YIELDED"
  grep -v "went " "$OUT/yield-$try.txt" | sed "s/^/  $NAME: /" | tee -a "$R"
  if staged loaded 300; then say "after $OTHER, a predicted request is loaded again"; else say "after $OTHER, no predicted request was loaded again within 300 s: $(stages)"; fi
}

# The project's own code in its readable view, and `own`, a box of it. This computer is its machine for now: a page shows the project,
# and Calcifer says the code stays in its vault here. Then the page closes, before anything is rented.
VIEW=$(jq -r '.view // empty' "$OUT/vault.json")
[ -n "$VIEW" ] && [ -d "$VIEW" ] || fail "the project has no readable view: $(head -c 300 "$OUT/vault.json")"
cp -r "$ROOT/e2e/cli/project/boxed" "$VIEW/boxed" || fail "the project's own code wasn't put in its view"
(cd "$RUN_PROJECT" && cozy --json machine use local) > "$OUT/use-local.json" 2>&1 || fail "machine use local: $(tail -c 300 "$OUT/use-local.json")"
run_page add own --tool ./boxed/plain --fields '{"instructions": "a still, a shade lighter"}' > "$OUT/own-box.json" 2>&1 || fail "own's box: $(tail -c 300 "$OUT/own-box.json")"
page
STAYS="own isn't guessed: its code is this encrypted project's own, which stays in its vault on this computer"
for _ in $(seq 90); do
  cozy machine logs local calcifer --lines 20000 > "$OUT/calcifer.log" 2>&1 || true
  grep -q "$STAYS" "$OUT/calcifer.log" && break
  sleep 2
done
grep -q "$STAYS" "$OUT/calcifer.log" || fail "on this computer, own's guess: $(grep -E "own('s| isn't| guessed)|no box is guessed" "$OUT/calcifer.log" | tail -4 | cut -c1-600)"
say "on this computer: $STAYS ($(find "$VIEW/boxed" -type f | wc -l) files of its own code in its view)"
unshown
quote "$(param need_s 2400)"

rent
RENTED=$SECONDS
ours
(cd "$RUN_PROJECT" && cozy --json machine use "$NAME") > "$OUT/use.json" 2>&1 || fail "machine use $NAME: $(tail -c 300 "$OUT/use.json")"
began=$SECONDS
(cd "$RUN_PROJECT" && cozy --json package install "$SOURCE/qwen-image-2" --on "$NAME") > "$OUT/install.json" 2>&1 ||
  fail "qwen-image-2 not installed on $NAME: $(tail -c 300 "$OUT/install.json")"
say "step 0: qwen-image-2 installed on $NAME in $((SECONDS - began)) s"
sample
say "step 1: a page shows the encrypted project; own, a box of its own code, is guessed onto $NAME"
own_ahead
say "step 2: photo gets Qwen Image 2.1, then an edit"
kept photo || fail "$NAME doesn't keep the session's TensorD $V"
edit photo
carried photo "$PHOTO" 180
# Its models too, so its render waits for nothing.
for _ in $(seq 900); do
  shown "$OUT/photo-ready.json"; got=$(row photo)
  whole "$OUT/photo-ready.json" "$I2I" && jq -e --arg m "$NAME" '.ready == true and .machine == $m' <<< "${got:-null}" > /dev/null 2>&1 && break
  sleep 2
done
jq -e --arg m "$NAME" '.ready == true and .machine == $m' <<< "${got:-null}" > /dev/null 2>&1 || fail "photo isn't ready on $NAME after 30 min: ${got:-no row}"
say "photo ready on $NAME: $got"
# Its request loaded, and left there: its draft stands unchanged well past the 3 s a machine told to run ahead waits.
if staged loaded 600; then
  sleep 20
  say "20 s on, photo's draft unchanged, the predicted requests on $NAME are: $(stages)"
  [[ " $(stages) " == *" loaded "* ]] || MISMATCH+=("photo's request didn't stay loaded: $(stages)")
else
  MISMATCH+=("photo's request wasn't loaded on $NAME in 600 s: $(stages)")
fi
say "step 3: the uplink shaped to $MBIT Mbit/s; wide gets Qwen and an edit, and photo is rendered while wide's picture goes ahead"
sudo tc qdisc add dev "$DEV" root tbf rate "${MBIT}mbit" burst 64kb latency 400ms || fail "the uplink couldn't be shaped"
TX0=$(cat "/sys/class/net/$DEV/statistics/tx_bytes")
edit wide
for _ in $(seq 120); do [ $(($(cat "/sys/class/net/$DEV/statistics/tx_bytes") - TX0)) -ge $((4 << 20)) ] && break; sleep 0.5; done
say "wide's picture is going ahead: $(($(cat "/sys/class/net/$DEV/statistics/tx_bytes") - TX0)) bytes sent since its edit"
rendered photo "$PHOTO"
carried wide "$WIDE" $((WIDE_B * 8 / (MBIT * 1000000) * 4 + 120))
TX=$(($(cat "/sys/class/net/$DEV/statistics/tx_bytes") - TX0))
sudo tc qdisc del dev "$DEV" root || fail "the uplink shaping wasn't removed"
waits=$(grep -c "what goes ahead to $NAME waits: a run's inputs go first" "$OUT/calcifer.log")
say "wide: the carry waited for a run's $waits times; it landed $(landed "$WIDE" | wc -l) times; the runner sent $TX bytes for its $WIDE_B"
[ "$waits" -ge 1 ] || MISMATCH+=("wide's carry never waited for photo's render")
[ "$(landed "$WIDE" | wc -l)" = 1 ] || MISMATCH+=("wide's picture landed $(landed "$WIDE" | wc -l) times")
[ "$TX" -le $((WIDE_B * 13 / 10)) ] || MISMATCH+=("the runner sent $TX bytes while wide's $WIDE_B went ahead")
say "step 4: wide rendered"
rendered wide "$WIDE"
say "Calcifer's carries ahead: $(grep -o "[a-z]*'s guess carried .*" "$OUT/calcifer.log" | paste -sd ';' -)"
# Nothing ran ahead: no speculative run in the machine's log, and no predicted request its Status ever said was running or done.
ran_log=$(grep -acE "runs as if it were sent|predicted requests run ahead here|speculative run" "$OUT/tensord-$NAME.log")
ran_status=$(jq -R -r 'fromjson? | .status.ahead[]? | select(.stage == "running" or .stage == "done") | .id' "$OUT/status.ndjson" 2> /dev/null | sort -u | wc -l)
were_loaded=$(jq -R -r 'fromjson? | .status.ahead[]? | select(.stage == "loaded") | .id' "$OUT/status.ndjson" 2> /dev/null | sort -u | wc -l)
say "nothing ran ahead: $NAME's log names $ran_log speculative runs; its Status said running or done for $ran_status predicted requests, loaded for $were_loaded"
[ "$ran_log" = 0 ] && [ "$ran_status" = 0 ] || MISMATCH+=("a predicted request ran ahead: $ran_log in $NAME's log, $ran_status in its Status")
(cd "$RUN_PROJECT" && cozy --json run ls --all --limit 50) > "$OUT/runs.json" 2>&1 || true
say "the project's runs: $(jq -r '[.runs[]? | "\(.number) \(.target // "")"] | join("; ")' "$OUT/runs.json" 2> /dev/null | cut -c1-400)"
if [ ${#MISMATCH[@]} = 0 ]; then say "steps 1 to 4 matched"; else say "steps 1 to 4, what didn't match: ${MISMATCH[*]}"; fi
if [ "$YIELD" = true ]; then
  left=$((${CAP_S:-0} - (SECONDS - RENTED)))
  if [ "$left" -lt 1500 ]; then
    say "step 5 skipped: the cap buys about $left s more, under the 1500 s $OTHER's fetch and three restarts may take"
  else
    say "step 5: $OTHER's weights onto $NAME, then it is sent while a predicted request loads (the cap buys about $left s more)"
    yield_arm
  fi
fi
logs
say "$NAME ends on TensorD $(version)"
[ ${#MISMATCH[@]} = 0 ] || fail "what didn't match: ${MISMATCH[*]}"
say "PASS: an encrypted project's own code and its pictures went ahead to the rental, the pictures behind a render's carry and each landing once; each render found what it reads there and adopted its item; nothing ran ahead${YIELDED:+; $OTHER, sent while a load held the GPU: $YIELDED}"
