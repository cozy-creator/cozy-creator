# walk-ahead (cl-1123; the 0.27.0 launch walkthrough's W-S10 and W-S11, pod-session 44338350009): on one rental, what a box's guess
# prepares before Render, for qwen-image-2's published release and for the same package from a packages checkout (a dev home, as
# Paul's is). Proof-branch harness, never merged. Every step through this runner's cozy and its Calcifer; the evidence is the machine's
# own (its Status every 2 s in samples.ndjson, its log, the runs' records) and each project's board.
#   sealed  an encrypted project's BiRefNet box (from the checkout) on a picture, drafted and shown before the rental is ready, its
#           machine the order's: does it reach `ready` there with nothing more done?
#   arm 1   published (`qwen-image-2/image@<published>`): draw drafted text-to-image, a page showing its project, nothing rendered.
#           Is its environment installed ahead (its item passes `installing`; the pod's environments hold it)? While expander_t2i's
#           own bytes come, the draft mentions <pic>; nothing rendered: which enhancers are whole, what TensorD dropped. Then Render:
#           accepted→started, its quote, what it waited for, and what lands in the minute after it started.
#   between arm 1's project unshown, the release uninstalled (its executor ends), both enhancers uninstalled from the pod.
#   arm 2   the same from source (`qwen-image-2/image`, local/qwen-image-2); the rental is ended by name once its Render starts.
# The order is one: an out-of-stock answer ends the session, never ordered again.
# params:
#   tree       the packages_refs checkout (`-f packages_refs=pk=rc/0.27.0`, tree pk)
#   published  qwen-image-2's Hub release for arm 1 (default 0.14.0)
#   link_mbit  the rental's download link shaped to this many Mbit/s (default 800; "": as rented), so the edit lands while
#              expander_t2i comes; a host that refuses runs as rented and says so
#   need_s     what the cap must buy (default 2400)
. "$(dirname "$0")/lib.sh"
# A failure keeps the rental's TensorD log (unless this session ended it) and Calcifer's.
LOGS_FROM=$(date -u +%s) ENDED=''
pod_logs() { cozy machine logs "$NAME" --since "$(( ($(date -u +%s) - LOGS_FROM) / 60 + 2 ))m" -n 0 > "$OUT/tensord-$NAME.log" 2>&1 || true; }
fail() {
  [ -z "${NAME:-}" ] || [ -n "$ENDED" ] || pod_logs
  cozy machine logs local calcifer --lines 20000 > "$OUT/calcifer.log" 2>&1
  say "FAIL: $*"; RC=1; exit 1
}
TREE=$(param tree '') PUBLISHED=$(param published 0.14.0) LINK=$(param link_mbit 800) NEED=$(param need_s 2400)
T2I=pottokao/qwen-image-2.1-pe-t2i-abliterated I2I=darrellbest/qwen-image-2.1-pe-i2i-abliterated
[ -n "$TREE" ] && [ -f "$PACKAGES/$TREE/qwen-image-2/pyproject.toml" ] && [ -f "$PACKAGES/$TREE/birefnet/pyproject.toml" ] ||
  fail "dispatch with -f packages_refs=<name>=<packages ref> and params.tree=<name>"
[[ -z $LINK || $LINK =~ ^[0-9]+$ ]] || fail "link_mbit $LINK"
SOURCE=$(cd "$PACKAGES/$TREE" && pwd)
V=$(cozy --version | awk '{print $2}')
[[ $V == *-dev* ]] || fail "cozy $V: the RC's dev build (pod-session cozy=<dev-build run id>)"
for tool in jq curl ffmpeg python3; do command -v "$tool" > /dev/null || fail "no $tool"; done
cfg=$HOME/.cozy/config.toml
mkdir -p "${cfg%/*}" && touch "$cfg" || fail "no $cfg"
{ echo 'dev = true'; grep -v '^dev *=' "$cfg" || true; } > "$cfg.new" && mv "$cfg.new" "$cfg" || fail "dev = true not written"
awk -v source="$SOURCE" '
  function ours() { print "diagnostics = true"; print "packages = \"" source "\""; put = 1 }
  /^(diagnostics|packages) *=/ { next } { print } /^\[calcifer\]/ { ours() } END { if (!put) { printf "\n[calcifer]\n"; ours() } }' \
  "$cfg" > "$cfg.next" && mv "$cfg.next" "$cfg" || fail "[calcifer] not written"
