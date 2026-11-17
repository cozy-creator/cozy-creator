# needs-ahead (cl-1103; Paul, 11-17: "calcifer says 'okay, I'm going to send machine-A this request, so I should tell it to download
# the required files and install them ahead of time' … if calcifer doesn't do this and just sends the request straight away, then
# tensord / the machine must figure out what it needs and download / install it"): Qwen Image 2 from a packages checkout (a dev home,
# as Paul's laptop is), every step through this runner's cozy and its Calcifer; each rental runs the session's TensorD. Evidence is
# the machine's own: `cozy machine show` (what it holds of each model), the runs' records (`placed`, the quote;
# `measurements.weights`, what each waited for) and its log.
# Steps 2–3 run no function, only a guess's prepare item, so with `cpu_sku` they run on a CPU pod of their own (Paul, 11-17: "spawn
# some runpod cpu-only pods and then tell them to download stuff"), in a project of its own whose one machine it is: a quote doesn't
# rule out a machine with no GPU (it would run there on its CPU), so the guess goes to it. The GPU rental ($SKU) then runs 1 and 4.
# 1. `first` (prompt_upsampling off) rendered on the cold GPU rental, no page showing the project: nothing said ahead, so the
#    machine evaluates the request's rules itself. It holds the model and neither enhancer after; its quote's download is what it
#    waited for.
# 2. A page shows the project: `draw`, drafted text-to-image, is guessed on the machine, which fetches the model and expander_t2i,
#    never expander_i2i.
# 3. While expander_t2i is still coming, the draft mentions <pic>: expander_t2i stops where it was, and expander_i2i comes.
# 4. `draw` is ready on the GPU rental; rendered, its quote downloads nothing and its run waits for nothing. (With `cpu_sku`, its
#    draft there mentions <pic> from the start: the guess fetches expander_i2i, never expander_t2i.)
# params:
#   tree     the packages_refs checkout whose qwen-image-2 runs (`-f packages_refs=pk=<packages ref with its rules>`, tree pk)
#   cpu_sku  a CPU-only SKU (`cpu`) for steps 2–3 (default: none, all four on $SKU)
#   t2i, i2i the enhancers' repositories (default: qwen-image-2's defaults)
#   link_mbit the GPU rental's download link shaped to this many Mbit/s from step 2 on (tc police on its ingress, over SSH; default
#            800), so a draft's edit lands while expander_t2i is still coming ("": as rented; a fast pod fetched its 11 GB in 18 s)
#   need_s   the seconds the cap must buy of every rental together (default 3600)
. "$(dirname "$0")/lib.sh"
# A failure keeps the rental's TensorD log first: what it planned and fetched says why.
fail() {
  [ -z "${NAME:-}" ] || cozy machine logs "$NAME" --since 2h -n 0 > "$OUT/tensord-$NAME.log" 2>&1
  cozy machine logs local calcifer --lines 20000 > "$OUT/calcifer.log" 2>&1
  say "FAIL: $*"; RC=1; exit 1
}
TREE=$(param tree '') CPU_SKU=$(param cpu_sku '')
T2I=$(param t2i pottokao/qwen-image-2.1-pe-t2i-abliterated) I2I=$(param i2i darrellbest/qwen-image-2.1-pe-i2i-abliterated)
[ -n "$TREE" ] && [ -f "$PACKAGES/$TREE/qwen-image-2/pyproject.toml" ] ||
  fail "qwen-image-2's rules aren't released: dispatch with -f packages_refs=<name>=<their packages ref> and params.tree=<name>"
SOURCE=$(cd "$PACKAGES/$TREE" && pwd)
V=$(cozy --version | awk '{print $2}')
[[ $V == *-dev* ]] || fail "cozy $V: the change runs from a dev build (pod-session cozy=<dev-build run id>)"
for tool in jq curl ffmpeg python3; do command -v "$tool" > /dev/null || fail "no $tool"; done
# A dev home, its first-party packages from the checkout, each walk's placement said (diagnostics); Calcifer started again on it.
cfg=$HOME/.cozy/config.toml
mkdir -p "${cfg%/*}" && touch "$cfg" || fail "no $cfg"
{ echo 'dev = true'; grep -v '^dev *=' "$cfg" || true; } > "$cfg.new" && mv "$cfg.new" "$cfg" || fail "dev = true not written"
awk -v source="$SOURCE" '
  function ours() { print "diagnostics = true"; print "packages = \"" source "\""; put = 1 }
  /^(diagnostics|packages) *=/ { next } { print } /^\[calcifer\]/ { ours() } END { if (!put) { printf "\n[calcifer]\n"; ours() } }' \
  "$cfg" > "$cfg.next" && mv "$cfg.next" "$cfg" || fail "[calcifer] not written"
