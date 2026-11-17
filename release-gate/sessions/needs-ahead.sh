# needs-ahead (cl-1103; Paul, 11-17: "calcifer says 'okay, I'm going to send machine-A this request, so I should tell it to download
# the required files and install them ahead of time' … if calcifer doesn't do this and just sends the request straight away, then
# tensord / the machine must figure out what it needs and download / install it"): one cold rental and Qwen Image 2 from a packages
# checkout (a dev home, as Paul's laptop is), every step through this runner's cozy and its Calcifer; the rental runs the session's
# TensorD. Evidence is the machine's own: `cozy machine show` (what it holds of each model), the runs' records (`placed`, the
# quote; `measurements.weights`, what each waited for) and its log.
# 1. `first` (prompt_upsampling off) rendered on the cold rental, no page showing the project: nothing said ahead, so the machine
#    evaluates the request's rules itself. It holds the model and neither enhancer after; its quote's download is what it waited for.
# 2. A page shows the project: `draw`, drafted text-to-image, is guessed on the rental, which fetches expander_t2i, never expander_i2i.
# 3. While expander_t2i is still coming, the draft mentions <pic>: expander_t2i stops where it was, and expander_i2i comes.
# 4. `draw` is ready on the rental; rendered, its quote downloads nothing and its run waits for nothing.
# params:
#   tree   the packages_refs checkout whose qwen-image-2 runs (`-f packages_refs=pk=<packages ref with its rules>`, tree pk)
#   t2i, i2i  the enhancers' repositories (default: qwen-image-2's defaults)
#   need_s the seconds the cap must buy (default 3600)
. "$(dirname "$0")/lib.sh"
TREE=$(param tree '') T2I=$(param t2i pottokao/qwen-image-2.1-pe-t2i-abliterated) I2I=$(param i2i darrellbest/qwen-image-2.1-pe-i2i-abliterated)
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
run_page import "$OUT/pic.png" --name pic > "$OUT/pic.json" 2>&1 || fail "import pic: $(tail -c 300 "$OUT/pic.json")"
jq -n '[{name: "first", tool: "qwen-image-2/image", kind: "image",
          fields: {instructions: "A lighthouse on a cliff at dawn, waves breaking below.", megapixels: 1, prompt_upsampling: false}},
        {name: "draw", tool: "qwen-image-2/image", kind: "image", fields: {instructions: "A red bicycle against a white wall.", megapixels: 1}}]' \
  > "$OUT/boxes.json"
run_page boxes "$OUT/boxes.json" > "$OUT/boxes.txt" 2>&1 || fail "the project's boxes: $(tail -3 "$OUT/boxes.txt")"
say "cozy $V, a dev home; qwen-image-2 $(sed -n 's/^version = "\(.*\)"/\1/p' "$SOURCE/qwen-image-2/pyproject.toml" | head -1) from $TREE $(git -C "$SOURCE" rev-parse --short=8 HEAD 2> /dev/null)"
quote "$(param need_s 3600)"
rent
step tensord machine update "$NAME" --tensord "$(command -v cozy)" -y || fail "$NAME refused the session's TensorD"
(cd "$RUN_PROJECT" && cozy --json machine use "$NAME") > "$OUT/use.json" 2>&1 || fail "machine use $NAME: $(tail -c 300 "$OUT/use.json")"

# shown <file>: what the rental says it holds now. held <file> <repository>: its bytes there; whole: a checkpoint of it is complete.
shown() { cozy --json machine show "$NAME" > "$1" 2> /dev/null || true; }
held() { jq -r --arg m "$2" '[.status.models[]? | select(.repository == $m) | .checkpoints[]?.held_bytes // 0] | add // 0' "$1" 2> /dev/null || echo 0; }
whole() { jq -e --arg m "$2" '[.status.models[]? | select(.repository == $m) | .checkpoints[]?.complete] | any' "$1" > /dev/null 2>&1; }
models() { jq -c '[.status.models[]? | {repository, held: ([.checkpoints[]?.held_bytes // 0] | add), complete: [.checkpoints[]?.complete]}]' "$1" 2> /dev/null; }
# record <box> <file>: its newest run's whole record (`cozy run watch --once --full`).
record() {
  local n
  n=$(run_page asset "$1" | jq -r '.versions[-1].runs[-1] // empty')
  [ -n "$n" ] && (cd "$RUN_PROJECT" && cozy --json --full run watch "$n" --once) > "$2" 2>&1
}

