# lora-slots (cl-1281; Paul, 11-18: "fix the orbit lora so it's used correctly (follow platform rules) and all other packages should
# follow our platform-rules so that loras can be overwritten"): orbit, moss-image and minimax-h3 `edit` with their LoRAs as slots, on one
# rental, against the same boxes on the packages before them, same seed. Proof-branch harness, never merged. Every step through this
# runner's cozy and its Calcifer, in a dev home on a packages checkout (`old`, then `new`). Each render's record says what its calls'
# models took (`calls[].measurements.models[].adapters`: model, layers), its quote (`placed`) and what it waited for
# (`measurements.weights`); frames go to $OUT/frames for a person to open.
# 1. old: turn (Orbit), restyle (H3 edit, style_transfer, on a 5 s plate made here), depth and edit (MOSS), rendered.
# 2. new: the same boxes, guessed while a page shows them, then rendered: each slot's LoRA whole before Render, waited 0 B of.
# 3. new, overrides: Orbit's `lora`, the edit's `style_transfer`, MOSS's `depth` each replaced by another LoRA; a depth map at strength 0
#    (no LoRA, the twin); a Shot box whose `turbo_lora` is another LoRA (the nested segment calls take it); birefnet (no LoRA slot).
# 4. swap (`phases=swap`, `new` only): a page shows `styled` (krea-2/image, `swap_a` on its box) and `turn_swap` (Orbit, `models.lora`
#    `slot_a`); each is swapped (to `swap_b`, `slot_b`) once its old LoRA's own bytes come: whether the old download stops. Both rendered
#    (the new LoRA whole before Render, 0 B waited). Then a bare `--lora` on the CLI: krea-2/image (`bare_krea`) and Orbit (`bare`),
#    which slots each adapted. Every run the pod saw, by id, so a run another session pinned is seen (cl-1316).
# params:
#   old, new   the packages_refs checkouts (`-f packages_refs="old=rc/0.27.0 new=proof/cl-1281-lora-slots"`)
#   runtime    a cozy-runtime dev-build run id for the pod's Runtime (empty: the open RC's)
#   seed       every box's seed (default 20260926)
#   phases     all (default); overrides: step 3 alone on `new`, after the defaults it compares with (turn, restyle, depth); swap: step 4
. "$(dirname "$0")/lib.sh"
fail() {
  [ -z "${NAME:-}" ] || cozy machine logs "$NAME" --since 240m -n 0 > "$OUT/tensord-$NAME.log" 2>&1 || true
  say "FAIL: $*"; RC=1; exit 1
}
OLD=$PACKAGES/$(param old old) NEW=$PACKAGES/$(param new new) SEED=$(param seed 20260926) RT=$(param runtime '') NEED=$(param need_s 5400) PHASES=$(param phases all)
[[ $PHASES =~ ^(all|overrides|swap)$ ]] || fail "phases $PHASES: all, overrides or swap"
OTHER_H3=alissonerdx/minimax-h3-vfx-edit@0.1.0 OTHER_MOSS=thedeoxen/moss-image-canny-controlnet@0.1.0
for t in "$OLD" "$NEW"; do
  [ "$PHASES" = all ] || [ "$t" = "$NEW" ] || continue
  for p in orbit moss-image minimax-h3 krea-2; do [ -f "$t/$p/pyproject.toml" ] || fail "no $t/$p"; done
