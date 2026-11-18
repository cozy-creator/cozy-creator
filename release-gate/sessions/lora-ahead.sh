# lora-ahead (cl-1281; Paul, 11-18: "loras are the cheapest and best thing to prefetch"): on one rental, which LoRA files are whole on
# the machine a box's Render would go to before Render, and which come only during it. Proof-branch harness, never merged. A dev home
# on this runner's cozy, its first-party packages from the packages_refs checkout `tree`; every step through this runner's cozy and its
# Calcifer. The evidence is the machine's own: its Status each second (samples.ndjson), its log (each item's plan line), `cozy lora ls
# --on`, and the runs' records (`placed`, the quote; `measurements.weights`, what each waited for).
# 1. `first` (krea-2/image with `lora_c` on the box) rendered on the cold rental, no page showing the project: its quote against what
#    landed, the LoRA among it.
# 2. A page shows the project: `styled` (krea-2/image with `lora_a`) and `turn` (orbit/orbit on <pic>, whose package names ORB360 in its
#    code) are guessed. Which LoRAs come, and when.
# 3. Once `lora_a`'s own bytes come, styled's `lora` is swapped to `lora_b`: whether `lora_a` stops.
# 4. Both ready, nothing rendered: what the machine holds of each LoRA.
# 5. styled rendered, then a caller's bare `--lora` (cl-1241: it adapts the DiT), then turn: each one's quote, what it waited for, and
#    what landed during it.
# params:
#   tree     the packages_refs checkout (`-f packages_refs=pk=rc/0.27.0`, tree pk)
#   lora_a, lora_b, lora_c  Krea 2 LoRAs on the Hub (defaults below)
#   seed     both boxes' seed (default 20260926)
#   need_s   what the cap must buy (default 3600)
. "$(dirname "$0")/lib.sh"
LOGS_FROM=$(date -u +%s) ENDED=''
pod_logs() { cozy machine logs "$NAME" --since "$(( ($(date -u +%s) - LOGS_FROM) / 60 + 2 ))m" -n 0 > "$OUT/tensord-$NAME.log" 2>&1 || true; }
fail() {
  [ -z "${NAME:-}" ] || [ -n "$ENDED" ] || pod_logs
  cozy machine logs local calcifer --lines 20000 > "$OUT/calcifer.log" 2>&1
  say "FAIL: $*"; RC=1; exit 1
}
TREE=$(param tree '') NEED=$(param need_s 3600) SEED=$(param seed 20260926)
LORA_A=$(param lora_a conradlocke/general-edit@0.1.0) LORA_B=$(param lora_b gokaygokay/krea-2-realism@0.1.1)
LORA_C=$(param lora_c krea/krea-2-darkbrush@0.1.1) ORB=matlowai/orb360
[ -n "$TREE" ] && [ -f "$PACKAGES/$TREE/krea-2/pyproject.toml" ] && [ -f "$PACKAGES/$TREE/orbit/pyproject.toml" ] && [ -f "$PACKAGES/$TREE/minimax-h3/pyproject.toml" ] ||
  fail "dispatch with -f packages_refs=<name>=<packages ref> and params.tree=<name>"
SOURCE=$(cd "$PACKAGES/$TREE" && pwd)
V=$(cozy --version | awk '{print $2}')
[[ $V == *-dev* ]] || fail "cozy $V: a dev build (pod-session cozy=<dev-build run id>)"
for tool in jq curl python3; do command -v "$tool" > /dev/null || fail "no $tool"; done
[ -f "$REF/seed.jpg" ] || fail "no $REF/seed.jpg"
cfg=$HOME/.cozy/config.toml
mkdir -p "${cfg%/*}" && touch "$cfg" || fail "no $cfg"
{ echo 'dev = true'; grep -v '^dev *=' "$cfg" || true; } > "$cfg.new" && mv "$cfg.new" "$cfg" || fail "dev = true not written"
awk -v source="$SOURCE" '
  function ours() { print "diagnostics = true"; print "packages = \"" source "\""; put = 1 }
  /^(diagnostics|packages) *=/ { next } { print } /^\[calcifer\]/ { ours() } END { if (!put) { printf "\n[calcifer]\n"; ours() } }' \
  "$cfg" > "$cfg.next" && mv "$cfg.next" "$cfg" || fail "[calcifer] not written"