cozy restart > "$OUT/restart.txt" 2>&1 || fail "cozy restart: $(tail -c 300 "$OUT/restart.txt")"
kernels "$SOURCE/qwen-image-2"

nice -n 19 ffmpeg -v error -y -f lavfi -i 'testsrc2=size=1024x1024:rate=1' -frames:v 1 "$OUT/pic.png" || fail "no picture"
WORDS='A red bicycle against a white wall.' PICTURED='The red bicycle of <pic>, against a white wall at night.'
# boxes <file> <draw's words> [first]: this project's picture and boxes.
boxes() {
  run_page import "$OUT/pic.png" --name pic > "$OUT/pic.json" 2>&1 || fail "import pic: $(tail -c 300 "$OUT/pic.json")"
  jq -n --arg words "$2" --arg first "${3:-}" '[{name: "draw", tool: "qwen-image-2/image", kind: "image", fields: {instructions: $words, megapixels: 1}}]
    + if $first == "" then [] else [{name: "first", tool: "qwen-image-2/image", kind: "image",
        fields: {instructions: "A lighthouse on a cliff at dawn, waves breaking below.", megapixels: 1, prompt_upsampling: false}}] end' > "$1"
  run_page boxes "$1" > "$1.txt" 2>&1 || fail "the project's boxes: $(tail -3 "$1.txt")"
}
GPU_PROJECT=$RUN_PROJECT
if [ -n "$CPU_SKU" ]; then
  boxes "$OUT/boxes.json" "$PICTURED" first
  run_project "$OUT/cpu-project" > "$OUT/cpu-project.json" 2>&1 || fail "the CPU pod's project: $(tail -c 300 "$OUT/cpu-project.json")"
  boxes "$OUT/cpu-boxes.json" "$WORDS"
  RUN_PROJECT=$GPU_PROJECT
else
  boxes "$OUT/boxes.json" "$WORDS" first
fi
say "cozy $V, a dev home; qwen-image-2 $(sed -n 's/^version = "\(.*\)"/\1/p' "$SOURCE/qwen-image-2/pyproject.toml" | head -1) from $TREE $(git -C "$SOURCE" rev-parse --short=8 HEAD 2> /dev/null)${CPU_SKU:+; steps 2–3 on $CPU_SKU}"
LINK=$(param link_mbit 800)
[[ -z $LINK || $LINK =~ ^[0-9]+$ ]] || fail "link_mbit $LINK"
if [ -n "$LINK" ]; then
  [ -f ~/.ssh/id_ed25519.pub ] || ssh-keygen -q -t ed25519 -N '' -f ~/.ssh/id_ed25519 || fail "ssh-keygen"
  RENT_ARGS=--ssh
fi
quote "$(param need_s 3600)" "$SKU" ${CPU_SKU:+"$CPU_SKU"}