cozy restart > "$OUT/restart.txt" 2>&1 || fail "cozy restart: $(tail -c 300 "$OUT/restart.txt")"
kernels "$SOURCE/qwen-image-2"
kernels "$SOURCE/birefnet"
nice -n 19 ffmpeg -v error -y -f lavfi -i 'testsrc2=size=1024x1024:rate=1' -frames:v 1 "$OUT/pic.png" || fail "no picture"
WORDS='A red bicycle against a white wall.' PICTURED='The red bicycle of <pic>, against a white wall at night.'
PAGE() { python3 -I "$RUN_PAGE" "$@"; }
# c <name> <dir> <cozy args…>: one command in <dir>, its output in <name>.txt.
c() { local n=$1 d=$2; shift 2; (cd "$d" && timeout 900 cozy "$@") > "$OUT/$n.txt" 2>&1 < /dev/null; }
now() { date +%s%3N; }

# The projects: two plain ones (one an arm each), and an encrypted one whose passphrase stays out of the artifact.
PJ=${RUNNER_TEMP:-/tmp}/walk-ahead SECRETS=${RUNNER_TEMP:-/tmp}/walk-ahead-secrets
mkdir -p "$PJ" "$SECRETS" && chmod 700 "$SECRETS" || fail "no $PJ"
PUB=$PJ/pub SRC=$PJ/src SEALED=$PJ/sealed
run_project "$PUB" > "$OUT/project-pub.json" 2>&1 || fail "project pub: $(tail -c 300 "$OUT/project-pub.json")"
run_project "$SRC" > "$OUT/project-src.json" 2>&1 || fail "project src: $(tail -c 300 "$OUT/project-src.json")"
head -c 24 /dev/urandom | base64 > "$SECRETS/pass.txt"
c new-sealed "$PJ" new sealed --passphrase-file "$SECRETS/pass.txt" --recovery-out "$SECRETS/recovery.txt" ||
  fail "the encrypted project: $(tail -c 300 "$OUT/new-sealed.txt")"
for p in pub src sealed; do
  PAGE import "$PJ/$p" "$OUT/pic.png" --name pic > "$OUT/pic-$p.json" 2>&1 || fail "$p's picture: $(tail -c 300 "$OUT/pic-$p.json")"
done
# What each arm's tool is: the Hub's release, and the checkout's code.
c describe-pub "$PUB" run "qwen-image-2/image@$PUBLISHED" --describe || fail "qwen-image-2/image@$PUBLISHED: $(tail -c 300 "$OUT/describe-pub.txt")"
c describe-src "$SRC" run qwen-image-2/image --describe || fail "qwen-image-2/image: $(tail -c 300 "$OUT/describe-src.txt")"
! grep -q 'from source' "$OUT/describe-pub.txt" || fail "arm 1's tool runs from source: $(head -3 "$OUT/describe-pub.txt")"
grep -q 'from source' "$OUT/describe-src.txt" || fail "arm 2's tool isn't the checkout's: $(head -3 "$OUT/describe-src.txt")"
grep -Eq 'expander_t2i|expander_i2i' "$OUT/describe-pub.txt" || fail "qwen-image-2@$PUBLISHED has no expander slots: $(cat "$OUT/describe-pub.txt")"
draft() { jq -nc --arg w "$1" '{instructions: $w, megapixels: 1}'; }
PAGE add "$SEALED" seal --tool birefnet/remove_background --fields '{"image": "<pic>"}' > "$OUT/add-seal.json" 2>&1 || fail "seal: $(tail -c 300 "$OUT/add-seal.json")"
PAGE add "$PUB" draw --tool "qwen-image-2/image@$PUBLISHED" --fields "$(draft "$WORDS")" > "$OUT/add-pub.json" 2>&1 || fail "pub's draw: $(tail -c 300 "$OUT/add-pub.json")"
PAGE add "$SRC" draw --tool qwen-image-2/image --fields "$(draft "$WORDS")" > "$OUT/add-src.json" 2>&1 || fail "src's draw: $(tail -c 300 "$OUT/add-src.json")"
say "cozy $V, a dev home; arm 1 qwen-image-2@$PUBLISHED (Hub), arm 2 qwen-image-2 $(sed -n 's/^version = "\(.*\)"/\1/p' "$SOURCE/qwen-image-2/pyproject.toml" | head -1) from $TREE $(git -C "$SOURCE" rev-parse --short=8 HEAD 2> /dev/null); sealed: birefnet from $TREE"

