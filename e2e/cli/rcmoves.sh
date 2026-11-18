# rc_moves (cl-1201): a dev home's moves to the open RC never hold up work. Asked by the release coordinator, from Predictive
# Downloads' #3784 proof (cl-1123): "a move must never hold up a render", and "software moves happen at rent/boot, never inside a
# request". Before, a submit to a rental not on the RC fetched the RC's cozy from GitHub inside the request (49 s on Paul's laptop)
# and moved the rental first; and a render that came while the follower's move was being sent waited for it and its restart.
# Two stood-in rentals of a dev home (`cozy machine serve --rental` on this runner, its stood-in Hub granting this computer's key,
# as holdwins.sh's), each booting from the newest release's image, so behind the RC. As on the laptop, GitHub answers this
# computer's Calcifer at about 2 MB/s (an HTTPS proxy, rcmoves/slow.py) and each rental takes what Calcifer sends it at about 2 MB/s
# (a relay in front of it). Linux, a GitHub runner; its own shard.
#   rc_request   a first submit, as the rental becomes ready: its run is made and accepted at once, on what the rental runs; no
#                move is made or fetched for it first.
#   rc_yields    a render submitted while the follower's move is being sent: it is accepted at once (no 5–15 s hold, measured),
#                the move yields, and the rental moves to the RC once it is idle.
#   rc_held      (cl-1236, shard rc-held by dispatch) a render held behind a third rental's update by hand (this cozy's image, to
#                the RC's cozy) keeps it through the update's restart past its idle deadline.
here_rcmoves=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
RM_DIR='' RM_CFG='' RM_SAVED='' RM_GH='' RM_KEY='' RM_DROPIN=$HOME/.config/systemd/user/cozy-calcifer.service.d/cl1201-slow-github.conf
# Each connection's bytes a second, through either slow link: 4 upload lanes to a rental make about 2 MB/s.
RM_UPLINK=524288 RM_GITHUB=2097152

rc_moves() {
step "a dev home's moves to the open RC never hold up work: none from a request, and a pending one yields to a render (cl-1201)"
[ "${RUNNER_OS:-}" = Linux ] && [ "${GITHUB_ACTIONS:-}" = true ] || { echo "rc moves: a fresh GitHub Linux runner only"; return 0; }
RM_DIR=$OUT/rcmoves RM_CFG=$HOME/.cozy/config.toml RM_SAVED=$OUT/rcmoves-config.toml
cp "$RM_CFG" "$RM_SAVED" || { check "rc moves: no config.toml to keep"; return; }
rm_setup && { rc_request; rc_yields; }
rm_quit
}

# rc_held alone, by dispatch (shard rc-held): its timing on a runner isn't settled yet (cl-1236), so it holds no RC run's shard.
rc_held_alone() {
[ "${RUNNER_OS:-}" = Linux ] && [ "${GITHUB_ACTIONS:-}" = true ] || { echo "rc held: a fresh GitHub Linux runner only"; return 0; }
RM_DIR=$OUT/rcmoves RM_CFG=$HOME/.cozy/config.toml RM_SAVED=$OUT/rcmoves-config.toml
cp "$RM_CFG" "$RM_SAVED" || { check "rc held: no config.toml to keep"; return; }
rm_setup && rc_held
rm_quit
}

rm_setup() {
local v
v=$(cozy --version | awk '{print $2}')
[[ $v == *-dev+g* ]] || { check "rc moves: cozy $v is no RC build, with no RC to follow"; return 1; }
rm_rentals stand-a stand-b || return 1
{ echo 'dev = true'; grep -v '^dev *=' "$RM_SAVED"; } > "$RM_CFG"
RM_GH=$(rm_listen "$RM_DIR/github" connect "$RM_GITHUB" github.com githubusercontent.com) || { check "rc moves: the slow GitHub didn't start"; return 1; }
mkdir -p "${RM_DROPIN%/*}" && printf '[Service]\nEnvironment=HTTPS_PROXY=%s\nEnvironment=https_proxy=%s\nEnvironment=NO_PROXY=%s\nEnvironment=no_proxy=%s\n' \
  "http://127.0.0.1:$RM_GH" "http://127.0.0.1:$RM_GH" 127.0.0.1,localhost,::1 127.0.0.1,localhost,::1 > "$RM_DROPIN" \
  && systemctl --user daemon-reload || { check "rc moves: Calcifer's slow GitHub wasn't set"; return 1; }
rm_start
}