# shown <file>: what the machine says it holds now. held <file> <repository>: its own bytes there (`unique_bytes`, what no other
# model shares: the two enhancers share 357 MiB, which either one's download brings; a checkpoint whose header alone landed, as a
# plan's resolution brings every slot's, holds a few hundred KiB); whole: a checkpoint of it is complete.
# total: what it holds of every model, each object once (each one's own bytes, and what they share once).
shown() { cozy --json machine show "$NAME" > "$1" 2> /dev/null || true; }
held() { jq -r --arg m "$2" '[.status.models[]? | select(.repository == $m) | .unique_bytes // 0] | add // 0' "$1" 2> /dev/null || echo 0; }
whole() { jq -e --arg m "$2" '[.status.models[]? | select(.repository == $m) | .checkpoints[]?.complete] | any' "$1" > /dev/null 2>&1; }
models() { jq -c '[.status.models[]? | {repository, total_bytes, unique_bytes, complete: [.checkpoints[]?.complete]}]' "$1" 2> /dev/null; }
total() { jq -r '([.status.models[]?.unique_bytes // 0] | add // 0) as $own | ([.status.models[]? | (.total_bytes // 0) - (.unique_bytes // 0)] | max // 0) as $shared | $own + $shared' "$1" 2> /dev/null || echo 0; }
HEADER=$((16 << 20))
# record <box> <file>: its newest run's whole record (`cozy run watch --once --full`).
record() {
  local n
  n=$(run_page asset "$1" | jq -r '.versions[-1].runs[-1] // empty')
  [ -n "$n" ] && (cd "$RUN_PROJECT" && cozy --json --full run watch "$n" --once) > "$2" 2>&1
}
# version: the TensorD $NAME runs now. ours: the session's (`V`), which carries the change. A dev home's rental moves to the open RC's
# builds once it is ready (rc.rs), and 44046574458's moved back after the session's update: it is checked before each step.
version() { cozy --json machine show "$NAME" 2> /dev/null | jq -r '.status.version // empty'; }
ours() {
  local tries=0 runs
  # Once: the open RC's builds first, its Runtime among them (the checkout's qwen-image-2 needs cozy-runtime>=0.47.0.dev0), by an
  # update that names nothing; then the session's TensorD by hand, which holds it there.
  if [[ " ${RC_DONE:-} " != *" $NAME "* ]]; then
    RC_DONE="${RC_DONE:-} $NAME"
    step rc machine update "$NAME" -y || fail "$NAME didn't move to the open RC's builds"
  fi
  while runs=$(version); [ "$runs" != "$V" ]; do
    [ "$tries" -lt 3 ] || fail "$NAME doesn't keep the session's TensorD $V: it runs ${runs:-nothing it says}"
    [ "$tries" = 0 ] || say "$NAME runs ${runs:-nothing it says}, not the session's $V: its TensorD again"
    step tensord machine update "$NAME" --tensord "$(command -v cozy)" -y || fail "$NAME refused the session's TensorD"
    sleep 30; tries=$((tries + 1))
  done
  say "$NAME runs the session's TensorD $V"
}
# on_machine: the rental just booted ($NAME) on the session's TensorD, and this project's one machine.
on_machine() {
  ours
  (cd "$RUN_PROJECT" && cozy --json machine use "$NAME") > "$OUT/use-$NAME.json" 2>&1 || fail "machine use $NAME: $(tail -c 300 "$OUT/use-$NAME.json")"
}
# page: a page shows this project, as the page's own fetch asks it (cl-985): its boxes are guessed while it does.
page() {
  local addr id stream='' headers
  addr=$(jq -r .address "$HOME/.cozy/calcifer/calcifer.lock.holder")
  headers=(-H "Host: $addr" -H 'Sec-Fetch-Site: same-origin' -H 'Sec-Fetch-Mode: cors' -H 'Sec-Fetch-Dest: empty' -H 'cozy-project-storage: 3')
  curl -sSN "${headers[@]}" -H 'Accept: text/event-stream' "http://$addr/v1/events" > "$OUT/page-$NAME.sse" 2>&1 & PAGE=$!; BG="$BG $PAGE"
  for _ in $(seq 100); do
    stream=$(sed -n 's/^data: //p' "$OUT/page-$NAME.sse" | jq -r 'select(.type == "hello") | .stream' 2> /dev/null | head -1); [ -n "$stream" ] && break; sleep 0.1
  done
  [ -n "$stream" ] || fail "the page's stream said no hello: $(head -c 300 "$OUT/page-$NAME.sse")"
  id=$(python3 -c 'import sys; sys.path.insert(0, sys.argv[1]); import page; print(page.read(sys.argv[2])["id"])' "$(dirname "$RUN_PAGE")" "$RUN_PROJECT" 2> "$OUT/id.err")
  [ -n "$id" ] || fail "the project's id: $(tail -c 300 "$OUT/id.err")"
  PAGE_MS=$(date +%s%3N)
  curl -fsS -X PUT "${headers[@]}" -H "Origin: http://$addr" -H 'Content-Type: application/json' -d "$(jq -nc --arg id "$id" '{projects: [$id]}')" \
    "http://$addr/v1/events/$stream" > "$OUT/show-$NAME.json" 2>&1 || fail "the page couldn't show the project: $(tail -c 300 "$OUT/show-$NAME.json")"
}
# first_fetch <since ms> <rule>: from TensorD's log, when the first list item planned after <since> whose rules hold for <rule> was
# planned, and when the machine first fetched an object at a prepare item's order after that (Unix ms each; the pod's clock).
first_fetch() {
  python3 -I - "$OUT/tensord-$NAME.log" "$1" "$2" <<'PY'
import datetime, re, sys
log, since, rule = sys.argv[1], int(sys.argv[2]), sys.argv[3]
def ms(stamp):
    return int(datetime.datetime.fromisoformat(stamp.replace("Z", "+00:00")).timestamp() * 1000)
planned = fetched = 0
for line in open(log, errors="replace"):
    parts = line.split(" ", 1)
    if not parts[0][:4].isdigit():
        continue
    at = ms(parts[0])
    if at < since:
        continue
    if not planned and "cozy:prepare:" in line and "its rules hold for" in line and f'"{rule}"' in line.split("its rules hold for", 1)[1].split("}", 1)[0]:
        planned = at
    elif planned and not fetched:
        found = re.search(r"fetching object=\S+ bytes=\d+ priority=(\d+)", line)
        if found and int(found.group(1)) >= 1 << 63:
            fetched = at
print(planned, fetched)
PY
}
# row: draw's guess's row on the board, the newest that names it: what the machine a render of it would go to has ready for it.
row() { run_page board | jq -c --arg n draw '[.ahead[]? | select((.names // []) | index($n))] | max_by(.run) // empty' 2> /dev/null; }
# guess_ready <file> <repository> <minutes>: draw's guess is ready on $NAME and holds that model whole; its row in $READY.
guess_ready() {
  local began=$SECONDS
  while [ $((SECONDS - began)) -lt $(($3 * 60)) ]; do
    shown "$1"; READY=$(row)
    whole "$1" "$2" && jq -e --arg m "$NAME" '.ready == true and .machine == $m' <<< "${READY:-null}" > /dev/null 2>&1 && return 0
    sleep 2
  done
  whole "$1" "$2" || fail "$2 isn't whole on $NAME after $3 min: $(models "$1"); row: ${READY:-none}"
  fail "draw isn't ready on $NAME: ${READY:-no row}"
}
# sample <file> <tsv>: what $NAME holds now, into <file>, and a line of <tsv>: the runner's clock (Unix ms), each enhancer's own bytes.
sample() { shown "$1"; printf '%s\t%s\t%s\t%s\n' "$(date +%s%3N)" "$(held "$1" "$T2I")" "$(held "$1" "$I2I")" "$(jq -r '.status.version // empty' "$1")" >> "$2"; }
# MISMATCH: what didn't match, said at the end; each step runs all the same.
MISMATCH=()
# switch: steps 2 and 3 on $NAME, this project's machine.
switch() {
  say "step 2: a page shows the project; draw, drafted text-to-image, is guessed on $NAME"
  ours
  page
  # expander_t2i's own bytes coming: 1 s on, the draft changes, most of its 11 GB still to come. expander_i2i never comes meanwhile.
  local began=$SECONDS guessed='' had series=$OUT/t2i-series.tsv
  printf 'unix_ms\tt2i_own\ti2i_own\ttensord\n' > "$series"
  while [ $((SECONDS - began)) -lt 300 ]; do
    sample "$OUT/guess.json" "$series"
    [ "$(held "$OUT/guess.json" "$I2I")" -le "$HEADER" ] || fail "expander_i2i came for a text-to-image draft: $(models "$OUT/guess.json")"
    ! whole "$OUT/guess.json" "$T2I" || fail "expander_t2i was whole before the edit could cut it (no unfinished bytes to stop): $(models "$OUT/guess.json")"
    [ "$(held "$OUT/guess.json" "$T2I")" -gt "$HEADER" ] && break
    sleep 0.5
  done
  had=$(held "$OUT/guess.json" "$T2I")
  [ "$had" -gt "$HEADER" ] || fail "draw's guess brought no expander_t2i to $NAME in 5 min: $(models "$OUT/guess.json"); its row: $(row)"
  sleep 1
  sample "$OUT/guess.json" "$series"
  guessed=$(row)
  say "draw's guess on $NAME: ${guessed}; it holds $(held "$OUT/guess.json" "$T2I") bytes of expander_t2i's own, none of expander_i2i's: $(models "$OUT/guess.json")"
  say "step 3: draw's draft mentions <pic>"
  EDIT_MS=$(date +%s%3N)
  run_page set draw --fields "$(jq -nc --arg w "$PICTURED" '{instructions: $w}')" > "$OUT/set.json" 2>&1 || fail "draw's edit: $(tail -c 300 "$OUT/set.json")"
  say "draw's draft changed at $EDIT_MS (Unix ms) with $(held "$OUT/guess.json" "$T2I") bytes of expander_t2i's own here"
  # Each second until draw is ready with expander_i2i whole: where expander_t2i's own bytes stop.
  began=$SECONDS
  while [ $((SECONDS - began)) -lt 1800 ]; do
    sample "$OUT/switch.json" "$series"; READY=$(row)
    whole "$OUT/switch.json" "$I2I" && jq -e --arg m "$NAME" '.ready == true and .machine == $m' <<< "${READY:-null}" > /dev/null 2>&1 && break
    sleep 1
  done
  cozy machine logs "$NAME" --since 2h -n 0 > "$OUT/tensord-$NAME.log" 2>&1 || true
  whole "$OUT/switch.json" "$I2I" || fail "expander_i2i isn't whole on $NAME 30 min after the edit: $(models "$OUT/switch.json"); row: ${READY:-none}"
  jq -e --arg m "$NAME" '.ready == true and .machine == $m' <<< "${READY:-null}" > /dev/null || fail "draw isn't ready on $NAME: ${READY:-no row}"
  T2I_LEFT=$(held "$OUT/switch.json" "$T2I")
  # When expander_t2i's own bytes last grew, by the runner's clock, and the drop as TensorD logged it (its own clock).
  local grew dropped at_edit
  at_edit=$(awk -F'\t' -v e="$EDIT_MS" 'NR > 1 && $1 <= e {b = $2} END {print b + 0}' "$series")
  grew=$(awk -F'\t' 'NR > 1 {if ($2 != last) {at = $1; last = $2}} END {print at + 0}' "$series")
  dropped=$(grep -m1 -E "list item cozy:prepare:[^ ]* dropped: its run ends" "$OUT/tensord-$NAME.log" | cut -c1-40)
  say "expander_t2i's own bytes: $at_edit at the edit, $T2I_LEFT at the end; last grew at $grew (Unix ms), $((grew - EDIT_MS)) ms after the edit; TensorD's drop: ${dropped:-not in its log}; series in t2i-series.tsv"
  if whole "$OUT/switch.json" "$T2I"; then
    MISMATCH+=("expander_t2i came whole after the draft stopped needing it ($((grew - EDIT_MS)) ms after the edit)")
  elif [ $((grew - EDIT_MS)) -gt 5000 ] || [ $((T2I_LEFT - at_edit)) -gt $((1 << 30)) ]; then
    MISMATCH+=("expander_t2i's own bytes grew $((T2I_LEFT - at_edit)) bytes for $((grew - EDIT_MS)) ms after the edit")
  fi
  say "after the edit: expander_t2i $([ ${#MISMATCH[@]} = 0 ] && echo stopped, unfinished || echo "${MISMATCH[-1]}"); expander_i2i whole; draw ready on $NAME: $(models "$OUT/switch.json")"
}