[ -z "$LINK" ] || { [ -f ~/.ssh/id_ed25519.pub ] || ssh-keygen -q -t ed25519 -N '' -f ~/.ssh/id_ed25519 || fail "ssh-keygen"; RENT_ARGS=--ssh; }
# One quote and one order: an out-of-stock answer is the providers' balance, and ends the session (no second order).
say "cozy $(cozy --version); $(cozy whoami 2>&1 | grep -E '^account')"
! listed pr-pod-session-probe || fail "gone-check: a rental that never existed reads live"
f=$OUT/quote-$SKU.json
cozy machine rent "$SKU" --disk-gb "$DISK" $RENT_ARGS --json < /dev/null > "$f" 2> "${f%.json}.err" || true
! grep -qs sku_out_of_stock "$f" "${f%.json}.err" || fail "OUT OF STOCK at the quote, not asked again: $(tail -c 300 "${f%.json}.err")"
grep -qs quote_unagreed "$f" "${f%.json}.err" || fail "a quote without --yes did not refuse: $(tail -c 300 "$f") $(tail -c 300 "${f%.json}.err")"
price=$(jq -R -r 'fromjson? | select(.quote) | .quote.usd_micros_per_hour' "$f" | head -1)
[ -n "$price" ] || fail "no quote for $SKU: $(tail -c 300 "$f") $(tail -c 300 "${f%.json}.err")"
cp "$f" "$OUT/quote.json"
CAP_S=$(awk -v c="$CAP" -v h="$price" 'BEGIN{printf "%d", c*1e6 / h * 3600}')
[ "$CAP_S" -ge "$NEED" ] || fail "\$$CAP leaves only ${CAP_S}s at $price micro-USD/h"
say "quote $SKU $price micro-USD/h, $DISK GB asked: \$$CAP buys about ${CAP_S}s; the guard ends it by what the Hub bills"
[ "$RENT" = true ] || { say "pre-flight only: nothing rented"; exit 0; }
cozy --json machine rent "$SKU" --disk-gb "$DISK" $RENT_ARGS -y > "$OUT/rent0.out" 2> "$OUT/rent0.err" 9>&- & RP_0=$!
until RID=$(accepted "$OUT/rent0.out"); [ -n "$RID" ] || ! kill -0 "$RP_0" 2> /dev/null; do sleep 1; done
[ -n "$RID" ] || RID=$(accepted "$OUT/rent0.out")
if [ -z "$RID" ]; then
  wait "$RP_0"
  ! grep -qs sku_out_of_stock "$OUT/rent0.out" "$OUT/rent0.err" || fail "OUT OF STOCK: the order for $SKU was refused and not placed again: $(tail -c 300 "$OUT/rent0.err")"
  fail "the order was not accepted: $(tail -c 300 "$OUT/rent0.out") $(tail -3 "$OUT/rent0.err")"
fi
RIDS+=("$RID"); echo "$RID" >> "$OUT/rental"; SKU_0=$SKU; ORDERED_MS=$(now)
say "rental $RID ordered"
guard
# Each project's machine is the order's, chosen under its handle while it boots: no guess goes to another session's pod.
# One that refuses its handle is chosen by its name once it's ready, and sealed shown then.
PINNED=yes
for p in sealed pub src; do c "use-$p" "$PJ/$p" --json machine use "$RID" || PINNED=''; done
# show_sealed: sealed shown, its board's rows for its box every 5 s: whether it gets ready on the rental with nothing more done.
show_sealed() {
  ( exec python3 -I "$RUN_PAGE" show "$SEALED" > "$OUT/show-sealed.json" 2>&1 ) & BG="$BG $!"
  SEALED_MS=$(now)
  ( while :; do printf '%s\t%s\n' "$(now)" "$(python3 -I "$RUN_PAGE" board "$SEALED" 2> /dev/null | jq -c '[.ahead[]? | {names, machine, stage, waiting, ready, held_back}]' 2> /dev/null)"; sleep 5; done \
    >> "$OUT/sealed-board.tsv" ) & BG="$BG $!"
  say "sealed's seal (an encrypted project's BiRefNet box on its picture) shown at $SEALED_MS, $((SEALED_MS - ORDERED_MS)) ms after the order"
}
if [ -n "$PINNED" ]; then
  show_sealed
