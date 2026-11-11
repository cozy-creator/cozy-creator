# xs-171 F's end-to-end half (README.md, Calibration): sourced after the attention suite's cells, as
#   release-gate.yml -f suites=attention -f cells=release-gate/suites/attention/calibrate.sh ...
# On this class's reference card, each model that declares 8-bit attention renders the bench arms' requests (suites/bench/
# cells.tsv, `a-*`: two prompts and seeds each) on the table's kernels and at `--attention-kernel exact`, and media.py pair
# measures each pair: layout SSIM for a picture, each eighth's mean difference for a video. H3 on the four classes the
# design names, the pictures on the two GeForce ones (measurement-design §3.4). calibration.jsonl and calibration/ (both
# outputs of each pair) are what Paul's bars are set from; nothing here passes or fails on a distance.
cal_class=$(run_get "$OUT" attention-inventory .result.detail | jq -r '.klass // empty' 2>/dev/null)
case $cal_class in
  mma.sync/half | mma.sync+fp4/half) cal_cells="a-h3-15s a-h3-15s-b a-qwen4mp a-qwen4mp-b a-moss a-moss-b a-klein a-klein-b" ;;
  wgmma | tcgen05) cal_cells="a-h3-15s a-h3-15s-b" ;;
  *) cal_cells= ;;
esac
log "calibration: class ${cal_class:-unknown}, ${cal_cells:-no arms on this class}"
mkdir -p calibration
for cal in $cal_cells; do
  for cal_arm in "$cal" "$cal-exact"; do
    IFS=$'\t' read -r _ _ _ _ cal_seed cal_target cal_args _ < <(awk -F'\t' -v n="$cal_arm" '$1 == n' "$HERE/suites/bench/cells.tsv")
    [ -n "${cal_target:-}" ] || { expect "calibrate-$cal_arm" FAIL "no $cal_arm row in suites/bench/cells.tsv"; continue 2; }
    cal_target=$(pkg "${cal_target%/*}" "${cal_target##*/}")  # a --packages checkout's, as bench's cells
    eval "cell \"calibrate-\$cal_arm\" \"\$cal_target\" $cal_args seed:=$cal_seed"
  done
  cal_table=$(output "calibrate-$cal" 'png|webp|jpg|mp4') cal_exact=$(output "calibrate-$cal-exact" 'png|webp|jpg|mp4')
  if [ -z "$cal_table" ] || [ -z "$cal_exact" ]; then
    expect "calibrate-$cal" FAIL "no output pair: table '${cal_table:-none}', exact '${cal_exact:-none}'"
    continue
  fi
  cp "$cal_table" "calibration/$cal-table.${cal_table##*.}"; cp "$cal_exact" "calibration/$cal-exact.${cal_exact##*.}"
  cal_pair=$(nice -n 19 uv run -q --no-project --with numpy python -I "$HERE/suites/bench/media.py" pair "$cal_exact" "$cal_table" 2>&1)
  jq -cn --arg class "$cal_class" --arg card "$SKU" --arg cell "$cal" --argjson pair "${cal_pair:-null}" \
    --arg table_run "$(cat "calibrate-$cal.number")" --arg exact_run "$(cat "calibrate-$cal-exact.number")" \
    '{class: $class, card: $card, cell: $cell, table_run: $table_run, exact_run: $exact_run, pair: $pair}' >> calibration.jsonl 2>/dev/null ||
    log "calibrate-$cal: media.py said $cal_pair"
  expect "calibrate-$cal" PASS "table run $(cat "calibrate-$cal.number") against exact run $(cat "calibrate-$cal-exact.number"): $cal_pair"
done
[ ! -s calibration.jsonl ] || {
  echo "| class | arm pair | table / exact run | layout SSIM (picture) or worst eighth (video) |"; echo "|---|---|---|---|"
  jq -r '"| \(.class) | \(.cell) | \(.table_run) / \(.exact_run) | \(.pair.ssim // ([.pair.eighths[]?] | max) // "?") |"' calibration.jsonl
} > calibration.md