if [ -n "$CPU_SKU" ]; then
  # Steps 2–3 on the CPU pod, in its own project: a project shown by a page is guessed until it closes, and the GPU rental's
  # project is shown only at step 4.
  RUN_PROJECT=$OUT/cpu-project
  order "$CPU_SKU"; ready_pod=$RID; ready
  on_machine
  # Its code goes there first, as a render there would carry it: a guess names local code by a manifest its machine already holds.
  (cd "$RUN_PROJECT" && cozy --json package install "$SOURCE/qwen-image-2" --on "$NAME") > "$OUT/cpu-install.json" 2>&1 ||
    fail "qwen-image-2 not installed on $NAME: $(tail -c 300 "$OUT/cpu-install.json")"
  switch
  for m in qwen/qwen-image-2.1 atomicchat/qwen-image-2.1-text-encoder-abliterated; do
    whole "$OUT/switch.json" "$m" || fail "draw's guess left $m unfinished on $NAME: $(models "$OUT/switch.json")"
  done
  # Its page goes, so its project closes and its guesses end; the CPU pod ends.
  kill "$PAGE" 2> /dev/null || true
  say "$NAME ends: steps 2–3 are done"
  cozy machine end "$ready_pod" -y > "$OUT/cpu-end.txt" 2>&1 || cozy machine end "$ready_pod" -y --force >> "$OUT/cpu-end.txt" 2>&1
  RUN_PROJECT=$GPU_PROJECT