else
  say "a project couldn't choose the booting rental by its handle ($(tail -c 200 "$OUT"/use-*.txt | tr '\n' ' ')): chosen by name once ready"
fi
ready
READY_MS=$(now)
if [ -z "$PINNED" ]; then
  for p in sealed pub src; do c "use-$p" "$PJ/$p" --json machine use "$NAME" || fail "$p: machine use $NAME: $(tail -c 300 "$OUT/use-$p.txt")"; done
  show_sealed
fi

# The pod's Status every 2 s: its list items' readiness, environments, and each model's bytes (its own, all, whole).
( while :; do
    printf '{"t":%s,"s":%s}\n' "$(now)" "$(cozy --json machine show "$NAME" 2> /dev/null | jq -c '{v: .status.version,
      ahead: [.status.ahead[]? | {id, state, stage: .readiness.stage, done: .readiness.done, total: .readiness.total, reason: .readiness.reason}],
      envs: [.status.environments[]? | "\(.package) \(.release)"],
      models: [.status.models[]? | {r: .repository, all: .total_bytes, own: .unique_bytes, whole: ([.checkpoints[]?.complete] | any)}]}' 2> /dev/null || echo null)"
    sleep 2
  done >> "$OUT/samples.ndjson" ) & BG="$BG $!"
# ours: the open RC's builds on $NAME (its Runtime among them), then the session's TensorD, as needs-ahead.sh does.
version() { cozy --json machine show "$NAME" 2> /dev/null | jq -r '.status.version // empty'; }
step rc machine update "$NAME" -y || fail "$NAME didn't move to the open RC's builds"
tries=0
while runs=$(version); [ "$runs" != "$V" ]; do
  [ "$tries" -lt 3 ] || fail "$NAME doesn't keep the session's TensorD $V: it runs ${runs:-nothing it says}"
  step tensord machine update "$NAME" --tensord "$(command -v cozy)" -y || fail "$NAME refused the session's TensorD"
  sleep 30; tries=$((tries + 1))
done
say "$NAME runs the session's TensorD $V"
SHAPED=''
if [ -n "$LINK" ]; then
  for _ in $(seq 20); do timeout 120 cozy machine ssh "$NAME" true < /dev/null > /dev/null 2>&1 && break; sleep 15; done
  timeout 300 cozy machine ssh "$NAME" "command -v tc > /dev/null || { apt-get update -qq && DEBIAN_FRONTEND=noninteractive apt-get install -y -qq iproute2; }" \
    < /dev/null > "$OUT/iproute2.txt" 2>&1 || say "iproute2 not installed on $NAME: $(tail -c 200 "$OUT/iproute2.txt")"
  dev=$(timeout 120 cozy machine ssh "$NAME" "ip route show default | awk '{print \$5; exit}'" < /dev/null 2> /dev/null | tr -d '\r')
  if timeout 120 cozy machine ssh "$NAME" "tc qdisc add dev $dev handle ffff: ingress && tc filter add dev $dev parent ffff: protocol ip u32 match u32 0 0 police rate ${LINK}mbit burst 4m drop flowid :1" \
      < /dev/null > "$OUT/link.txt" 2>&1; then
    SHAPED=1; say "$NAME's link ($dev) is shaped to $LINK Mbit/s"
  else
    say "$NAME refused the shaped link: $(tail -c 200 "$OUT/link.txt"); it runs as rented, and an edit lands where the host's own speed puts it"
  fi
fi