done
V=$(cozy --version | awk '{print $2}')
[[ $V == *-dev* ]] || fail "cozy $V: a dev build (pod-session cozy=<dev-build run id>)"
command -v ffmpeg > /dev/null || fail "no ffmpeg"
mkdir -p "$OUT/frames" "$OUT/inputs"
W=''
[ -z "$RT" ] || { W=$(wheel_from "$OUT/wheels/runtime" "$RT") && [ -n "$W" ] || fail "no Runtime wheel from run $RT"; }
# tree <dir>: the dev home's packages are that checkout's, read at Calcifer's restart.
tree() {
  local cfg=$HOME/.cozy/config.toml
  mkdir -p "${cfg%/*}" && touch "$cfg"
  { echo 'dev = true'; grep -v '^dev *=' "$cfg" || true; } > "$cfg.new" && mv "$cfg.new" "$cfg"
  awk -v source="$1" '
    function ours() { print "diagnostics = true"; print "packages = \"" source "\""; put = 1 }
    /^(diagnostics|packages) *=/ { next } { print } /^\[calcifer\]/ { ours() } END { if (!put) { printf "\n[calcifer]\n"; ours() } }' \
    "$cfg" > "$cfg.next" && mv "$cfg.next" "$cfg"
  cozy restart > "$OUT/restart.txt" 2>&1 || fail "cozy restart: $(tail -c 300 "$OUT/restart.txt")"
  say "packages: $1 ($(git -C "$1" rev-parse --short=8 HEAD 2> /dev/null))"
}
for t in "$OLD" "$NEW"; do [ "$PHASES" = all ] || [ "$t" = "$NEW" ] || continue; kernels "$t/minimax-h3"; kernels "$t/moss-image"; done
[ "$PHASES" != swap ] || kernels "$NEW/krea-2"

# Inputs: the picture, a 5 s plate panning over it, and a depth map (near is white) of its size.
cp "$REF/seed.jpg" "$OUT/inputs/pic.jpg"
ffmpeg -loglevel error -y -loop 1 -i "$OUT/inputs/pic.jpg" -t 5 -r 24 -vf "scale=1280:-2,crop=960:540:'(iw-960)*t/5':0,format=yuv420p" "$OUT/inputs/plate.mp4" ||
  fail "no plate"
ffmpeg -loglevel error -y -f lavfi -i "gradients=s=1024x1024:c0=white:c1=black:type=radial:x0=512:y0=600:x1=1024:y1=1024,format=rgb24" -frames:v 1 \
  "$OUT/inputs/depth.png" || fail "no depth map"
run_page import "$OUT/inputs/pic.jpg" --name pic > "$OUT/pic.json" 2>&1 || fail "import pic: $(tail -c 300 "$OUT/pic.json")"
run_page import "$OUT/inputs/plate.mp4" --name plate > "$OUT/plate.json" 2>&1 || fail "import plate: $(tail -c 300 "$OUT/plate.json")"
run_page import "$OUT/inputs/depth.png" --name depthmap > "$OUT/depthmap.json" 2>&1 || fail "import depth: $(tail -c 300 "$OUT/depthmap.json")"
turn() { jq -nc --argjson s "$SEED" '{instructions: "<pic>", seed: $s}'; }
restyle() { jq -nc --argjson s "$SEED" --arg t "$1" '{plate: "<plate>", edit: "style_transfer", seed: $s, summary: "\($t)<pic> painted in thick oil brushstrokes.",
  beats: [{duration: 5, description: "\($t)The scene from the plate, painted in thick oil brushstrokes."}]}'; }