fi

rent
on_machine
say "step 1: first, rendered on the cold $NAME with no page showing the project"
timeout 3000 python3 "$RUN_PAGE" render "$RUN_PROJECT" --on "$NAME" first > "$OUT/first.txt" 2>&1 || fail "first: $(tail -c 400 "$OUT/first.txt")"
record first "$OUT/first.json" || fail "first has no run record: $(tail -c 300 "$OUT/first.txt")"
shown "$OUT/after-first.json"
quoted=$(jq -r '.placed.fetch_bytes // empty' "$OUT/first.json") waited=$(jq -r '.measurements.weights.needs // empty' "$OUT/first.json")
say "first: quoted $quoted bytes to download ($(jq -c .placed "$OUT/first.json")); waited for $waited ($(jq -c .measurements.weights "$OUT/first.json")); $NAME holds $(total "$OUT/after-first.json"): $(models "$OUT/after-first.json")"
[ -n "$quoted" ] && [ -n "$waited" ] || fail "first's record says no quote or no needs"
[ "$(held "$OUT/after-first.json" "$T2I")" -le "$HEADER" ] && [ "$(held "$OUT/after-first.json" "$I2I")" -le "$HEADER" ] ||
  fail "an enhancer came for a request with prompt_upsampling off: $(models "$OUT/after-first.json")"