# now_held <label>: $NAME's Status now, read into T2I/I2I's own bytes and wholeness, its environments, and draw's row on $P's board.
held() { jq -r --arg m "$2" '[.status.models[]? | select(.repository == $m) | .unique_bytes // 0] | add // 0' "$1" 2> /dev/null || echo 0; }
whole() { jq -e --arg m "$2" '[.status.models[]? | select(.repository == $m) | .checkpoints[]?.complete] | any' "$1" > /dev/null 2>&1 && echo yes || echo no; }
models() { jq -c '[.status.models[]? | {r: .repository, all: .total_bytes, own: .unique_bytes, whole: ([.checkpoints[]?.complete] | any)}]' "$1" 2> /dev/null; }
now_held() {
  NOW=$OUT/now-$1.json
  cozy --json machine show "$NAME" > "$NOW" 2> /dev/null || true
  T2I_OWN=$(held "$NOW" "$T2I") T2I_WHOLE=$(whole "$NOW" "$T2I") I2I_OWN=$(held "$NOW" "$I2I") I2I_WHOLE=$(whole "$NOW" "$I2I")
  T2I_OWN=${T2I_OWN:-0} I2I_OWN=${I2I_OWN:-0}
  ENVS=$(jq -c '[.status.environments[]? | "\(.package) \(.release)"]' "$NOW" 2> /dev/null)
  ROW=$(PAGE board "$P" 2> /dev/null | jq -c '[.ahead[]? | select((.names // []) | index("draw"))] | max_by(.run) // {}' 2> /dev/null)
  ITEMS=$(jq -c '[.status.ahead[]? | "\(.state)/\(.readiness.stage // "-")"]' "$NOW" 2> /dev/null)
  printf '%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\n' "$(now)" "$T2I_OWN" "$T2I_WHOLE" "$I2I_OWN" "$I2I_WHOLE" "$ENVS" "$ITEMS" "$ROW" >> "$OUT/arm-$1.tsv"
}
# newest <dir>: the newest qwen-image-2 render's run in that project, by its name.
newest() { (cd "$1" && cozy run ls < /dev/null 2> /dev/null) | awk 'NR > 1 && $2 ~ /qwen-image-2\/image/ {print $1; exit}'; }
record() { (cd "$1" && cozy --json --full run watch "$2" --once < /dev/null) > "$3" 2> /dev/null; }

