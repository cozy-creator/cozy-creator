# lora-held (cl-1281; Paul: "loras are the cheapest and best thing to prefetch"): an H3 edit box on one pod of `sku`, its
# `style_transfer` slot at its default (`alissonerdx/minimax-h3-style-transfer@0.1.0`, which a machine resolves to its newest fix,
# 0.1.1), a project shown by a page so the box's guess makes its LoRA whole there before Render; then rendered. It judges that the
# render's quote counts what its run then fetched (`placed.fetch_bytes` == `measurements.weights.needs`, 0 with the LoRA whole): a quote
# that counts a held LoRA as still to come overstates the machine that has it (pod-session 44506869385: restyle_new quoted 692,932,748
# B to fetch, its run needed 0, as its quote named 0.1.0, which no machine pulls).
# params:
#   tree    the packages_refs checkout whose minimax-h3 runs (default pk)
#   seed    the box's seed (default 20260926)
#   need_s  the pod's time the quote asks for (default 2400)
. "$(dirname "$0")/lib.sh"
fail() {
  [ -z "${NAME:-}" ] || cozy machine logs "$NAME" tensord --since 2h -n 0 > "$OUT/tensord-pod.log" 2>&1 || true
  cozy machine logs local calcifer --lines 20000 > "$OUT/calcifer.log" 2>&1 || true
  say "FAIL: $*"; RC=1; exit 1
}
TREE=$(param tree pk) SEED=$(param seed 20260926) NEED=$(param need_s 2400)
[[ $SEED =~ ^[0-9]+$ && $NEED =~ ^[0-9]+$ ]] || fail "seed $SEED, need_s $NEED"
CHECKOUT=$(cd "$PACKAGES/$TREE" 2> /dev/null && pwd) || fail "no packages_refs checkout $TREE"
grep -q 'style_transfer: Weights' "$CHECKOUT/minimax-h3/h3.py" || fail "the checkout's minimax-h3 has no style_transfer slot"
V=$(cozy --version | awk '{print $2}')
[[ $V == *-dev* ]] || fail "cozy $V: a dev build (pod-session cozy=<dev-build run id>)"
for tool in jq ffmpeg python3; do command -v "$tool" > /dev/null || fail "no $tool"; done
# A dev home whose minimax-h3 is the checkout's.
SOURCE=$OUT/packages
mkdir -p "$SOURCE" "$OUT/inputs" && ln -sfn "$CHECKOUT/minimax-h3" "$SOURCE/minimax-h3" || fail "no packages folder"
cfg=$HOME/.cozy/config.toml
mkdir -p "${cfg%/*}" && touch "$cfg" || fail "no $cfg"
{ echo 'dev = true'; grep -v '^dev *=' "$cfg" || true; } > "$cfg.new" && mv "$cfg.new" "$cfg" || fail "dev = true not written"
awk -v source="$SOURCE" '
  function ours() { print "diagnostics = true"; print "packages = \"" source "\""; put = 1 }
  /^(diagnostics|packages) *=/ { next } { print } /^\[calcifer\]/ { ours() } END { if (!put) { printf "\n[calcifer]\n"; ours() } }' \
  "$cfg" > "$cfg.next" && mv "$cfg.next" "$cfg" || fail "[calcifer] not written"
cozy restart > "$OUT/restart.txt" 2>&1 || fail "cozy restart: $(tail -c 300 "$OUT/restart.txt")"
kernels "$SOURCE/minimax-h3"
say "cozy $V; minimax-h3 from $TREE ($(git -C "$CHECKOUT" rev-parse --short=8 HEAD 2> /dev/null))"
# The inputs: the picture and a 5 s plate panning over it.
cp "$REF/seed.jpg" "$OUT/inputs/pic.jpg" || fail "no seed picture"
ffmpeg -loglevel error -y -loop 1 -i "$OUT/inputs/pic.jpg" -t 5 -r 24 -vf "scale=1280:-2,crop=960:540:'(iw-960)*t/5':0,format=yuv420p" "$OUT/inputs/plate.mp4" ||
  fail "no plate"