# The quote counts each component's chunks; the machine its objects: a header's bytes apart at most.
[ "$waited" -gt 0 ] && [ $(( (quoted > waited ? quoted - waited : waited - quoted) * 200 )) -le "$waited" ] ||
  fail "first's cold quote $quoted bytes is off the $waited it waited for by more than 0.5%"

# shaped: $NAME's download link at $LINK Mbit/s, as a slower pod's is: what a guess fetches takes long enough to be cut.
shaped() {
  [ -n "$LINK" ] || return 0
  local dev _
  for _ in $(seq 20); do timeout 120 cozy machine ssh "$NAME" true < /dev/null > /dev/null 2>&1 && break; sleep 15; done
  # A worker image has no tc: iproute2 comes first.
  timeout 300 cozy machine ssh "$NAME" "command -v tc > /dev/null || { apt-get update -qq && DEBIAN_FRONTEND=noninteractive apt-get install -y -qq iproute2; }" \
    < /dev/null > "$OUT/iproute2.txt" 2>&1 || say "iproute2 not installed on $NAME: $(tail -c 200 "$OUT/iproute2.txt")"
  dev=$(timeout 120 cozy machine ssh "$NAME" "ip route show default | awk '{print \$5; exit}'" < /dev/null 2> /dev/null | tr -d '\r')
  if timeout 120 cozy machine ssh "$NAME" "tc qdisc add dev $dev handle ffff: ingress && tc filter add dev $dev parent ffff: protocol ip u32 match u32 0 0 police rate ${LINK}mbit burst 4m drop flowid :1" \
      < /dev/null > "$OUT/link.txt" 2>&1; then
    say "$NAME's link ($dev) is shaped to $LINK Mbit/s"
  else
    say "$NAME refused the shaped link: $(tail -c 200 "$OUT/link.txt"); it runs as rented"
  fi
}
shaped
if [ -n "$CPU_SKU" ]; then
  say "step 4: a page shows the project; draw, its draft mentioning <pic>, is guessed on $NAME"
  page
  guess_ready "$OUT/switch.json" "$I2I" 30
  T2I_LEFT=$(held "$OUT/switch.json" "$T2I")
  [ "$T2I_LEFT" -le "$HEADER" ] || fail "expander_t2i came for a draft with a picture: $(models "$OUT/switch.json")"
  say "draw ready on $NAME, expander_i2i whole, none of expander_t2i: $(models "$OUT/switch.json")"
