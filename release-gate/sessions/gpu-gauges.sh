# gpu-gauges (cl-1191), Paul 11-17: "there was a plan to show details on a machine, like its GPU utilization, like a throttle, from 0 -> 100%,
# like what runpod has. I don't see it yet on the /machines page. Why not? I want to see how much GPUs are being utilized please."
# One rental of $SKU, its TensorD this runner's cozy (pod-session `cozy`: a dev build) by `machine update --tensord`. SDXL is warmed with one
# image, then `renders` more go one after another while Machines is open in Chrome (gpu-gauges.spec.js's GPU branch, GPU_MACHINE): its row's
# GPU 0 bar must reach 50% during them and fall to 10% after, each look logged twice a second (gauges.jsonl) and Machines and the rental's
# drawer screenshotted at 1440×900 and 1920×1080, paper and dark, during and after. `cozy machine show` prints the GPU's numbers idle,
# during and after, each beside the pod's own nvidia-smi.
# Without `rent`, $0: the quote only.
# params:
#   sdxl     the SDXL function (sdxl/generate)
#   renders  images after the warm one (6)
. "$(dirname "$0")/lib.sh"
WEB=$(cd "$(dirname "$0")/../../e2e/cli/web" && pwd)
SDXL=$(param sdxl sdxl/generate) RENDERS=$(param renders 6)
[[ $RENDERS =~ ^[1-9][0-9]?$ ]] || fail "renders $RENDERS"
for tool in jq node npm google-chrome; do command -v "$tool" > /dev/null || fail "no $tool"; done
BIN=$(command -v cozy)
say "this runner's cozy, the pod's TensorD to be: $("$BIN" --version)"
quote 1800
RENT_ARGS=--ssh
[ -f ~/.ssh/id_ed25519.pub ] || ssh-keygen -q -t ed25519 -N '' -f ~/.ssh/id_ed25519 || fail "ssh-keygen"
PW=$OUT/playwright-test
npm install -s --prefix "$PW" @playwright/test@1.55.0 > "$OUT/npm.log" 2>&1 || fail "npm install @playwright/test: $(tail -c 300 "$OUT/npm.log")"
rent
step tensord machine update "$NAME" --tensord "$BIN" -y || fail "the pod refused this cozy as its TensorD"
v=$(cozy machine show "$NAME" --json | jq -r .status.version)
[ "$v" = "$("$BIN" --version | awk '{print $2}')" ] || fail "the pod runs TensorD $v, not $("$BIN" --version)"

# look <label>: `cozy machine show`'s GPU lines and its JSON reading, beside the pod's nvidia-smi.
look() {
  cozy machine show "$NAME" > "$OUT/show-$1.txt" 2>&1
  cozy --json machine show "$NAME" 2> /dev/null | jq -c '.gpu_use // "none"' > "$OUT/show-$1.json"
  timeout 60 cozy machine ssh "$NAME" "nvidia-smi --query-gpu=index,utilization.gpu,memory.used,memory.total,power.draw,power.limit,temperature.gpu,clocks.sm,clocks_event_reasons.active --format=csv,noheader" \
    < /dev/null > "$OUT/smi-$1.txt" 2>&1
  say "$1: $(grep -E '^GPU [0-9]+ now' "$OUT/show-$1.txt" | tr -s ' ' || echo 'no GPU line')"
  say "$1: nvidia-smi $(grep -v '^Warning' "$OUT/smi-$1.txt" | paste -sd';')"
  grep -qE '^GPU 0 now' "$OUT/show-$1.txt" || { say "FAIL $1: cozy machine show printed no GPU 0 now"; RC=1; }
}
look idle

step warm run "$SDXL" --on "$NAME" 'prompt=a lighthouse on a cliff at dusk, photo' || fail "the warm SDXL image"

# Chrome on Machines from before the renders until after them.
say "page: Machines, during and after the renders"
env GPU_MACHINE="$NAME" GAUGE_LOG="$OUT/gauges.jsonl" HOLDER="$HOME/.cozy/calcifer/calcifer.lock.holder" COZY="$BIN" \
  CHROME="$(command -v google-chrome)" OUT="$OUT" ARGV_LOG="$OUT/argv.log" NODE_PATH="$PW/node_modules" \
  node "$PW/node_modules/@playwright/test/cli.js" test -c "$WEB/gpu.config.js" -g 'gpu-gauges: a rental under a render' > "$OUT/page.log" 2>&1 & page=$!
BG="$BG $page"
sleep 20
(
  for i in $(seq "$RENDERS"); do step "render-$i" run "$SDXL" --on "$NAME" "prompt=a lighthouse on a cliff at dusk, photo, take $i" "seed:=$i" || exit 1; done
) & renders=$!
BG="$BG $renders"
# During: once the warm image's run has been followed by a few seconds of the next.
sleep 25
look during
wait "$renders" || { say "FAIL a render failed: $(tail -3 "$OUT"/render-*.txt | tail -c 400)"; RC=1; }
sleep 20
look after
wait "$page"; code=$?
cp "$OUT/web/report.json" "$OUT/web/report-page.json" 2> /dev/null
node "$WEB/report.js" "$OUT/web/report-page.json" gpu-gauges > "$OUT/page.report" 2>&1
sed 's/^/    /' "$OUT/page.report" >> "$R"
[ "$code" = 0 ] || { say "FAIL page: exit $code ($(grep -m1 -E '^FAILED' "$OUT/page.report" | cut -c1-300))"; RC=1; }
find "$OUT/web/results" -name 'gauges-*.png' -exec cp {} "$OUT/" \; 2> /dev/null
jq -r 'select(.busy != null) | "\(.at) \(.phase) \(.busy)%"' "$OUT/gauges.jsonl" 2> /dev/null | awk 'NR % 10 == 1' | tee -a "$R"
exit "$RC"
