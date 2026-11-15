# render-estimate (cl-1004, Paul 11-15: "before the user pushes 'render' we should display to them some sort of estimated time"):
# what the page shows beside Render against what the render takes, on one rented card this runner's Calcifer never ran H3 on.
# The bench's H3 Turbo 15 s shot, a box of the project, whose machine is the pod alone (`cozy machine use`):
#   - `fit` as the page asks it before any run: the pod, its card, our measurement of that card (`api::estimate::reference`);
#   - rendered twice on the pod, the second warm, each run's own time from its record;
#   - `fit` again: now measured there.
# Passes when the first estimate is within 35% of the warm run. One rental of $SKU. params:
#   tool    the shot's tool (default minimax-h3/generate, the Hub's)
. "$(dirname "$0")/lib.sh"
TOOL=$(param tool minimax-h3/generate)
J() { jq -e "$@" > /dev/null; }
estimate() { run_page fit shot | jq -c '.estimates[0] // {}'; }
newest_run() { run_page asset shot | jq -r '.versions[-1].runs[-1]'; }

quote 3000
rent
step update machine update "$NAME" --tensord "$(command -v cozy)" -y || fail "update"
say "pod $(cozy machine show "$NAME" --json | jq -c '{v: .status.version, rt: .status.runtime, gpu: .status.gpus[0].name}')"
(cd "$RUN_PROJECT" && cozy machine use "$NAME") > "$OUT/use.txt" 2>&1 || fail "machine use $NAME: $(tail -c 300 "$OUT/use.txt")"

FIELDS=$(jq -cn '{beats: [{duration: 15, description: "[Shot 1]\nA slow bank of fog crosses the bay. The camera tracks along the cliff edge while gulls ride the wind. Spray climbs the rocks below the gallery."}],
  soundscape: "Wind and ocean surf, with waves splashing against the rocks.", turbo: true, seed: 20261301}')
made=$(run_page add shot --tool "$TOOL" --fields "$FIELDS" 2>&1)
J '.name == "shot"' <<< "$made" || fail "drop shot: $made"

before=$(estimate)
say "before any run: $before"
J --arg m "$NAME" '.machine == $m and .basis == "reference" and .run_ms > 0' <<< "$before" || fail "the estimate before any run isn't our measurement on $NAME: $before"

declare -A TOOK=()
for n in 1 2; do
  say "render $n on $NAME"
  run_page render --on "$NAME" shot > "$OUT/render-$n.txt" 2>&1 || fail "render $n: $(tail -c 500 "$OUT/render-$n.txt")"
  run=$(newest_run)
  cozy run watch "$run" --once --json > "$OUT/run-$n.json" 2>&1
  [ "$(jq -r .status "$OUT/run-$n.json")" = completed ] || fail "run $run: $(jq -r '.reason.message // .status' "$OUT/run-$n.json" | cut -c1-300)"
  TOOK[$n]=$(jq '.execution_ms // 0' "$OUT/run-$n.json")
  say "run $run ($n): execution $((TOOK[$n] / 1000)) s"
done

after=$(estimate)
say "after two runs: $after"
J '.basis == "measured"' <<< "$after" || fail "the estimate after its runs isn't measured: $after"
est=$(jq '.run_ms' <<< "$before") warm=${TOOK[2]}
off=$(awk -v e="$est" -v w="$warm" 'BEGIN { printf "%.1f", (e - w) / w * 100 }')
say "estimate before any run $((est / 1000)) s, warm run $((warm / 1000)) s ($off%), cold run $((TOOK[1] / 1000)) s; measured estimate $(($(jq '.run_ms' <<< "$after") / 1000)) s"
awk -v e="$est" -v w="$warm" 'BEGIN { exit !(e > 0.65 * w && e < 1.35 * w) }' || fail "the estimate is $off% off the warm run"
say "PASS: the estimate before any run is within 35% of the warm run"
