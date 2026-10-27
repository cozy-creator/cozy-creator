# Four independent Qwen-Image-2 requests at once on a 4-GPU machine, each on one GPU (--gpus=1): every GPU serves
# its own replica and none waits on another. Sourced by gate.sh --cells on a 4-GPU rental; `only` names qwen-r1..r4.
# `qwen-replicas` holds them to four completed runs on four GPUs, by four executors, all executing at once.
QR=$(pkg qwen-image-2 generate) QN=()
QM=("blown glass" "amber resin" "clear ice" "smoked quartz")
for i in 1 2 3 4; do
  n=qwen-r$i
  [ -z "$ONLY" ] || [[ $ONLY == *",$n,"* ]] || continue
  p="This is an RGBA image with transparency. $OBJECT made of ${QM[i - 1]}, semi-transparent with soft reflections. The image has alpha channel and the background is transparent."
  printf '%s\t%s\n' "$n" "$p" >> prompts.tsv
  run_submit "$OUT" "$n" run "$QR" --on "$M" seed:=$((R + 340 + i)) prompt="$p" --gpus=1 --out "$OUT/out/$n" > /dev/null
  QN+=("$n")
done
for n in "${QN[@]}"; do
  run_wait "$OUT" "$n"
  printf '%s\t%s\t%s\n' "$n" "$QR" "$(cat "$n.number")" >> cells.tsv
  log "$n run $(cat "$n.number") $(run_get "$OUT" "$n" .status)"
done
if [ ${#QN[@]} -gt 1 ]; then
  v=$(python3 -I "$HERE/suites/execution/multi-gpu/concurrent.py" "${QN[@]/%/.json}")
  expect qwen-replicas "${v%% *}" "${v#* }"
fi