cozy restart > "$OUT/restart.txt" 2>&1 || fail "cozy restart: $(tail -c 300 "$OUT/restart.txt")"
kernels "$SOURCE/krea-2"
kernels "$SOURCE/minimax-h3"
now() { date +%s%3N; }
repo() { echo "${1%@*}"; }
A=$(repo "$LORA_A") B=$(repo "$LORA_B") C=$(repo "$LORA_C")

# The project's picture and boxes, all drafted, none rendered.
box() { jq -nc --arg w "$1" --arg l "$2" --argjson s "$SEED" '{instructions: $w, megapixels: 1, seed: $s, lora: [{ref: $l}]}'; }
run_page import "$REF/seed.jpg" --name pic > "$OUT/pic.json" 2>&1 || fail "import pic: $(tail -c 300 "$OUT/pic.json")"
run_page add first --tool krea-2/image --fields "$(box 'A lighthouse on a cliff at dawn, waves breaking below.' "$LORA_C")" > "$OUT/add-first.json" 2>&1 ||
  fail "first: $(tail -c 400 "$OUT/add-first.json")"
run_page add styled --tool krea-2/image --fields "$(box 'A red bicycle against a white wall.' "$LORA_A")" > "$OUT/add-styled.json" 2>&1 ||
  fail "styled: $(tail -c 400 "$OUT/add-styled.json")"
run_page add turn --tool orbit/orbit --fields "$(jq -nc --argjson s "$SEED" '{instructions: "<pic>", seed: $s}')" > "$OUT/add-turn.json" 2>&1 ||
  fail "turn: $(tail -c 400 "$OUT/add-turn.json")"
for b in first styled turn; do run_page asset "$b" | jq -c '{name: .asset.name, tool: .asset.tool, fields: .asset.fields}' >> "$OUT/boxes.ndjson" 2> /dev/null; done
say "cozy $V, a dev home; krea-2 $(sed -n 's/^version = "\(.*\)"/\1/p' "$SOURCE/krea-2/pyproject.toml" | head -1), orbit $(sed -n 's/^version = "\(.*\)"/\1/p' "$SOURCE/orbit/pyproject.toml" | head -1), minimax-h3 $(sed -n 's/^version = "\(.*\)"/\1/p' "$SOURCE/minimax-h3/pyproject.toml" | head -1) from $TREE $(git -C "$SOURCE" rev-parse --short=8 HEAD 2> /dev/null); boxes: $(tr '\n' ' ' < "$OUT/boxes.ndjson" | cut -c1-900)"

# One quote and one order: an out-of-stock answer ends the session, never ordered again.
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
RIDS+=("$RID"); echo "$RID" >> "$OUT/rental"; SKU_0=$SKU
say "rental $RID ordered"
guard
ready
(cd "$RUN_PROJECT" && cozy --json machine use "$NAME") > "$OUT/use.json" 2>&1 || fail "machine use $NAME: $(tail -c 300 "$OUT/use.json")"

# The pod's Status each second: its list items' readiness and each model's bytes (its own, all, whole).
( while :; do
    printf '{"t":%s,"s":%s}\n' "$(now)" "$(cozy --json machine show "$NAME" 2> /dev/null | jq -c '{v: .status.version,
      ahead: [.status.ahead[]? | {id, state, stage: .readiness.stage, done: .readiness.done, total: .readiness.total, reason: .readiness.reason}],
      models: [.status.models[]? | {r: .repository, all: .total_bytes, own: .unique_bytes, whole: ([.checkpoints[]?.complete] | any)}]}' 2> /dev/null || echo null)"
    sleep 1
  done >> "$OUT/samples.ndjson" ) & BG="$BG $!"