say "step 1: first, rendered on the cold $NAME with no page showing the project"
timeout 3000 python3 "$RUN_PAGE" render "$RUN_PROJECT" --on "$NAME" first > "$OUT/first.txt" 2>&1 || fail "first: $(tail -c 400 "$OUT/first.txt")"
record first "$OUT/first.json" || fail "first has no run record: $(tail -c 300 "$OUT/first.txt")"
shown "$OUT/after-first.json"
quoted=$(jq -r '.placed.fetch_bytes // empty' "$OUT/first.json") waited=$(jq -r '.measurements.weights.needs // empty' "$OUT/first.json")
landed=$(jq -r '[.status.models[]?.checkpoints[]?.held_bytes // 0] | add // 0' "$OUT/after-first.json")
say "first: quoted $quoted bytes to download ($(jq -c .placed "$OUT/first.json")); waited for $waited ($(jq -c .measurements.weights "$OUT/first.json")); $NAME holds $landed: $(models "$OUT/after-first.json")"
[ -n "$quoted" ] && [ -n "$waited" ] || fail "first's record says no quote or no needs"
[ "$(held "$OUT/after-first.json" "$T2I")" = 0 ] && [ "$(held "$OUT/after-first.json" "$I2I")" = 0 ] ||
  fail "an enhancer came for a request with prompt_upsampling off: $(models "$OUT/after-first.json")"
# The quote counts each component's chunks; the machine its objects: a header's bytes apart at most.
[ "$waited" -gt 0 ] && [ $(( (quoted > waited ? quoted - waited : waited - quoted) * 200 )) -le "$waited" ] ||
  fail "first's cold quote $quoted bytes is off the $waited it waited for by more than 0.5%"

say "step 2: a page shows the project; draw, drafted text-to-image, is guessed on $NAME"
ADDR=$(jq -r .address "$HOME/.cozy/calcifer/calcifer.lock.holder")
H=(-H "Host: $ADDR" -H 'Sec-Fetch-Site: same-origin' -H 'Sec-Fetch-Mode: cors' -H 'Sec-Fetch-Dest: empty' -H 'cozy-project-storage: 3')
curl -sSN "${H[@]}" -H 'Accept: text/event-stream' "http://$ADDR/v1/events" > "$OUT/page.sse" 2>&1 & BG="$BG $!"
STREAM=''
for _ in $(seq 100); do
  STREAM=$(sed -n 's/^data: //p' "$OUT/page.sse" | jq -r 'select(.type == "hello") | .stream' 2> /dev/null | head -1); [ -n "$STREAM" ] && break; sleep 0.1
done
[ -n "$STREAM" ] || fail "the page's stream said no hello: $(head -c 300 "$OUT/page.sse")"
ID=$(cd "$RUN_PROJECT" && cozy --json status | jq -r '.id // empty')
curl -fsS -X PUT "${H[@]}" -H "Origin: http://$ADDR" -H 'Content-Type: application/json' -d "$(jq -nc --arg id "$ID" '{projects: [$id]}')" \
  "http://$ADDR/v1/events/$STREAM" > "$OUT/show.json" 2>&1 || fail "the page couldn't show the project: $(tail -c 300 "$OUT/show.json")"
# expander_t2i coming, not yet whole: the moment to change the draft. expander_i2i never comes meanwhile.
began=$SECONDS cut=0
while [ $((SECONDS - began)) -lt 1200 ]; do
  shown "$OUT/guess.json"
  [ "$(held "$OUT/guess.json" "$I2I")" = 0 ] || fail "expander_i2i came for a text-to-image draft: $(models "$OUT/guess.json")"
  ! whole "$OUT/guess.json" "$T2I" || fail "expander_t2i was whole before the edit could cut it (no unfinished bytes to stop): $(models "$OUT/guess.json")"
  cut=$(held "$OUT/guess.json" "$T2I"); [ "$cut" -gt $((512 << 20)) ] && break
  sleep 1