# rm_rentals <name>...: the newest release's image, and each stood-in rental of it, recorded as a rental this computer ordered, down.
# Calcifer is left stopped (rm_start).
rm_rentals() {
local v runtime tensorfs released name
v=$(cozy --version | awk '{print $2}')
runtime=$(curl -fsSL --retry 3 https://pypi.org/pypi/cozy-runtime/json | jq -r '.info.version // empty')
tensorfs=$(curl -fsSL --retry 3 https://pypi.org/pypi/tensorfs/json | jq -r '.info.version // empty')
released=$(curl -fsSL --retry 3 https://github.com/cozy-creator/cozy-creator/releases/latest/download/dist-manifest.json \
  | jq -r '.releases[] | select(.app_name == "cozy") | .app_version')
[ -n "$runtime" ] && [ -n "$tensorfs" ] && [ -n "$released" ] \
  || { check "rc moves: the newest releases: Runtime ${runtime:-?}, TensorFS ${tensorfs:-?}, cozy ${released:-?}"; return 1; }
RM_KEY=$(cozy --json machine show local | jq -r '.controller_key // empty')
[ -n "$RM_KEY" ] || { check "rc moves: machine show local names no controller key"; return 1; }
rm -rf "$RM_DIR" && mkdir -p "$RM_DIR/image/bin" "$RM_DIR/image/wheels" "$RM_DIR/x" || { check "rc moves: $RM_DIR not made"; return 1; }
rm_image "$released" cozy-runtime "$runtime" tensorfs "$tensorfs" || return 1
echo "rc moves: each rental's image: cozy $released, Runtime $runtime, TensorFS $tensorfs; this computer's cozy $v"

# Added once each for its address (its relay) and fingerprint, then recorded as a rental this computer ordered, with it down.
for name in "$@"; do rm_rental "$name" || return 1; done
systemctl --user stop cozy-calcifer || { check "rc moves: Calcifer didn't stop"; return 1; }
for name in "$@"; do
  rm_store "UPDATE machines SET kind = 'rental', rental_id = 'rental-$name', hub = 'http://127.0.0.1:1' WHERE name = '$name'" \
    || { check "rc moves: $name wasn't recorded as a rental"; return 1; }
done
}

# rc_request (cl-1201, the release coordinator: "A first runs/submit to a rental not yet on the RC downloads the RC's cozy inside
# the request. That download belongs at rent/boot, off the request path"): stand-a boots; as soon as Calcifer says it's ready, a
# run is asked of it. The run is made and accepted at once, on the image's software: nothing waited for GitHub or for a move.
rc_request() {
local d=$RM_DIR/stand-a t0 code=0 made accepted
rm_serve stand-a
for _ in $(seq 360); do rm_show stand-a > "$d/show-ready.json" && J '.status.phase == "ready"' "$d/show-ready.json" && break; sleep 0.5; done
J '.status.phase == "ready"' "$d/show-ready.json" || { check "rc request: stand-a wasn't ready in 3 min: $(tail -c 600 "$d/machine.log")"; return; }
t0=$(date +%s%3N)
(cd "$RUN_PROJECT" && timeout 900 "$COZY" --json run "$here_rcmoves/weightless/reverse" "text=first$RANDOM" --on stand-a) > "$d/first.ndjson" 2> "$d/first.err" || code=$?
rm_runs stand-a "$d/runs.json"
echo "rc request: stand-a's runs: $(jq -c '[.runs | sort_by(.number)[] | {number, name, kind, status, target: .target[0:60], created: (.created_at_ms - '"$t0"'), accepted: ((.accepted_at_ms // 0) - '"$t0"'), reason: .reason.code}]' "$d/runs.json")"
[ "$code" = 0 ] || check "rc request: cozy run on stand-a exited $code: $(tail -c 400 "$d/first.err")"
made=$(jq -r '[.runs[] | select(.kind != "update")] | min_by(.created_at_ms) | .created_at_ms // empty' "$d/runs.json")
accepted=$(jq -r '[.runs[] | select(.kind != "update")] | min_by(.created_at_ms) | .accepted_at_ms // empty' "$d/runs.json")
[ -n "$made" ] && [ -n "$accepted" ] || { check "rc request: no run of stand-a's was made and accepted: $(head -c 600 "$d/runs.json")"; return; }
echo "rc request: asked as stand-a was ready; its first run was made $((made - t0)) ms later and accepted $((accepted - made)) ms after that"
[ $((made - t0)) -lt 5000 ] || check "rc request: its first run was made $((made - t0)) ms after it was asked: the request waited (for GitHub, or a move)"
[ $((accepted - made)) -lt 5000 ] || check "rc request: its first run was accepted $((accepted - made)) ms after it was made: it waited for a move"
J --argjson at "$accepted" '[.runs[] | select(.kind == "update" and .created_at_ms < $at and .reason.code != "yielded")] | length == 0' "$d/runs.json" \
  || check "rc request: stand-a moved before its run went: $(jq -c --argjson at "$accepted" '[.runs[] | select(.kind == "update" and .created_at_ms < $at) | {number, status, target}]' "$d/runs.json")"
rm_down stand-a
}

# rc_yields (cl-1201, the release coordinator: "A follower's own RC move reaching a pod just before a render still holds the render
# behind the restart for about 5–15 s. The rule: a move never starts once a render is queued for, or being sent to, that machine"):
# stand-b boots idle and the follower makes its move to the RC; while that is being sent, a render is asked of it. The render is
# accepted at once, the move yields, and stand-b moves to the RC once it's idle again.
rc_yields() {
local d=$RM_DIR/stand-b t0 code=0 move='' made accepted
rm_serve stand-b
for _ in $(seq 480); do
  rm_runs stand-b "$d/runs-move.json"
  move=$(jq -r '[.runs[]? | select(.kind == "update" and (.target | test("\\(RC\\)")) and .status == "queued")] | first | .id // empty' "$d/runs-move.json" 2> /dev/null)
  [ -n "$move" ] && break
  sleep 0.5
done
[ -n "$move" ] || { check "rc yields: no move to the RC was pending at stand-b's boot in 4 min: $(head -c 400 "$d/runs-move.json") $(rm_show stand-b | jq -c '{state, rc, status: (.status // {} | {phase, version})}')"; return; }
echo "rc yields: at stand-b's boot the follower made $move, being sent: $(jq -c --arg m "$move" '.runs[] | select(.id == $m) | {number, status, waiting, target}' "$d/runs-move.json")"
t0=$(date +%s%3N)
(cd "$RUN_PROJECT" && timeout 900 "$COZY" --json run "$here_rcmoves/weightless/reverse" "text=render$RANDOM" --on stand-b) > "$d/render.ndjson" 2> "$d/render.err" || code=$?
rm_runs stand-b "$d/runs.json"
echo "rc yields: stand-b's runs: $(jq -c '[.runs | sort_by(.number)[] | {number, name, kind, status, target: .target[0:60], created: (.created_at_ms - '"$t0"'), accepted: ((.accepted_at_ms // 0) - '"$t0"'), reason: .reason.code}]' "$d/runs.json")"
[ "$code" = 0 ] || check "rc yields: cozy run on stand-b exited $code: $(tail -c 400 "$d/render.err")"
made=$(jq -r '[.runs[] | select(.kind != "update")] | min_by(.created_at_ms) | .created_at_ms // empty' "$d/runs.json")
accepted=$(jq -r '[.runs[] | select(.kind != "update")] | min_by(.created_at_ms) | .accepted_at_ms // empty' "$d/runs.json")
[ -n "$made" ] && [ -n "$accepted" ] || { check "rc yields: no run of stand-b's was made and accepted: $(head -c 600 "$d/runs.json")"; return; }
echo "rc yields: asked while $move was being sent; its first run was made $((made - t0)) ms later and accepted $((accepted - made)) ms after that"
[ $((accepted - made)) -lt 5000 ] || check "rc yields: the render's first run was accepted $((accepted - made)) ms after it was made: it was held behind the move"
J --arg m "$move" '.runs[] | select(.id == $m) | .status == "canceled" and .reason.code == "yielded"' "$d/runs.json" \
  || check "rc yields: $move didn't yield to the render: $(jq -c --arg m "$move" '.runs[] | select(.id == $m) | {status, reason}' "$d/runs.json")"
J --argjson at "$accepted" '[.runs[] | select(.kind == "update" and .created_at_ms < $at and .reason.code != "yielded")] | length == 0' "$d/runs.json" \
  || check "rc yields: stand-b moved before the render went: $(jq -c --argjson at "$accepted" '[.runs[] | select(.kind == "update" and .created_at_ms < $at) | {number, status, reason}]' "$d/runs.json")"

# Idle again, it moves: once the render has ended, the follower makes its move again and sends it.
local ended again
ended=$(jq -r '[.runs[] | select(.kind != "update") | .settled_at_ms // 0] | max // 0' "$d/runs.json")
for _ in $(seq 300); do
  rm_runs stand-b "$d/runs-after.json"
  J --arg m "$move" '[.runs[]? | select(.kind == "update" and (.target | test("\\(RC\\)")) and .id != $m and .accepted_at_ms != null)] | length > 0' "$d/runs-after.json" && break
  sleep 1
done
again=$(jq -r --arg m "$move" '[.runs[]? | select(.kind == "update" and (.target | test("\\(RC\\)")) and .id != $m)] | min_by(.created_at_ms) | .id // empty' "$d/runs-after.json")
for _ in $(seq 240); do
  rm_runs stand-b "$d/runs-after.json"
  J --arg a "$again" '[.runs[]? | select(.id == $a and (.status == "queued" or .status == "in_progress"))] | length == 0' "$d/runs-after.json" && break
  sleep 1
done
rm_show stand-b > "$d/show-after.json" || true
echo "rc yields: the render ended $((ended - t0)) ms after it was asked; then $(jq -c '[.runs | sort_by(.number)[] | select(.kind == "update") | {number, status, reason: .reason.code, created: (.created_at_ms - '"$t0"'), accepted: ((.accepted_at_ms // 0) - '"$t0"')}]' "$d/runs-after.json"); stand-b runs $(jq -c '.status // {} | {version, runtime}' "$d/show-after.json")"
J --arg a "$again" --argjson ended "$ended" '[.runs[]? | select(.id == $a and .created_at_ms >= $ended and .accepted_at_ms != null)] | length == 1' "$d/runs-after.json" \
  || check "rc yields: stand-b wasn't moved to the RC once idle: $(jq -c '[.runs[]? | select(.kind == "update") | {number, status, reason, created_at_ms}]' "$d/runs-after.json") (the render ended at $ended)"
rm_down stand-b
}

# rc_held (cl-1236, from #3824's review): a rental released itself under the run waiting for its update. Calcifer holds a run in its
# own journal while the machine updates, so TensorD sees no job, and since cl-1203 an update restarts no idle clock: one that
# outlasted the deadline released the rental on the restarted service's first pass, before the run was sent, and it requeued (a new
# rental bought). stand-c, a stood-in rental whose image is this cozy on PyPI's newest Runtime and TensorFS, ages a minute idle, then
# is updated by hand to the RC's cozy, which Calcifer stages to it through the relay; a render asked of it meanwhile waits for that.
# Its idle window is an hour until the update restarts its service, then 150 s, so the restarted service starts with the clock's last
# start (its boot) past it, as one an update outlasted would. The render completes on stand-c, which never asked its Hub to release
# it; before, it released itself as the update's restart came up, and the render never ran there.
rc_held() {
local d=$RM_DIR/stand-c image=$RM_DIR/image-c t0 code=0 runner updater rc td release settled
step "a render held for its rental's update keeps that rental through the update's restart, past its idle deadline (cl-1236)"
rc=$(cozy --version | awk '{n = split($2, v, /[.-]/); print v[1] "." v[2] + ($2 ~ /-/ ? 0 : 1) ".0"}')
td=$(curl -fsSL --retry 5 --retry-all-errors "https://github.com/cozy-creator/cozy-creator/releases/download/rc-$rc/SHA256SUMS" 2> /dev/null \
  | sed -n 's/.*  cozy-\(.*\)-linux-x86_64$/\1/p' | head -1) || td=
[ -n "$td" ] || { check "rc held: rc-$rc names no cozy for linux x86_64 to update stand-c to"; return; }
mkdir -p "$image/bin" "$image/wheels" && cp "$RM_DIR/image/wheels/"* "$image/wheels/" && install -m 755 "$(readlink -f "$COZY")" "$image/bin/cozy" \
  || { check "rc held: stand-c's image wasn't made"; return; }
rm_rental stand-c "$image" || return
rm_serve stand-c
for _ in $(seq 360); do rm_show stand-c > "$d/show-ready.json" && J '.status.phase == "ready"' "$d/show-ready.json" && break; sleep 0.5; done
J '.status.phase == "ready"' "$d/show-ready.json" || { check "rc held: stand-c wasn't ready in 3 min: $(tail -c 600 "$d/machine.log")"; rm_down stand-c; return; }
sleep 60
printf '[machine]\nidle_seconds = 150\n' > "$d/home/.cozy/config.toml"
(timeout 600 "$COZY" --json machine update stand-c --tensord "$td" -y) > "$d/update.json" 2> "$d/update.err" &
updater=$!
for _ in $(seq 240); do
  rm_show stand-c > "$d/show-update.json" || true
  J '[.jobs[]? | select(.kind == "update" and (.status == "queued" or .status == "in_progress"))] | length > 0' "$d/show-update.json" && break
  sleep 0.5
done
t0=$(date +%s%3N)
(cd "$RUN_PROJECT" && timeout 600 "$COZY" --json run "$here_rcmoves/weightless/reverse" "text=held$RANDOM" --on stand-c) > "$d/held.ndjson" 2> "$d/held.err" &
runner=$!
echo "rc held: asked as stand-c's update to cozy $td went: $(jq -c '[.jobs[]? | select(.kind == "update") | {number, status, waiting, target}]' "$d/show-update.json")"
wait "$runner" || code=$?
wait "$updater" || true
rm_runs stand-c "$d/runs.json"
echo "rc held: the render waited: $(grep -oE '"reason": *"(updating stand-c first|waiting for stand-c to finish updating)[^"]*"' "$d/held.ndjson" | head -1)"
echo "rc held: stand-c's runs: $(jq -c '[.runs | sort_by(.number)[] | {number, kind, status, target: .target[0:60], created: (.created_at_ms - '"$t0"'), accepted: ((.accepted_at_ms // 0) - '"$t0"'), settled: ((.settled_at_ms // 0) - '"$t0"'), reason: .reason.code}]' "$d/runs.json")"
echo "rc held: stand-c's Hub heard: $(jq -sc --argjson t "$t0" '[.[] | select(.kind != "check_in") | {kind, ms: ((.at * 1000 | floor) - $t)}]' "$d/events.jsonl" 2> /dev/null)"
grep -E 'idle|releas|restarting|keepalive' "$d/machine.log" | tail -8 | cut -c1-300 || true
grep -qE '"reason": *"(updating stand-c first|waiting for stand-c to finish updating)' "$d/held.ndjson" \
  || check "rc held: the render wasn't held behind stand-c's update: $(tail -c 400 "$d/held.ndjson")"
release=$(jq -s '[.[] | select(.kind == "release") | .at * 1000 | floor] | min // 0' "$d/events.jsonl" 2> /dev/null || echo 0)
settled=$(jq -r '[.runs[] | select(.kind != "update") | .settled_at_ms // 0] | max // 0' "$d/runs.json")
[ "$code" = 0 ] && J '[.runs[] | select(.kind != "update")] | length > 0 and all(.status == "completed")' "$d/runs.json" \
  || check "rc held: the render held behind stand-c's update didn't complete there (exit $code): $(tail -c 400 "$d/held.err")"
if [ "${release:-0}" != 0 ] && { [ "${settled:-0}" = 0 ] || [ "$release" -lt "$settled" ]; }; then
  check "rc held: stand-c asked its Hub to release it $((release - t0)) ms after the render was asked, under the render held for its update"
fi
rm_down stand-c
}

# rm_image <cozy> <distribution> <version>...: the release's cozy, checked against its release's sha256.sum, and each wheel from PyPI
# (its abi3 manylinux x86_64 wheel, else its pure one), as a worker image keeps them.
rm_image() {
  local d=$RM_DIR a=cozy-x86_64-unknown-linux-gnu.tar.gz base="https://github.com/cozy-creator/cozy-creator/releases/download/v$1" bin url
  curl -fsSL --retry 3 -o "$d/$a" "$base/$a" && curl -fsSL --retry 3 -o "$d/sha256.sum" "$base/sha256.sum" \
    && (cd "$d" && awk -v f="$a" '{ n = $2; sub(/^\*/, "", n) } n == f' sha256.sum | sha256sum -c --strict -) > /dev/null \
    && tar -xzf "$d/$a" -C "$d/x" --no-same-owner && bin=$(find "$d/x" -type f -name cozy -print -quit) && [ -n "$bin" ] \
    && install -m 755 "$bin" "$d/image/bin/cozy" || { check "rc moves: cozy $1's $a didn't check against its sha256.sum"; return 1; }
  shift
  while [ $# -ge 2 ]; do
    url=$(curl -fsSL --retry 3 "https://pypi.org/pypi/$1/$2/json" | jq -r '[.urls[] | select(.packagetype == "bdist_wheel")]
      | (map(select(.filename | test("abi3-manylinux.*x86_64"))) + map(select(.filename | test("-none-any")))) | first | .url // empty')
    [ -n "$url" ] && curl -fsSL --retry 3 -o "$d/image/wheels/${url##*/}" "$url" || { check "rc moves: $1 $2's wheel from PyPI: ${url:-none}"; return 1; }
    shift 2
  done
}

# rm_rental <name> [image]: its image ($RM_DIR/image unless named), its stood-in Hub and its slow relay; served once to be added by its
# relay's address, then down.
rm_rental() {
  local name=$1 d=$RM_DIR/$1 image=${2:-$RM_DIR/image} port fp code hub provider ca
  mkdir -p "$d/home/.cozy" "$d/root/opt/cozy/wheels" "$d/bin" && cp "$image/wheels/"* "$d/root/opt/cozy/wheels/" \
    && cp "$image/bin/cozy" "$d/bin/cozy" || { check "rc moves: $name's image wasn't made"; return 1; }
  printf '[machine]\nidle_seconds = 3600\n' > "$d/home/.cozy/config.toml"
  printf '%s\n' "$RM_KEY" > "$d/keys"
  python3 "$here_rcmoves/rentalself/standin.py" serve "$d" held > "$d/standin.log" 2>&1 & echo $! > "$d/standin.pid"
  for _ in $(seq 60); do [ -s "$d/ready" ] && break; sleep 0.5; done
  read -r hub provider ca < "$d/ready" 2> /dev/null || { check "rc moves: $name's stand-in Hub didn't start: $(tail -c 400 "$d/standin.log")"; return 1; }
  port=$(rm_port)
  echo "$port" > "$d/port"
  rm_listen "$d/relay" relay "127.0.0.1:$port" "$RM_UPLINK" > "$d/relay.port" || { check "rc moves: $name's relay didn't start"; return 1; }
  rm_serve "$name"
  for _ in $(seq 240); do
    openssl s_client -connect "127.0.0.1:$port" < /dev/null 2> /dev/null | openssl x509 -outform der > "$d/leaf.der" 2> /dev/null && [ -s "$d/leaf.der" ] && break
    kill -0 "$(cat "$d/machine.pid")" 2> /dev/null || break
    sleep 0.5
  done
  [ -s "$d/leaf.der" ] || { check "rc moves: $name never served on $port: $(tail -c 800 "$d/machine.log")"; return 1; }
  fp=$(sha256sum < "$d/leaf.der" | cut -c1-64)
  for _ in $(seq 60); do
    code=0; cozy --json machine add "$name" "127.0.0.1:$(cat "$d/relay.port")" --fingerprint "sha256:$fp" > "$d/add.json" 2>&1 || code=$?
    [ "$code" = 0 ] && break
    sleep 1
  done
  [ "$code" = 0 ] || { check "rc moves: machine add $name: $(tail -c 400 "$d/add.json") $(tail -c 400 "$d/machine.log")"; return 1; }
  rm_down "$name"
}

# rm_serve <name>: the stood-in rental, booting, its own process group.
rm_serve() {
  local d=$RM_DIR/$1 hub provider ca
  read -r hub provider ca < "$d/ready"
  env HOME="$d/home" COZY_MACHINE_ROOT="$d/root" COZY_WORKER_ID="rcmoves-$1" COZY_WORKER_INTERNAL_PORT="$(cat "$d/port")" \
    COZY_LISTEN_HOST=127.0.0.1 TENSORHUB_ORIGIN="https://127.0.0.1:$hub" TENSORHUB_CA_DER_B64URL="$ca" CUDA_VISIBLE_DEVICES= \
    CONTAINER_ID="$((4300 + ${#1} + RANDOM % 100))" CONTAINER_API_KEY=rcmoves-standin-key COZY_PROVIDER_API_ORIGIN="http://127.0.0.1:$provider" \
    setsid "$d/bin/cozy" machine serve --rental >> "$d/machine.log" 2>&1 & echo $! > "$d/machine.pid"
}

# rm_down <name>: the rental gone at once, as a host lost.
rm_down() {
  local pid
  pid=$(cat "$RM_DIR/$1/machine.pid" 2> /dev/null) || return 0
  [ -n "$pid" ] || return 0
  kill -KILL -- "-$pid" 2> /dev/null || true
  wait "$pid" 2> /dev/null || true
  : > "$RM_DIR/$1/machine.pid"
}

# rm_listen <file> <slow.py args...>: slow.py started in the background, its PID in <file>.pid; prints its port once it listens.
rm_listen() {
  local file=$1; shift
  python3 -I "$here_rcmoves/rcmoves/slow.py" "$@" > "$file.out" 2> "$file.err" & echo $! > "$file.pid"
  for _ in $(seq 40); do [ -s "$file.out" ] && { head -1 "$file.out"; return 0; }; sleep 0.25; done
  return 1
}

rm_port() { python3 -c 'import socket; s = socket.socket(); s.bind(("127.0.0.1", 0)); print(s.getsockname()[1])'; }

rm_quit() {
  local name pid
  for name in stand-a stand-b stand-c stand-d; do
    rm_down "$name"
    for pid in "$RM_DIR/$name/standin.pid" "$RM_DIR/$name/relay.pid"; do
      [ -s "$pid" ] && { kill "$(cat "$pid")" 2> /dev/null || true; }
    done
  done
  [ -s "$RM_DIR/github.pid" ] && { kill "$(cat "$RM_DIR/github.pid")" 2> /dev/null || true; }
  rm -f "$RM_DROPIN" && systemctl --user daemon-reload || true
  cp "$RM_SAVED" "$RM_CFG"
  cozy restart > "$OUT/rcmoves-back.txt" 2>&1 || check "rc moves: Calcifer didn't start again off the dev home: $(tail -c 300 "$OUT/rcmoves-back.txt")"
}

# rm_store <sql>: one statement on Calcifer's stopped store, which must change one row.
rm_store() {
  python3 -I - "$HOME/.cozy/calcifer/state.db" "$1" <<'PY'
import sqlite3, sys
db = sqlite3.connect(sys.argv[1], isolation_level=None)
changed = db.execute(sys.argv[2]).rowcount
db.execute("PRAGMA wal_checkpoint(TRUNCATE)")
sys.exit(0 if changed == 1 else 1)
PY
}

rm_start() {
  systemctl --user start cozy-calcifer || { check "rc moves: Calcifer didn't start"; return 1; }
  for _ in $(seq 60); do cozy --json machine ls > /dev/null 2>&1 && return 0; sleep 1; done
  check "rc moves: Calcifer didn't answer in 60 s"
  return 1
}

# rm_runs <name> <file>: what went to the machine, as {runs: […]}: Calcifer's jobs there (its installs and updates, `machine show`) and
# the project's runs there.
rm_runs() {
  rm_show "$1" > "$2.show" 2> /dev/null || true
  (cd "$RUN_PROJECT" && "$COZY" --json run ls --on "$1" --limit 0) > "$2.project" 2> /dev/null || true
  jq -s '{runs: ((.[0].jobs // []) + (.[1].runs // []) | unique_by(.id))}' "$2.show" "$2.project" > "$2" 2> /dev/null || echo '{"runs": []}' > "$2"
}
rm_show() { timeout 60 "$COZY" --json machine show "$1" 2> /dev/null; }
