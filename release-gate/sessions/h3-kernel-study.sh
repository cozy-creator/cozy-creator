# h3-kernel-study: per layer and per head, on real H3 activations, does a cheap Q/K/V outlier statistic predict each
# production attention kernel's error? Each request runs once from packages `probe` (production minimax-h3 plus a
# proof-only probe) with the transformer pinned to kitchen-sol-fp4-fixed-means, so every call (dense and sparse steps)
# passes Sol's policy boundary; at four steps per request the probe measures every call against BF16 SDPA and returns
# the rows, and each site's Q/K RMSNorm gammas (a weight-only rule's input), as the run's `study` JSON. Proof-only.
# Every verdict is the run record's.
# params:
#   requests   [{function, prompt, seed, duration_s, est_s, optional}]: optional ones run only while the cap leaves
#              their est_s (scaled by how the first request compared with its own) plus 5 min
#   cohort     {"runtime"}: what the pod must run (0.34.2)
#   pin        the transformer's attention kernel (transformer=kitchen-sol-fp4-fixed-means)
#   traffic_gb (90), need_s (the seconds the required requests need)
. "$(dirname "$0")/lib.sh"
PKG=$PACKAGES/probe/minimax-h3 RT=$(jq -r '.runtime // empty' <<<"$(param cohort '{}')")
PIN=$(param pin transformer=kitchen-sol-fp4-fixed-means)
grep -q 'KERNEL-ERROR STUDY' "$PKG/minimax_h3/_kernel_study.py" && grep -q '_kernel_study.drain' "$PKG/h3.py" || fail "packages probe lacks the study"
say "package probe: $(git -C "$PKG" rev-parse --short HEAD), minimax-h3 $(grep -m1 '^version' "$PKG/pyproject.toml")"
param requests '[]' > "$OUT/requests.json"
jq -e 'length >= 3 and all(.[]; (.function | test("^fl2va(_turbo)?$")) and (.prompt | length > 0) and (.seed | type == "number")
  and (.est_s | type == "number"))' "$OUT/requests.json" >/dev/null || fail "requests: 3+ {function, prompt, seed, est_s}"
[[ $RT =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]] || fail "cohort.runtime"
printf 'run 5 · sync local/x (source changed)\nrun 6 · local/x/f\nrun      6\n' > "$OUT/probe-cell.txt"
[ "$(runno probe-cell)" = 6 ] || fail "parsers"
N_REQ=$(jq length "$OUT/requests.json")
for i in $(seq 0 $((N_REQ - 1))); do jq ".[$i] | {prompt, seed, duration_s}" "$OUT/requests.json" > "$OUT/r$i.request.json"; done
quote "$(param traffic_gb 90)" "$(param need_s 3200)"
T_ORDER=$(date +%s)
rent
cozy machine show "$RID" --json > "$OUT/show-cohort.json" 2>&1
jq -e --arg r "$RT" '.status.runtime == $r' "$OUT/show-cohort.json" >/dev/null || fail "$NAME runs Runtime $(jq -r .status.runtime "$OUT/show-cohort.json"), not $RT"
say "ASSERT $NAME runs $(jq -c '{v: .status.version, rt: .status.runtime, tfs: .status.tensorfs, image: .rental.base_worker_image_digest}' "$OUT/show-cohort.json")"

SCALE=1 DONE=0
for i in $(seq 0 $((N_REQ - 1))); do
  fn=$(jq -r ".[$i].function" "$OUT/requests.json") est=$(jq -r ".[$i].est_s" "$OUT/requests.json")
  left=$((CAP_S - $(date +%s) + T_ORDER - 300)) need=$(awk -v e="$est" -v s="$SCALE" 'BEGIN{printf "%d", e*s}')
  if jq -e ".[$i].optional == true" "$OUT/requests.json" >/dev/null && [ "$need" -gt "$left" ]; then
    say "SKIP r$i ($fn): needs ~${need}s, the cap leaves ${left}s"; continue
  fi
  t0=$(date +%s)
  step "r$i" run "$PKG/$fn" --input "$OUT/r$i.request.json" --lane int8 --gpus=1 --attention-kernel "$PIN" --on "$NAME" --out "$OUT/media/r$i"; c=$?
  N=$(runno "r$i"); [ -n "$N" ] || N=$(grep -oE 'cozy run show [0-9]+' "$OUT/r$i.txt" | tail -1 | awk '{print $4}')
  [ -n "$N" ] || fail "r$i named no run (exit $c)"
  cozy run show "$N" --once --json > "$OUT/r$i.json" 2>/dev/null; cozy run show "$N" --once > "$OUT/r$i.show.txt" 2>&1
  jq -e '.status == "completed"' "$OUT/r$i.json" >/dev/null || fail "run $N: $(jq -r '.reason.message // .status' "$OUT/r$i.json" | head -c 600)"
  [ "$i" != 0 ] || SCALE=$(awk -v a="$(( $(date +%s) - t0 ))" -v e="$est" 'BEGIN{s = a / e; printf "%.2f", s < 1 ? 1 : s}')
  study=$(ls "$OUT/media/r$i"/*study*.json 2>/dev/null | head -1)
  [ -n "$study" ] || fail "run $N returned no study output: $(ls "$OUT/media/r$i" | tr '\n' ' ')"
  jq --argjson n "$N" --arg f "$fn" '([.calls[] | select(.function | test("^generate_")) | .measurements] | last) as $m | {
      run: $n, function: $f, runtime: $m.software.runtime,
      route: [$m.observations[]? | select(.name == "attention.selected" and (.fields.component | test("transformer"))) | "\(.value)[\(.fields.dense_reference)]"],
      steps_s: [$m.attribution.steps.denoise.series[]?[1] / 1000 | . * 10 | round / 10],
      study: ([.result.warnings[]? | select(startswith("kernel-study ")) | .[13:] | fromjson] | last)}' "$OUT/r$i.json" > "$OUT/r$i.facts.json"
  say "run $N ($fn, $(( $(date +%s) - t0 ))s): $(jq -c . "$OUT/r$i.facts.json")"
  jq -e --arg r "$RT" '.runtime == $r' "$OUT/r$i.facts.json" >/dev/null || fail "run $N ran Runtime $(jq -r .runtime "$OUT/r$i.facts.json")"
  jq -e --arg p "${PIN#*=}" 'any(.route[]; startswith($p + "["))' "$OUT/r$i.facts.json" >/dev/null || fail "run $N did not serve $PIN"
  jq -e '.calls > 0 and .failed == 0 and ([.calls[].step] | unique | length) == 4
    and ([.calls[].path] | unique | length) >= 50' "$study" >/dev/null ||
    fail "run $N study: $(jq -c '{calls: (.calls | length), failed, notes, steps: ([.calls[].step] | unique), paths: ([.calls[].path] | unique | length)}' "$study")"
  say "  study $(basename "$study"): $(jq -c '{calls: (.calls | length), seconds: (.seconds | round), notes, steps: ([.calls[].step] | unique), paths: ([.calls[].path] | unique | length),
    gammas: ([.weights[] | select(.norm_k != null)] | length)}' "$study")"
  DONE=$((DONE + 1))
done
[ "$DONE" -ge 3 ] || fail "only $DONE requests measured"
say "PASS: $DONE requests measured per layer and per head"