# The open RC's builds on $NAME (its Runtime among them), then the session's TensorD, as needs-ahead.sh does.
version() { cozy --json machine show "$NAME" 2> /dev/null | jq -r '.status.version // empty'; }
ours() {
  local tries=0 runs
  while runs=$(version); [ "$runs" != "$V" ]; do
    [ "$tries" -lt 3 ] || fail "$NAME doesn't keep the session's TensorD $V: it runs ${runs:-nothing it says}"
    step tensord machine update "$NAME" --tensord "$(command -v cozy)" -y || fail "$NAME refused the session's TensorD"
    sleep 30; tries=$((tries + 1))
  done
}
step rc machine update "$NAME" -y || fail "$NAME didn't move to the open RC's builds"
ours
say "$NAME runs the session's TensorD $V"

shown() { cozy --json machine show "$NAME" > "$1" 2> /dev/null || true; }
held() { jq -r --arg m "$2" '[.status.models[]? | select(.repository == $m) | .unique_bytes // 0] | add // 0' "$1" 2> /dev/null || echo 0; }
whole() { jq -e --arg m "$2" '[.status.models[]? | select(.repository == $m) | .checkpoints[]?.complete] | any' "$1" > /dev/null 2>&1 && echo yes || echo no; }
models() { jq -c '[.status.models[]? | {r: .repository, all: .total_bytes, own: .unique_bytes, whole: ([.checkpoints[]?.complete] | any)}]' "$1" 2> /dev/null; }
loras() { local l; for l in "$A" "$B" "$C" "$ORB"; do printf '%s own %s whole %s; ' "$l" "$(held "$1" "$l")" "$(whole "$1" "$l")"; done; }
row() { run_page board | jq -c --arg n "$1" '[.ahead[]? | select((.names // []) | index($n))] | max_by(.run) // empty' 2> /dev/null; }
is_ready() { jq -e --arg m "$NAME" '.ready == true and .machine == $m' <<< "${1:-null}" > /dev/null 2>&1; }
record() {
  RUN_N=$(run_page asset "$1" | jq -r '.versions[-1].runs[-1] // empty')
  [ -n "$RUN_N" ] && (cd "$RUN_PROJECT" && cozy --json --full run watch "$RUN_N" --once) > "$2" 2>&1
}
# listing <label>: what `cozy lora ls` and `cozy model ls` say $NAME holds.
listing() {
  (cd "$RUN_PROJECT" && cozy lora ls --on "$NAME" < /dev/null) > "$OUT/loras-$1.txt" 2>&1 || true
  (cd "$RUN_PROJECT" && cozy model ls --on "$NAME" < /dev/null) > "$OUT/models-$1.txt" 2>&1 || true
}
# rendered <box>: the box rendered on $NAME, its record in <box>.json; said: its quote, what it waited for, and the LoRAs after.
rendered() {
  local began at
  began=$(now)
  timeout 3000 python3 "$RUN_PAGE" render "$RUN_PROJECT" --on "$NAME" "$1" > "$OUT/render-$1.txt" 2>&1 || { say "$1's render failed: $(tail -c 400 "$OUT/render-$1.txt")"; RC=1; }
  at=$(now)
  record "$1" "$OUT/$1.json" || { say "$1 has no run record: $(tail -c 300 "$OUT/render-$1.txt")"; RC=1; return 0; }
  shown "$OUT/after-$1.json"
  say "$1 rendered in $((at - began)) ms: $(jq -c '{status, created_at_ms, accepted_at_ms, started_at_ms, settled_at_ms, placed, weights: .measurements.weights}' "$OUT/$1.json" 2> /dev/null | cut -c1-1500)"
  say "$1, after its render: $(loras "$OUT/after-$1.json")"
  (cd "$RUN_PROJECT" && cozy run watch "$RUN_N" --once -o "$OUT/out-$1") > /dev/null 2> "$OUT/out-$1.err" || true
}

say "step 1: first (krea-2/image, $LORA_C on its box) rendered on the cold $NAME, no page showing the project"
shown "$OUT/cold.json"; say "cold: $(models "$OUT/cold.json")"
rendered first
quoted=$(jq -r '.placed.fetch_bytes // empty' "$OUT/first.json") waited=$(jq -r '.measurements.weights.needs // empty' "$OUT/first.json")
say "first: quoted ${quoted:-nothing} bytes, waited for ${waited:-nothing}; $NAME holds $(models "$OUT/after-first.json")"
listing after-first