quote "$NEED"
rent
in_project() { (cd "$RUN_PROJECT" && cozy "$@"); }
in_project --json machine use "$NAME" > "$OUT/use.json" 2>&1 || fail "machine use $NAME: $(tail -c 300 "$OUT/use.json")"
step rc machine update "$NAME" -y || fail "$NAME didn't move to the open RC's builds"
step tensord machine update "$NAME" --tensord "$(command -v cozy)" -y || fail "$NAME refused the session's TensorD"
say "$NAME: $(cozy --json machine show "$NAME" | jq -c '{v: .status.version, runtime: .status.runtime, gpu: .status.gpus[0].name}')"
python3 "$RUN_PAGE" show "$RUN_PROJECT" > "$OUT/show.json" 2> "$OUT/show.err" & BG="$BG $!"
for _ in $(seq 100); do grep -q '"held"' "$OUT/show.json" 2> /dev/null && break; sleep 0.1; done
grep -q '"held"' "$OUT/show.json" || fail "no page shows the project: $(tail -c 300 "$OUT/show.err")"
run_page import "$OUT/inputs/pic.jpg" --name pic > "$OUT/pic.json" 2>&1 || fail "import pic: $(tail -c 300 "$OUT/pic.json")"
run_page import "$OUT/inputs/plate.mp4" --name plate > "$OUT/plate.json" 2>&1 || fail "import plate: $(tail -c 300 "$OUT/plate.json")"
run_page add restyle --tool minimax-h3/edit --fields "$(jq -nc --argjson s "$SEED" '{plate: "<plate>", edit: "style_transfer", seed: $s,
  summary: "<pic> painted in thick oil brushstrokes.", beats: [{duration: 5, description: "The scene from the plate, painted in thick oil brushstrokes."}]}')" \
  > "$OUT/add-restyle.json" 2>&1 || fail "the restyle box: $(tail -c 400 "$OUT/add-restyle.json")"

# The guess: the box's LoRA whole on the pod before Render (and its row ready).
LORA=alissonerdx/minimax-h3-style-transfer
whole() { cozy --json machine show "$NAME" 2> /dev/null | jq -c --arg m "$LORA" '[.status.models[]? | select(.repository == $m) | {total_bytes, checkpoints: [.checkpoints[] | {release, complete}]}]'; }
row() { run_page board 2> /dev/null | jq -c '[.ahead[]? | select((.names // []) | index("restyle"))] | max_by(.run) // empty' 2> /dev/null; }
for _ in $(seq 360); do
  held=$(whole); r=$(row)
  jq -e 'any(.[]; any(.checkpoints[]; .complete == true))' <<< "${held:-[]}" > /dev/null 2>&1 && jq -e '.ready == true' <<< "${r:-null}" > /dev/null 2>&1 && break
  sleep 5
done
say "before Render: $LORA here: ${held:-none}; the guess: ${r:-none}"
judge() { if eval "$1"; then say "ok: $2"; else say "NOT: $2${3:+ ($3)}"; FAILS=$((FAILS + 1)); fi; }
FAILS=0
judge 'jq -e "any(.[]; any(.checkpoints[]; .complete == true))" <<< "${held:-[]}" > /dev/null 2>&1' "the box's LoRA is whole on $NAME before Render"

began=$SECONDS
timeout 3000 python3 "$RUN_PAGE" render "$RUN_PROJECT" --on "$NAME" restyle > "$OUT/render-restyle.txt" 2>&1 || say "restyle's render: $(tail -c 400 "$OUT/render-restyle.txt")"
n=$(run_page asset restyle | jq -r '.versions[-1].runs[-1] // empty')
[ -n "$n" ] && in_project --json --full run watch "$n" --once > "$OUT/restyle.json" 2>&1
say "restyle rendered in $((SECONDS - began)) s: $(jq -c '{number, status, placed: (.placed | {machine, basis, fetch_bytes, prepare_ms}), weights: .measurements.weights,
  style_transfer: [.measurements.models[]? | select(.slot == "style_transfer") | {release, manifest}]}' "$OUT/restyle.json" 2> /dev/null)"
judge 'jq -e ".status == \"completed\"" "$OUT/restyle.json" > /dev/null 2>&1' "restyle completed"
judge 'jq -e ".placed.fetch_bytes == .measurements.weights.needs" "$OUT/restyle.json" > /dev/null 2>&1' \
  "restyle's quote counts what its run fetched (placed.fetch_bytes == measurements.weights.needs)" \
  "$(jq -c '{quoted: .placed.fetch_bytes, needed: .measurements.weights.needs}' "$OUT/restyle.json" 2> /dev/null)"
cozy machine logs local calcifer --lines 20000 > "$OUT/calcifer.log" 2>&1 || true
[ "$FAILS" = 0 ] || fail "$FAILS of the judged facts did not hold"
say "PASS"