else
  switch
fi
say "step 4: draw rendered on $NAME, on $(version)"
before=$(total "$OUT/switch.json")
timeout 3000 python3 "$RUN_PAGE" render "$RUN_PROJECT" --on "$NAME" draw > "$OUT/draw.txt" 2>&1 || fail "draw: $(tail -c 400 "$OUT/draw.txt")"
record draw "$OUT/draw.json" || fail "draw has no run record: $(tail -c 300 "$OUT/draw.txt")"
shown "$OUT/after-draw.json"
after=$(total "$OUT/after-draw.json")
say "draw: placed $(jq -c .placed "$OUT/draw.json"); weights $(jq -c .measurements.weights "$OUT/draw.json"); $NAME held $before bytes before, $after after"
jq -e '.placed.fetch_bytes == 0' "$OUT/draw.json" > /dev/null || fail "draw's quote on $NAME counts $(jq -r .placed.fetch_bytes "$OUT/draw.json") bytes to download"
jq -e '.measurements.weights.needs == 0' "$OUT/draw.json" > /dev/null || fail "draw waited for $(jq -r .measurements.weights.needs "$OUT/draw.json") bytes"
[ "$after" = "$before" ] || fail "$NAME holds $((after - before)) bytes more after draw's render"
[ "$(held "$OUT/after-draw.json" "$T2I")" = "$T2I_LEFT" ] || fail "draw's render fetched expander_t2i"
cozy machine logs "$NAME" --since 2h -n 0 > "$OUT/tensord-$NAME.log" 2>&1 || true
cozy machine logs local calcifer --lines 20000 > "$OUT/calcifer.log" 2>&1 || true
# Adoption (cl-1123): draw's run names the item that made it ready, and is that request to the byte.
adopted=$(grep -m1 -E "run run-[0-9a-f]+ (is the request|names)" "$OUT/tensord-$NAME.log" | sed 's/.*INFO tensord::ahead: //')
say "draw's run on $NAME: ${adopted:-named no predicted request}"
[[ $adopted == *"is the request"*"takes what it made ready"* ]] || MISMATCH+=("draw's render didn't adopt its item: ${adopted:-it named none}")
# From each draft to its machine's first byte (`first_fetch`): the page showing the project, and the edit.
read -r planned fetched <<< "$(first_fetch "$PAGE_MS" expander_t2i)"
say "the T2I draft: shown at $PAGE_MS; planned on $NAME $((planned - PAGE_MS)) ms later; first fetched $((fetched - PAGE_MS)) ms after it was shown"
[ "$fetched" -gt 0 ] && [ $((fetched - PAGE_MS)) -le 5000 ] || MISMATCH+=("the T2I draft's first byte came $((fetched - PAGE_MS)) ms after the page showed it")
if [ -n "${EDIT_MS:-}" ]; then
  read -r planned fetched <<< "$(first_fetch "$EDIT_MS" expander_i2i)"
  say "the I2I edit: made at $EDIT_MS; planned on $NAME $((planned - EDIT_MS)) ms later; first fetched $((fetched - EDIT_MS)) ms after the edit"
  [ "$fetched" -gt 0 ] && [ $((fetched - EDIT_MS)) -le 5000 ] || MISMATCH+=("the I2I edit's first byte came $((fetched - EDIT_MS)) ms after it")
fi
[ ${#MISMATCH[@]} = 0 ] || fail "what didn't match: ${MISMATCH[*]}"
say "PASS: each machine fetched what each request's rules said, Calcifer's word ahead of the render, and the cold quote was what landed"