# arm <label> <project> <end>: draw guessed, edited, rendered; <end> = yes ends the rental by name once the render starts.
arm() {
  local label=$1 end=$3 shown began edit_ms render_ms r rec started=''
  P=$2
  printf 'unix_ms\tt2i_own\tt2i_whole\ti2i_own\ti2i_whole\tenvs\titems\tdraw_row\n' > "$OUT/arm-$label.tsv"
  ( exec python3 -I "$RUN_PAGE" show "$P" > "$OUT/show-$label.json" 2>&1 ) & SHOW=$!; BG="$BG $SHOW"
  shown=$(now)
  say "arm $label: a page shows ${P##*/}; draw, drafted text-to-image, not rendered"
  # Until expander_t2i's own bytes come (the edit then cuts them), it is whole, draw is ready without it, or 12 min.
  began=$SECONDS
  while [ $((SECONDS - began)) -lt 720 ]; do
    now_held "$label"
    [ "$T2I_OWN" -gt $((64 << 20)) ] || [ "$T2I_WHOLE" = yes ] || jq -e '.ready == true' <<< "${ROW:-null}" > /dev/null 2>&1 && break
    sleep 2
  done
  say "arm $label, +$(( $(now) - shown )) ms: expander_t2i own $T2I_OWN (whole $T2I_WHOLE), expander_i2i own $I2I_OWN (whole $I2I_WHOLE); environments $ENVS; items $ITEMS; draw's row $ROW"
  sleep 3; now_held "$label"
  edit_ms=$(now)
  PAGE set "$P" draw --fields "$(jq -nc --arg w "$PICTURED" '{instructions: $w}')" > "$OUT/set-$label.json" 2>&1 || say "arm $label: draw's edit: $(tail -c 300 "$OUT/set-$label.json")"
  say "arm $label: draw's draft mentions <pic> at $edit_ms (+$((edit_ms - shown)) ms), expander_t2i own $T2I_OWN (whole $T2I_WHOLE), environments $ENVS"
  # Nothing rendered: until expander_i2i is whole and draw ready, or 15 min.
  began=$SECONDS
  while [ $((SECONDS - began)) -lt 900 ]; do
    now_held "$label"
    [ "$I2I_WHOLE" = yes ] && jq -e '.ready == true' <<< "${ROW:-null}" > /dev/null 2>&1 && break
    sleep 2
  done
  say "arm $label, after the edit, nothing rendered (+$(( $(now) - edit_ms )) ms): expander_t2i own $T2I_OWN (whole $T2I_WHOLE), expander_i2i own $I2I_OWN (whole $I2I_WHOLE); environments $ENVS; draw's row $ROW"
  cp "$NOW" "$OUT/before-render-$label.json"
  # Render, as the page's Render on this machine; followed until it starts.
  render_ms=$(now)
  PAGE render "$P" --on "$NAME" --queue draw > "$OUT/render-$label.json" 2>&1 || say "arm $label: the render: $(tail -c 300 "$OUT/render-$label.json")"
  began=$SECONDS
  while [ $((SECONDS - began)) -lt 900 ]; do
    r=$(newest "$P")
    if [ -n "$r" ] && record "$P" "$r" "$OUT/run-$label.json"; then
      started=$(jq -r '.started_at_ms // empty' "$OUT/run-$label.json" 2> /dev/null)
      [ -n "$started" ] && break
      jq -e '.settled_at_ms != null' "$OUT/run-$label.json" > /dev/null 2>&1 && break
    fi
    sleep 1
  done
  say "arm $label: Render at $render_ms; run ${r:-none}: $(jq -c '{status, created_at_ms, accepted_at_ms, started_at_ms, placed, weights: .measurements.weights}' "$OUT/run-$label.json" 2> /dev/null)"
  [ -z "$started" ] || say "arm $label: accepted→started $(jq -r '.started_at_ms - .accepted_at_ms' "$OUT/run-$label.json") ms, Render→started $((started - render_ms)) ms"
  if [ "$end" = yes ]; then
    pod_logs
    say "arm $label's render started: $NAME ends now, by name"
    cozy machine end "$NAME" -y > "$OUT/end-by-name.txt" 2>&1 || cozy machine end "$NAME" -y --force >> "$OUT/end-by-name.txt" 2>&1
    ENDED=1
    say "END $NAME: $(tail -c 200 "$OUT/end-by-name.txt" | tr '\n' ' ')"
    return 0
  fi
  # What lands in the minute after it started: its extras, behind every need.
  now_held "$label"; cp "$NOW" "$OUT/at-start-$label.json"
  sleep 60
  now_held "$label"; cp "$NOW" "$OUT/after-start-$label.json"
  record "$P" "$r" "$OUT/run-$label.json"
  began=$SECONDS
  until jq -e '.settled_at_ms != null' "$OUT/run-$label.json" > /dev/null 2>&1 || [ $((SECONDS - began)) -ge 300 ]; do sleep 3; record "$P" "$r" "$OUT/run-$label.json"; done
  say "arm $label, 60 s after its start: $(jq -c '{status, settled_at_ms}' "$OUT/run-$label.json" 2> /dev/null); models at its start $(models "$OUT/at-start-$label.json"); 60 s on $(models "$OUT/after-start-$label.json")"
  kill "$SHOW" 2> /dev/null
}

say "the sealed box's rows so far: $(tail -1 "$OUT/sealed-board.tsv" 2> /dev/null | cut -f2)"
arm 1 "$PUB" no
say "between: pub unshown, qwen-image-2 uninstalled from $NAME (its executor ends), both enhancers uninstalled"
sleep 5
c packages-before "$PUB" package ls --on "$NAME"
c uninstall-pub "$PUB" package uninstall qwen-image-2 --on "$NAME" || say "uninstall qwen-image-2: $(tail -c 300 "$OUT/uninstall-pub.txt")"
for m in "$T2I" "$I2I"; do c "uninstall-${m%%/*}" "$PUB" model uninstall "$m" --on "$NAME" || say "uninstall $m: $(tail -c 300 "$OUT/uninstall-${m%%/*}.txt")"; done
P=$PUB; now_held between
c packages-after "$PUB" package ls --on "$NAME"
say "between: $NAME holds $(models "$NOW"); environments $ENVS; items $ITEMS"
arm 2 "$SRC" yes
say "the sealed box: shown at $SEALED_MS, the rental ready at $READY_MS; its first ready row at $(awk -F'\t' '$2 ~ /"ready":true/ {print $1; exit}' "$OUT/sealed-board.tsv") (Unix ms); its last row $(tail -1 "$OUT/sealed-board.tsv" | cut -f2)"
cozy machine logs local calcifer --lines 20000 > "$OUT/calcifer.log" 2>&1 || true
find "$SEALED" -path '*diagnostics*' -name '*.log' -exec cp {} "$OUT/" \; 2> /dev/null
say "walk-ahead done"