say "step 2: a page shows the project; styled ($LORA_A) and turn (Orbit) are guessed on $NAME"
ours
( exec python3 -I "$RUN_PAGE" show "$RUN_PROJECT" > "$OUT/show.json" 2>&1 ) & SHOW=$!; BG="$BG $SHOW"
SHOWN_MS=$(now)
printf 'unix_ms\ta_own\ta_whole\tb_own\tb_whole\torb_own\torb_whole\tstyled\tturn\n' > "$OUT/series.tsv"
sample() {
  shown "$OUT/now.json"
  A_OWN=$(held "$OUT/now.json" "$A") A_WHOLE=$(whole "$OUT/now.json" "$A")
  printf '%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\n' "$(now)" "$A_OWN" "$A_WHOLE" "$(held "$OUT/now.json" "$B")" "$(whole "$OUT/now.json" "$B")" \
    "$(held "$OUT/now.json" "$ORB")" "$(whole "$OUT/now.json" "$ORB")" "${1:-}" "${2:-}" >> "$OUT/series.tsv"
}
# Until $LORA_A's own bytes come (a header alone is under 4 MiB), it is whole, or 5 min.
began=$SECONDS
while [ $((SECONDS - began)) -lt 300 ]; do
  sample
  [ "${A_OWN:-0}" -gt $((4 << 20)) ] || [ "$A_WHOLE" = yes ] && break
  sleep 0.3
done
say "step 3: +$(( $(now) - SHOWN_MS )) ms after the page showed it, $NAME holds $A_OWN bytes of $A (whole $A_WHOLE); styled's lora becomes $LORA_B"
EDIT_MS=$(now)
run_page set styled --fields "$(jq -nc --arg l "$LORA_B" '{lora: [{ref: $l}]}')" > "$OUT/set.json" 2>&1 || fail "styled's edit: $(tail -c 300 "$OUT/set.json")"
say "styled's draft changed at $EDIT_MS (Unix ms)"

# Step 4: until both are ready on $NAME (40 min at most), then until nothing more lands for 30 s (10 min at most): Orbit's extras.
began=$SECONDS; R_STYLED='' R_TURN=''
while [ $((SECONDS - began)) -lt 2400 ]; do
  R_STYLED=$(row styled) R_TURN=$(row turn)
  sample "$(jq -c '{ready, stage, machine}' <<< "${R_STYLED:-null}" 2> /dev/null)" "$(jq -c '{ready, stage, machine}' <<< "${R_TURN:-null}" 2> /dev/null)"
  is_ready "$R_STYLED" && is_ready "$R_TURN" && break
  sleep 2
done
is_ready "$R_STYLED" || { say "styled isn't ready on $NAME after $((SECONDS - began)) s: ${R_STYLED:-no row}"; RC=1; }
is_ready "$R_TURN" || { say "turn isn't ready on $NAME after $((SECONDS - began)) s: ${R_TURN:-no row}"; RC=1; }
READY_MS=$(now)
say "step 4, both ready +$((READY_MS - SHOWN_MS)) ms after the page showed them: styled's row ${R_STYLED:-none}; turn's row ${R_TURN:-none}"
say "at ready: $(loras "$OUT/now.json")"
began=$SECONDS; last='' still=0
while [ $((SECONDS - began)) -lt 600 ] && [ "$still" -lt 30 ]; do
  shown "$OUT/now.json"; sum=$(jq -r '[.status.models[]?.total_bytes // 0] | add // 0' "$OUT/now.json" 2> /dev/null)
  if [ "$sum" = "$last" ]; then still=$((still + 5)); else still=0; last=$sum; fi
  sleep 5