depth() { jq -nc --argjson s "$SEED" --argjson g "$1" '{instructions: "A stone tower on a hill at dusk.", megapixels: 1, seed: $s, control: [{image: "<depthmap>", kind: "depth", strength: $g}]}'; }
moss_edit() { jq -nc --argjson s "$SEED" '{instructions: "<pic> as a watercolor painting.", megapixels: 1, seed: $s}'; }
add() { run_page add "$1" --tool "$2" --fields "$3" > "$OUT/add-$1.json" 2>&1 || fail "$1: $(tail -c 400 "$OUT/add-$1.json")"; }
shown() { cozy --json machine show "$NAME" > "$1" 2> /dev/null || true; }
held() { jq -r --arg m "${2%@*}" '[.status.models[]? | select(.repository == $m) | .unique_bytes // 0] | add // 0' "$1" 2> /dev/null || echo 0; }
whole() { jq -e --arg m "${2%@*}" '[.status.models[]? | select(.repository == $m) | .checkpoints[]?.complete] | any' "$1" > /dev/null 2>&1 && echo yes || echo no; }
record() {
  local n
  n=$(run_page asset "$1" | jq -r '.versions[-1].runs[-1] // empty')
  [ -n "$n" ] && (cd "$RUN_PROJECT" && cozy --json --full run watch "$n" --once) > "$OUT/$1.json" 2>&1
}
took() { jq -c '{status, placed: (.placed | {fetch_bytes, prepare_ms}), weights: .measurements.weights,
  own: [.measurements.models[]? | select((.adapters // []) | length > 0) | {slot, adapters: [.adapters[] | {model, strength, layers}]}],
  calls: [.calls[]? | {function, adapted: [.measurements.models[]? | select((.adapters // []) | length > 0) | {slot, adapters: [.adapters[] | {model, strength, layers}]}]}
    | select(.adapted | length > 0)]}' "$OUT/$1.json" 2> /dev/null | cut -c1-1800; }
# frames <box>: a video's frames at 0, 1/4, 1/2 and 3/4 of it, or the picture, into $OUT/frames.
frames() {
  local f
  # Its picture or video, not a latents file an edit keeps beside it.
  f=$(run_page asset "$1" | jq -r '[.versions[-1].files | to_entries[] | .value.path // empty | select(test("[.](mp4|png|jpe?g|webp)$"))] | first // empty')
  [ -n "$f" ] && [ -s "$RUN_PROJECT/$f" ] || { say "$1: no file"; return 0; }
  case $f in
    *.mp4) local n q
      n=$(ffprobe -v error -count_frames -select_streams v:0 -show_entries stream=nb_read_frames -of csv=p=0 "$RUN_PROJECT/$f")
      for q in 0 1 2 3; do ffmpeg -loglevel error -y -i "$RUN_PROJECT/$f" -vf "select=eq(n\\,$((n * q / 4)))" -frames:v 1 "$OUT/frames/$1-$((q * 90)).png"; done
      cp "$RUN_PROJECT/$f" "$OUT/frames/$1.mp4" ;;
    *) cp "$RUN_PROJECT/$f" "$OUT/frames/$1.${f##*.}" ;;
  esac
}
rendered() {
  local began=$SECONDS
  timeout 3000 python3 "$RUN_PAGE" render "$RUN_PROJECT" --on "$NAME" "$1" > "$OUT/render-$1.txt" 2>&1 || { say "$1's render failed: $(tail -c 400 "$OUT/render-$1.txt")"; RC=1; }
  record "$1" || { say "$1: no record"; RC=1; return 0; }
  say "$1 rendered in $((SECONDS - began)) s: $(took "$1")"
  frames "$1"
}

if [ "$PHASES" = all ]; then
  tree "$OLD"
  say "cozy $V; boxes before: turn, restyle, depth, moss_edit"
  add turn_old orbit/orbit "$(turn)"
  add restyle_old minimax-h3/edit "$(restyle "")"
  add depth_old moss-image/image "$(depth 1)"
  add moss_edit_old moss-image/image "$(moss_edit)"
else
  tree "$NEW"
fi
if [ "$PHASES" = swap ]; then
  SWAP_A=$(param swap_a conradlocke/general-edit@0.1.0) SWAP_B=$(param swap_b gokaygokay/krea-2-realism@0.1.1)
  SLOT_A=$(param slot_a alissonerdx/minimax-h3-vfx-edit@0.1.1) SLOT_B=$(param slot_b matlowai/orb360@0.1.1)
  BARE=$(param bare jojocodex/minimax-h3-spatial-physics@0.1.1) BARE_KREA=$(param bare_krea krea/krea-2-darkbrush@0.1.1)
  krea() { jq -nc --arg l "$1" --argjson s "$SEED" '{instructions: "A red bicycle against a white wall.", megapixels: 1, seed: $s, lora: [{ref: $l}]}'; }
  slotted() { turn | jq -c --arg l "$1" '. + {models: {lora: {ref: $l}}}'; }
  add styled krea-2/image "$(krea "$SWAP_A")"
  add turn_swap orbit/orbit "$(slotted "$SLOT_A")"
  say "swap boxes: styled (krea-2/image, box lora $SWAP_A), turn_swap (orbit/orbit, models.lora $SLOT_A); no page shows them yet"
fi
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
(cd "$RUN_PROJECT" && cozy --json machine use "$NAME") > "$OUT/use.json" 2>&1 || fail "machine use $NAME"
step rc machine update "$NAME" -y || fail "$NAME didn't move to the open RC's builds"
[ -z "$W" ] || step runtime machine update "$NAME" --runtime "$W" -y || fail "$NAME refused the Runtime $W"
step tensord machine update "$NAME" --tensord "$(command -v cozy)" -y || fail "$NAME refused the session's TensorD"
say "$NAME: $(cozy --json machine show "$NAME" | jq -c '{v: .status.version, runtime: .status.runtime}')"
[ "$PHASES" = all ] && for b in turn_old restyle_old depth_old moss_edit_old; do rendered "$b"; done

[ "$PHASES" = all ] && {
tree "$NEW"
say "the same boxes on the new packages; the edit's trigger in its summary and beat as the page puts it"
add turn_new orbit/orbit "$(turn)"
add restyle_new minimax-h3/edit "$(restyle "style_transfer: ")"
add depth_new moss-image/image "$(depth 1)"
add moss_edit_new moss-image/image "$(moss_edit)"
( exec python3 -I "$RUN_PAGE" show "$RUN_PROJECT" > "$OUT/show.json" 2>&1 ) & SHOW=$!; BG="$BG $SHOW"
began=$SECONDS
row() { run_page board | jq -c --arg n "$1" '[.ahead[]? | select((.names // []) | index($n))] | max_by(.run) // empty' 2> /dev/null; }
is_ready() { jq -e --arg m "$NAME" '.ready == true and .machine == $m' <<< "${1:-null}" > /dev/null 2>&1; }
until [ $((SECONDS - began)) -ge 2400 ]; do
  all=1; for b in turn_new restyle_new depth_new moss_edit_new; do is_ready "$(row "$b")" || all=''; done
  [ -n "$all" ] && break; sleep 5
done
shown "$OUT/before-render.json"
for l in matlowai/orb360 alissonerdx/minimax-h3-style-transfer thedeoxen/moss-image-depth-controlnet chinoll/moss-image-edit; do
  say "before Render: $l own $(held "$OUT/before-render.json" "$l") whole $(whole "$OUT/before-render.json" "$l")"
done
cozy machine logs "$NAME" --since 240m -n 0 > "$OUT/tensord-$NAME.log" 2>&1 || true
grep -E "cozy:prepare:[^ ]*: its request" "$OUT/tensord-$NAME.log" | cut -c1-1200 | while read -r line; do say "plan: $line"; done
for b in turn_new restyle_new depth_new moss_edit_new; do rendered "$b"; done
}

if [ "$PHASES" = overrides ]; then
  say "the defaults each override is compared with, on this card"
  add turn_new orbit/orbit "$(turn)"
  add restyle_new minimax-h3/edit "$(restyle "style_transfer: ")"
  add depth_new moss-image/image "$(depth 1)"
  for b in turn_new restyle_new depth_new; do rendered "$b"; done
fi

if [ "$PHASES" = swap ]; then
  now() { date +%s%3N; }
  A=${SWAP_A%@*} B=${SWAP_B%@*} SA=${SLOT_A%@*} SB=${SLOT_B%@*}
  row() { run_page board | jq -c --arg n "$1" '[.ahead[]? | select((.names // []) | index($n))] | max_by(.run) // empty' 2> /dev/null; }
  is_ready() { jq -e --arg m "$NAME" '.ready == true and .machine == $m' <<< "${1:-null}" > /dev/null 2>&1; }
  say "swap: a page shows styled and turn_swap, guessed on $NAME"
  ( exec python3 -I "$RUN_PAGE" show "$RUN_PROJECT" > "$OUT/show.json" 2>&1 ) & SHOW=$!; BG="$BG $SHOW"
  SHOWN_MS=$(now)
  printf 'unix_ms\t%s own\twhole\t%s own\twhole\t%s own\twhole\t%s own\twhole\n' "$A" "$B" "$SA" "$SB" > "$OUT/series.tsv"
  sample() {
    shown "$OUT/now.json"
    A_OWN=$(held "$OUT/now.json" "$A") A_WHOLE=$(whole "$OUT/now.json" "$A") SA_OWN=$(held "$OUT/now.json" "$SA") SA_WHOLE=$(whole "$OUT/now.json" "$SA")
    printf '%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\n' "$(now)" "$A_OWN" "$A_WHOLE" "$(held "$OUT/now.json" "$B")" "$(whole "$OUT/now.json" "$B")" \
      "$SA_OWN" "$SA_WHOLE" "$(held "$OUT/now.json" "$SB")" "$(whole "$OUT/now.json" "$SB")" >> "$OUT/series.tsv"
  }
  # Each box is swapped once its old LoRA's own bytes come (a header alone is under 4 MiB) or it is whole; 10 min at most.
  coming() { [ "${1:-0}" -gt $((4 << 20)) ] || [ "$2" = yes ]; }
  swap_styled() {
    EDIT_A=$(now)
    run_page set styled --fields "$(jq -nc --arg l "$SWAP_B" '{lora: [{ref: $l}]}')" > "$OUT/set-styled.json" 2>&1 || fail "styled's swap: $(tail -c 300 "$OUT/set-styled.json")"
    say "styled swapped at +$((EDIT_A - SHOWN_MS)) ms: $NAME held $A_OWN B of $A (whole $A_WHOLE); its lora is now $SWAP_B"
  }
  swap_turn() {
    EDIT_SA=$(now)
    run_page set turn_swap --fields "$(slotted "$SLOT_B")" > "$OUT/set-turn_swap.json" 2>&1 || fail "turn_swap's swap: $(tail -c 300 "$OUT/set-turn_swap.json")"
    say "turn_swap swapped at +$((EDIT_SA - SHOWN_MS)) ms: $NAME held $SA_OWN B of $SA (whole $SA_WHOLE); its models.lora is now $SLOT_B"
  }
  EDIT_A='' EDIT_SA='' began=$SECONDS
  while [ $((SECONDS - began)) -lt 600 ] && { [ -z "$EDIT_A" ] || [ -z "$EDIT_SA" ]; }; do
    sample
    [ -n "$EDIT_A" ] || ! coming "$A_OWN" "$A_WHOLE" || swap_styled
    [ -n "$EDIT_SA" ] || ! coming "$SA_OWN" "$SA_WHOLE" || swap_turn
    sleep 0.3
  done
  [ -n "$EDIT_A" ] || { say "styled: none of $A came in 600 s; swapped now"; swap_styled; }
  [ -n "$EDIT_SA" ] || { say "turn_swap: none of $SA came in 600 s; swapped now"; swap_turn; }
  # Until both are ready on $NAME (40 min at most), then until nothing more lands for 30 s (10 min at most).
  began=$SECONDS R_STYLED='' R_TURN=''
  while [ $((SECONDS - began)) -lt 2400 ]; do
    R_STYLED=$(row styled) R_TURN=$(row turn_swap)
    sample
    is_ready "$R_STYLED" && is_ready "$R_TURN" && break
    sleep 2
  done
  is_ready "$R_STYLED" || { say "styled isn't ready on $NAME: ${R_STYLED:-no row}"; RC=1; }
  is_ready "$R_TURN" || { say "turn_swap isn't ready on $NAME: ${R_TURN:-no row}"; RC=1; }
  say "both ready +$(( $(now) - SHOWN_MS )) ms after the page showed them"
  began=$SECONDS last='' still=0
  while [ $((SECONDS - began)) -lt 600 ] && [ "$still" -lt 30 ]; do
    sample; sum=$(jq -r '[.status.models[]?.total_bytes // 0] | add // 0' "$OUT/now.json" 2> /dev/null)
    if [ "$sum" = "$last" ]; then still=$((still + 5)); else still=0; last=$sum; fi
    sleep 5
  done
  cp "$OUT/now.json" "$OUT/before-render.json"
  for l in "$A" "$B" "$SA" "$SB"; do say "before Render, nothing landing for ${still}s: $l own $(held "$OUT/before-render.json" "$l") whole $(whole "$OUT/before-render.json" "$l")"; done
  cozy machine logs "$NAME" --since 240m -n 0 > "$OUT/tensord-$NAME.log" 2>&1 || true
  grep -E "cozy:prepare:[^ ]*: its request" "$OUT/tensord-$NAME.log" | cut -c1-1100 | while read -r line; do say "plan: $line"; done
  grep -E "list item cozy:prepare:[^ ]* dropped" "$OUT/tensord-$NAME.log" | cut -c1-300 | while read -r line; do say "drop: $line"; done
  # <column> <edit ms>: the old LoRA's own bytes at the edit, and when they last grew.
  after_edit() {
    local at grew
    at=$(awk -F'\t' -v c="$1" -v e="$2" 'NR > 1 && $1 <= e {b = $c} END {print b + 0}' "$OUT/series.tsv")
    grew=$(awk -F'\t' -v c="$1" 'NR > 1 {if ($c > last) {at = $1}; last = $c} END {print at + 0}' "$OUT/series.tsv")
    echo "$at B at the edit, last grew $((grew - $2)) ms after it"
  }
  say "$A (styled's old box lora): $(after_edit 2 "$EDIT_A"); before Render $(held "$OUT/before-render.json" "$A") B, whole $(whole "$OUT/before-render.json" "$A")"
  say "$SA (turn_swap's old models.lora): $(after_edit 6 "$EDIT_SA"); before Render $(held "$OUT/before-render.json" "$SA") B, whole $(whole "$OUT/before-render.json" "$SA")"
  for b in styled turn_swap; do rendered "$b"; done
  # A bare --lora names no slot: krea-2/image's DiT (`full`, `model`), and Orbit's call's DiT beside its `lora` slot's default.
  say "bare: cozy run krea-2/image --lora $BARE_KREA on $NAME"
  step bare_krea run "$NEW/krea-2/image" "instructions=A red bicycle against a white wall." megapixels:=1 seed:="$SEED" --lora "$BARE_KREA" \
    --on "$NAME" -o "$OUT/frames/bare_krea" || { say "bare_krea failed: $(tail -c 400 "$OUT/bare_krea.txt")"; RC=1; }
  say "bare: cozy run orbit/orbit --lora $BARE on $NAME"
  step bare_orbit run "$NEW/orbit/orbit" 'instructions=<pic>' "references:=$(jq -cn --arg f "$OUT/inputs/pic.jpg" '[{name: "pic", image: $f}]')" \
    seed:="$SEED" --lora "$BARE" --on "$NAME" -o "$OUT/frames/bare_orbit" || { say "bare_orbit failed: $(tail -c 400 "$OUT/bare_orbit.txt")"; RC=1; }
  for b in bare_krea bare_orbit; do
    n=$(runno "$b")
    [ -n "$n" ] || { say "$b: no run number"; RC=1; continue; }
    (cd "$RUN_PROJECT" && cozy --json --full run watch "$n" --once) > "$OUT/$b.json" 2>&1
    say "$b, run $n: $(took "$b")"
    say "$b's request models: $(jq -c '[.request.models[]? // empty]' "$OUT/$b.json" 2> /dev/null | cut -c1-600)"
  done
  # cl-1316: every run the pod saw, and which this project's runs don't name.
  cozy machine logs "$NAME" --since 240m -n 0 > "$OUT/tensord-$NAME.log" 2>&1 || true
  (cd "$RUN_PROJECT" && cozy --json --full run ls) > "$OUT/runs.json" 2>&1 || true
  grep -oE "run-[0-9a-f]{24}" "$OUT/tensord-$NAME.log" | sort -u > "$OUT/pod-runs.txt"
  ours=$(grep -oE "run-[0-9a-f]{24}" "$OUT/runs.json" | sort -u)
  strays=$(comm -23 "$OUT/pod-runs.txt" <(echo "$ours") | while read -r r; do grep -m1 "$r" "$OUT/tensord-$NAME.log" | cut -c1-240; done)
  say "the pod saw $(wc -l < "$OUT/pod-runs.txt") run ids; not this project's runs (machine updates, list items, others): $(echo "$strays" | grep -c . || true)"
  echo "$strays" > "$OUT/pod-runs-not-ours.txt"
fi

if [ "$PHASES" != swap ]; then
say "overrides, the twin without a LoRA, nested calls, and a package with no LoRA slot"
add turn_other orbit/orbit "$(turn | jq -c --arg l "$OTHER_H3" '. + {models: {lora: {ref: $l}}}')"
add restyle_other minimax-h3/edit "$(restyle "" | jq -c --arg l "$OTHER_H3" '. + {models: {style_transfer: {ref: $l}}}')"
add depth_other moss-image/image "$(depth 1 | jq -c --arg l "$OTHER_MOSS" '. + {models: {depth: {ref: $l}}}')"
add depth_none moss-image/image "$(depth 0)"
# A Shot box's turbo_lora reaches its nested segment calls: an edit LoRA there is refused by the segment's own slot (it keeps no
# fl2va_turbo), named `fl2va_segment.models.turbo_lora`, before anything runs (44506869385). No other LoRA fits that slot.
if run_page add shot_turbo --tool minimax-h3/generate --fields "$(jq -nc --argjson s "$SEED" --arg l "$OTHER_H3" '{summary: "<pic> turns slowly to the camera.", seed: $s,
  beats: [{duration: 5, description: "<pic> turns slowly to face the camera."}], models: {turbo_lora: {ref: $l}}}')" > "$OUT/add-shot_turbo.json" 2>&1; then
  say "shot_turbo: an edit LoRA on turbo_lora was taken: $(tail -c 300 "$OUT/add-shot_turbo.json")"; RC=1
else
  say "shot_turbo refused, as its nested segment call's slot reads it: $(grep -o '"message":"[^"]*"' "$OUT/add-shot_turbo.json" | head -1 | cut -c1-300)"
fi
add plain birefnet/remove_background "$(jq -nc '{image: "<pic>"}')"
for b in turn_other restyle_other depth_other depth_none plain; do rendered "$b"; done
# Same seed, before and after: identical bytes, or the mean pixel difference.
for p in "turn_old turn_new" "restyle_old restyle_new" "depth_old depth_new" "moss_edit_old moss_edit_new" "depth_new depth_other" \
  "depth_new depth_none" "turn_new turn_other" "restyle_new restyle_other"; do
  set -- $p
  a=$(ls "$OUT/frames/$1"-90.png "$OUT/frames/$1".png 2> /dev/null | head -1) b=$(ls "$OUT/frames/$2"-90.png "$OUT/frames/$2".png 2> /dev/null | head -1)
  [ -n "$a" ] && [ -n "$b" ] || continue
  d=$(python3 - "$a" "$b" <<'PY' 2> /dev/null
import sys
from PIL import Image, ImageChops, ImageStat
a, b = (Image.open(p).convert("RGB") for p in sys.argv[1:3])
b = b.resize(a.size)
print(f"{sum(ImageStat.Stat(ImageChops.difference(a, b)).mean) / 3:.2f}")
PY
)
  say "$1 vs $2: mean |diff| ${d:-?} (0-255)"
done
fi
cozy machine logs "$NAME" --since 240m -n 0 > "$OUT/tensord-$NAME.log" 2>&1 || true
grep -E "run run-[0-9a-f]+ (is the request|names|differs)" "$OUT/tensord-$NAME.log" | sed 's/.*INFO tensord::ahead: //' | cut -c1-300 | while read -r line; do say "adopted: $line"; done
say "$NAME ends now, by name"
cozy machine end "$NAME" -y > "$OUT/end-by-name.txt" 2>&1 || cozy machine end "$NAME" -y --force >> "$OUT/end-by-name.txt" 2>&1
say "END $NAME: $(tail -c 200 "$OUT/end-by-name.txt" | tr '\n' ' ')"
[ "$RC" = 0 ] && say "DONE: the facts and frames are above; nothing here judges them"
