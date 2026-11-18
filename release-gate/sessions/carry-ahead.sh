# carry-ahead (cl-1123; Paul, 11-18: "uploading files might be a good pre-load to do too, like if I open a node, and it has a video that
# is a reference video that is like 100MBs, uploading that to the remote machine will take a few minutes … so pre-loading files on the
# machine makes sense as well", and "even for encrypted projects, sending ahead of time is fine. What really matters is that we can trust
# the remote machine as safe"): in an encrypted project, a picture box with Qwen Image 2.1 put on it and an edit typed has its picture
# carried ahead to the rental its render would go to, behind every run's carry, and its render finds it there and sends nothing more.
# Qwen Image 2 from a packages checkout (a dev home), every step through this runner's cozy and its Calcifer, on the session's TensorD;
# evidence is the machine's own log and records.
# 0. An encrypted project (`cozy new`, its passphrase from a file), unlocked with no idle lock; the rental has qwen-image-2 installed.
#    `photo` and `wide`, pictures of noise (~6 MB and ~24 MB, so each goes in ranges), imported into it.
# 1. A page shows it; photo gets Qwen and an edit: Calcifer says photo's guess carried its picture ahead to the rental, an encrypted
#    project's, and the machine says it landed whole, before any render.
# 2. The runner's uplink shaped to `mbit`: wide gets Qwen and an edit, and while its picture goes ahead, photo is rendered. The carry
#    waits for the render's ("a run's inputs go first") and goes on after; the machine lands wide's picture whole once, and the runner
#    sends about its size, not more.
# 3. Each render (photo in 2, then wide) reused its picture there ("reused … already on"), the machine landed none of its picture while it
#    was sent, and it adopted its item.
# params:
#   tree    the packages_refs checkout whose qwen-image-2 runs (`-f packages_refs=pk=<packages ref>`, tree pk)
#   need_s  the seconds the cap must buy (default 2400)
#   mbit    the shaped uplink in step 2 (default 16)
. "$(dirname "$0")/lib.sh"
fail() {
  [ -z "${DEV:-}" ] || sudo tc qdisc del dev "$DEV" root 2> /dev/null || true
  [ -z "${NAME:-}" ] || cozy machine logs "$NAME" --since 2h -n 0 > "$OUT/tensord-$NAME.log" 2>&1
  cozy machine logs local calcifer --lines 20000 > "$OUT/calcifer.log" 2>&1
  say "FAIL: $*"; RC=1; exit 1
}
TREE=$(param tree '') MBIT=$(param mbit 16)
I2I=darrellbest/qwen-image-2.1-pe-i2i-abliterated
[ -n "$TREE" ] && [ -f "$PACKAGES/$TREE/qwen-image-2/pyproject.toml" ] || fail "dispatch with -f packages_refs=<name>=<packages ref> and params.tree=<name>"
[[ $MBIT =~ ^[0-9]+$ && $MBIT -ge 4 ]] || fail "mbit $MBIT"
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
quote "$(param need_s 2400)"

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
page() {
  local addr id stream='' headers
  addr=$(jq -r .address "$HOME/.cozy/calcifer/calcifer.lock.holder")
  headers=(-H "Host: $addr" -H 'Sec-Fetch-Site: same-origin' -H 'Sec-Fetch-Mode: cors' -H 'Sec-Fetch-Dest: empty' -H 'cozy-project-storage: 3')
  curl -sSN "${headers[@]}" -H 'Accept: text/event-stream' "http://$addr/v1/events" > "$OUT/page.sse" 2>&1 & BG="$BG $!"
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
MISMATCH=()
# rendered <box> <digest> [&]: rendered on $NAME, it reused its picture there, the machine landed none of it while it was sent, and
# it adopted its item.
rendered() {
  local began ended adopted
  began=$(date +%s%3N)
  timeout 3000 python3 "$RUN_PAGE" render "$RUN_PROJECT" --on "$NAME" "$1" > "$OUT/$1.txt" 2>&1 || fail "$1: $(tail -c 400 "$OUT/$1.txt")"
  ended=$(date +%s%3N)
  record "$1" "$OUT/$1-run.json" || fail "$1 has no run record: $(tail -c 300 "$OUT/$1.txt")"
  logs
  grep -ho "reused [^\"]* already on $NAME" "$OUT/$1-run.json" "$OUT/$1-run.txt" | head -1 > "$OUT/$1-reused.txt"
  say "$1: rendered in $((ended - began)) ms; its run: $(cat "$OUT/$1-reused.txt")"
  [ -s "$OUT/$1-reused.txt" ] || MISMATCH+=("$1's run didn't find its picture on $NAME")
  [ -z "$(landed "$2" "$began" "$ended")" ] || MISMATCH+=("$NAME landed $1's picture again while it rendered")
  adopted=$(grep -E "run run-[0-9a-f]+ (is the request|names)" "$OUT/tensord-$NAME.log" | tail -1 | sed 's/.*INFO tensord::ahead: //')
  say "$1's run on $NAME: ${adopted:-named no predicted request}"
  [[ $adopted == *"is the request"*"takes what it made ready"* && $adopted != "${ADOPTED:-}" ]] || MISMATCH+=("$1's render didn't adopt its item: ${adopted:-it named none}")
  ADOPTED=$adopted
}

rent
ours
(cd "$RUN_PROJECT" && cozy --json machine use "$NAME") > "$OUT/use.json" 2>&1 || fail "machine use $NAME: $(tail -c 300 "$OUT/use.json")"
began=$SECONDS
(cd "$RUN_PROJECT" && cozy --json package install "$SOURCE/qwen-image-2" --on "$NAME") > "$OUT/install.json" 2>&1 ||
  fail "qwen-image-2 not installed on $NAME: $(tail -c 300 "$OUT/install.json")"
say "step 0: qwen-image-2 installed on $NAME in $((SECONDS - began)) s"
say "step 1: a page shows the encrypted project; photo gets Qwen Image 2.1, then an edit"
page
sleep 2
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
say "step 2: the uplink shaped to $MBIT Mbit/s; wide gets Qwen and an edit, and photo is rendered while wide's picture goes ahead"
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
say "step 3: wide rendered"
rendered wide "$WIDE"
say "Calcifer's carries ahead: $(grep -o "[a-z]*'s guess carried .*" "$OUT/calcifer.log" | paste -sd ';' -)"
[ ${#MISMATCH[@]} = 0 ] || fail "what didn't match: ${MISMATCH[*]}"
say "PASS: an encrypted project's pictures went ahead to the rental behind a render's carry, each landing once, and each render found its picture there"