done
cp "$OUT/now.json" "$OUT/before-render.json"
say "before Render, nothing landing for ${still}s (+$(( $(now) - SHOWN_MS )) ms): $(loras "$OUT/before-render.json")"
say "before Render, $NAME holds: $(models "$OUT/before-render.json")"
listing before-render
pod_logs
grep -E "cozy:prepare:[^ ]*: its request" "$OUT/tensord-$NAME.log" | sed 's/^\(.\{27\}\).*INFO [a-z:_]*: /\1 /' | cut -c1-1100 | while read -r line; do say "plan: $line"; done
grep -E "list item cozy:prepare:[^ ]* dropped" "$OUT/tensord-$NAME.log" | sed 's/^\(.\{27\}\).*INFO [a-z:_]*: /\1 /' | cut -c1-300 | while read -r line; do say "drop: $line"; done
at_edit=$(awk -F'\t' -v e="$EDIT_MS" 'NR > 1 && $1 <= e {b = $2} END {print b + 0}' "$OUT/series.tsv")
grew=$(awk -F'\t' 'NR > 1 {if ($2 > last) {at = $1}; last = $2} END {print at + 0}' "$OUT/series.tsv")
say "$A's own bytes: $at_edit at the edit, $(held "$OUT/before-render.json" "$A") before Render (whole $(whole "$OUT/before-render.json" "$A")); last grew $((grew - EDIT_MS)) ms after the edit"

say "step 5: styled rendered on $NAME, then turn"
ours
rendered styled
listing after-styled
say "bare: cozy run krea-2/image --lora $LORA_C, naming no slot, on $NAME"
step bare run "$SOURCE/krea-2/image" "instructions=A red bicycle against a white wall." megapixels:=1 seed:="$SEED" --lora "$LORA_C" --on "$NAME" ||
  { say "the bare --lora run failed: $(tail -c 400 "$OUT/bare.txt")"; RC=1; }
BARE_N=$(runno bare)
if [ -n "$BARE_N" ]; then
  (cd "$RUN_PROJECT" && cozy --json --full run watch "$BARE_N" --once) > "$OUT/bare.json" 2>&1
  say "bare --lora, run $BARE_N: $(jq -c '{status, target, placed, weights: .measurements.weights, adapted: [.measurements.models[]? | select((.adapters // []) | length > 0) | {slot, repository, adapters: [.adapters[] | {model, strength, components, layers}]}]}' "$OUT/bare.json" 2> /dev/null | cut -c1-1500)"
fi
rendered turn
listing after-turn
say "plain: birefnet, a package with no LoRA slot, on $NAME"
step plain run "$SOURCE/birefnet/remove_background" --file "image=$REF/seed.jpg" --on "$NAME" || { say "birefnet failed: $(tail -c 400 "$OUT/plain.txt")"; RC=1; }
PLAIN_N=$(runno plain)
if [ -n "$PLAIN_N" ]; then
  (cd "$RUN_PROJECT" && cozy --json --full run watch "$PLAIN_N" --once) > "$OUT/plain.json" 2>&1
  say "plain, run $PLAIN_N: $(jq -c '{status, placed, weights: .measurements.weights, models: [.measurements.models[]? | {slot, repository, adapters}]}' "$OUT/plain.json" 2> /dev/null | cut -c1-900)"
fi
pod_logs
cozy machine logs local calcifer --lines 20000 > "$OUT/calcifer.log" 2>&1 || true
grep -E "run run-[0-9a-f]+ (is the request|names)" "$OUT/tensord-$NAME.log" | sed 's/.*INFO tensord::ahead: //' | cut -c1-300 | while read -r line; do say "adopted: $line"; done
grep -iE "adapter|orb360" "$OUT/tensord-$NAME.log" | sed 's/^\(.\{27\}\).*\(INFO\|WARN\) [a-z:_]*: /\1 /' | cut -c1-400 | tail -40 > "$OUT/adapter-lines.txt"
say "TensorD's lines naming an adapter or ORB360: $(wc -l < "$OUT/adapter-lines.txt") (adapter-lines.txt)"
say "$NAME ends now, by name"
cozy machine end "$NAME" -y > "$OUT/end-by-name.txt" 2>&1 || cozy machine end "$NAME" -y --force >> "$OUT/end-by-name.txt" 2>&1
ENDED=1
say "END $NAME: $(tail -c 200 "$OUT/end-by-name.txt" | tr '\n' ' ')"
[ "$RC" = 0 ] && say "DONE: the facts are above; nothing here judges them"