done
[ "$cut" -gt 0 ] || fail "draw's guess fetched no expander_t2i on $NAME in 20 min: $(models "$OUT/guess.json"); board: $(run_page board | jq -c .ahead)"
say "draw's guess: $NAME holds $cut bytes of expander_t2i, none of expander_i2i: $(models "$OUT/guess.json")"

say "step 3: draw's draft mentions <pic>"
run_page set draw --fields '{"instructions": "The red bicycle of <pic>, against a white wall at night."}' > "$OUT/set.json" 2>&1 ||
  fail "draw's edit: $(tail -c 300 "$OUT/set.json")"
began=$SECONDS t2i=$cut
while [ $((SECONDS - began)) -lt 1800 ]; do
  shown "$OUT/switch.json"
  row=$(run_page board | jq -c --arg n draw '[.ahead[]? | select((.names // []) | index($n))] | max_by(.run) // empty' 2> /dev/null)
  whole "$OUT/switch.json" "$I2I" && jq -e --arg m "$NAME" '.ready == true and .machine == $m' <<< "${row:-null}" > /dev/null 2>&1 && break
  sleep 2
done
whole "$OUT/switch.json" "$I2I" || fail "expander_i2i isn't whole on $NAME 30 min after the edit: $(models "$OUT/switch.json"); row: ${row:-none}"
jq -e --arg m "$NAME" '.ready == true and .machine == $m' <<< "${row:-null}" > /dev/null || fail "draw isn't ready on $NAME: ${row:-no row}"
t2i=$(held "$OUT/switch.json" "$T2I")
! whole "$OUT/switch.json" "$T2I" || fail "expander_t2i came whole after the draft stopped needing it: $(models "$OUT/switch.json")"
say "after the edit: expander_t2i stopped at $t2i bytes ($cut when the draft changed), expander_i2i whole; draw ready on $NAME: $(models "$OUT/switch.json")"

say "step 4: draw rendered on $NAME"
before=$(jq -r '[.status.models[]?.checkpoints[]?.held_bytes // 0] | add // 0' "$OUT/switch.json")
timeout 3000 python3 "$RUN_PAGE" render "$RUN_PROJECT" --on "$NAME" draw > "$OUT/draw.txt" 2>&1 || fail "draw: $(tail -c 400 "$OUT/draw.txt")"
record draw "$OUT/draw.json" || fail "draw has no run record: $(tail -c 300 "$OUT/draw.txt")"
shown "$OUT/after-draw.json"
after=$(jq -r '[.status.models[]?.checkpoints[]?.held_bytes // 0] | add // 0' "$OUT/after-draw.json")
say "draw: placed $(jq -c .placed "$OUT/draw.json"); weights $(jq -c .measurements.weights "$OUT/draw.json"); $NAME held $before bytes before, $after after"
jq -e '.placed.fetch_bytes == 0' "$OUT/draw.json" > /dev/null || fail "draw's quote on $NAME counts $(jq -r .placed.fetch_bytes "$OUT/draw.json") bytes to download"
jq -e '.measurements.weights.needs == 0' "$OUT/draw.json" > /dev/null || fail "draw waited for $(jq -r .measurements.weights.needs "$OUT/draw.json") bytes"
[ "$after" = "$before" ] || fail "$NAME holds $((after - before)) bytes more after draw's render"
[ "$(held "$OUT/after-draw.json" "$T2I")" = "$t2i" ] || fail "draw's render fetched expander_t2i"
cozy machine logs "$NAME" --since 2h -n 0 > "$OUT/tensord.log" 2>&1 || true
say "PASS: the machine fetched what each request's rules said, Calcifer's word ahead of the render, and the cold quote was what landed"
