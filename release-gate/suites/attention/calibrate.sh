# xs-171 F's end-to-end half (README.md, Calibration): sourced after the attention suite's cells, as
#   release-gate.yml -f suites=attention -f cells=release-gate/suites/attention/calibrate.sh -f packages_ref=<open rc> ...
# On this class's reference card, each model that declares 8-bit attention renders the bench arms' requests (suites/bench/
# cells.tsv, `a-*`: two prompts and seeds each) at 16 bits (`--attention-kernel exact`, the reference), on the table's own
# kernels (today's choice), and pinned to each 8-bit kernel this class runs (the quality cells' level-8 kernels). media.py
# pair measures each against its 16-bit reference: layout SSIM for a picture, each eighth's mean difference for a video.
# signoff/ holds each output (a picture as PNG, a video as H.264 at CRF 18) and signoff.jsonl, one row per output, which
# signoff.py merges with the other classes' and the quality cells' errors into Paul's sign-off (manifest.json, bars).
# H3 runs on the four classes the design names, the pictures on the two GeForce ones (measurement-design §3.4).
cal_class=$(run_get "$OUT" attention-inventory .result.detail | jq -r '.klass // empty' 2>/dev/null)
case $cal_class in
  mma.sync/half | mma.sync+fp4/half) cal_cells="a-h3-15s a-h3-15s-b a-qwen4mp a-qwen4mp-b a-moss a-moss-b a-klein a-klein-b" ;;
  wgmma | tcgen05) cal_cells="a-h3-15s a-h3-15s-b" ;;
  *) cal_cells= ;;
esac
# This class's 8-bit kernels, from its quality cells: each is an arm of its own.
cal_kernels=$(for f in attention-quality-*.json; do run_get "$OUT" "${f%.json}" .result.detail | jq -r 'select(.level == 8) | .kernel' 2>/dev/null; done | sort -u | paste -sd' ')
log "calibration: class ${cal_class:-unknown}; arms exact, table${cal_kernels:+, $cal_kernels}; ${cal_cells:-no models on this class}"
mkdir -p signoff; : > signoff/signoff.jsonl
for cal in $cal_cells; do
  IFS=$'\t' read -r _ _ _ _ cal_seed cal_target cal_args _ < <(awk -F'\t' -v n="$cal" '$1 == n' "$HERE/suites/bench/cells.tsv")
  [ -n "${cal_target:-}" ] || { expect "calibrate-$cal" FAIL "no $cal row in suites/bench/cells.tsv"; continue; }
  cal_model=${cal_target%/*}
  cal_target=$(pkg "${cal_target%/*}" "${cal_target##*/}")  # a --packages checkout's, as bench's cells
  cal_ref=
  for cal_arm in exact table $cal_kernels; do
    cal_pin=(); [ "$cal_arm" = table ] || cal_pin=(--attention-kernel "$cal_arm")
    eval "cell \"calibrate-\$cal-\$cal_arm\" \"\$cal_target\" $cal_args seed:=$cal_seed \"\${cal_pin[@]}\""
    cal_out=$(output "calibrate-$cal-$cal_arm" 'png|webp|jpg|mp4')
    cal_file= cal_pair=null
    if [ -n "$cal_out" ]; then
      case $cal_out in
        *.mp4) cal_file=signoff/$cal-$cal_arm.mp4
               nice -n 19 ffmpeg -loglevel error -y -i "$cal_out" -c:v libx264 -crf 18 -preset medium -pix_fmt yuv420p -c:a copy "$cal_file" < /dev/null || cal_file= ;;
        *) cal_file=signoff/$cal-$cal_arm.png; nice -n 19 ffmpeg -loglevel error -y -i "$cal_out" "$cal_file" < /dev/null || cal_file= ;;
      esac
      [ "$cal_arm" != exact ] || cal_ref=$cal_out
      [ "$cal_arm" = exact ] || [ -z "$cal_ref" ] ||
        cal_pair=$(nice -n 19 uv run -q --no-project --with numpy python -I "$HERE/suites/bench/media.py" pair "$cal_ref" "$cal_out" 2>/dev/null) || cal_pair=null
    fi
    jq -cn --arg class "$cal_class" --arg card "$SKU" --arg cell "$cal" --arg model "$cal_model" --arg arm "$cal_arm" \
      --arg seed "$cal_seed" --arg run "$(cat "calibrate-$cal-$cal_arm.number" 2>/dev/null)" \
      --arg status "$(run_get "$OUT" "calibrate-$cal-$cal_arm" .status)" --arg reason "$(run_get "$OUT" "calibrate-$cal-$cal_arm" .reason.message)" \
      --arg file "$cal_file" --argjson pair "${cal_pair:-null}" \
      '{class: $class, card: $card, cell: $cell, model: $model, arm: $arm, seed: ($seed | tonumber), run: $run, status: $status,
        reason: $reason, file: $file, pair: $pair}' >> signoff/signoff.jsonl
  done
  expect "calibrate-$cal" PASS "$(jq -r --arg c "$cal" 'select(.cell == $c) | "\(.arm) run \(.run) \(.status) \(.pair.ssim // ([.pair.eighths[]?] | max) // "-")"' signoff/signoff.jsonl | paste -sd';')"
done
