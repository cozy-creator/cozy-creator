# A process comes up on cards another left nearly full, idle (run 31653): a holder fills both GPUs, then a second plan starts beside it.
BS=$OUT/beside; mkdir -p "$BS"
machine_gpus() { "$CLI" machine ssh "$M" "nvidia-smi --query-gpu=$1 --format=csv,noheader,nounits" 2>/dev/null | tr -d ' \r'; }
# Idle executors from earlier cells may have filled them already: then there is nothing to add.
fill=$(machine_gpus memory.free | sort -n | head -n 1 | awk '{ f = ($1 - 700) / 1024; printf "%.2f", f > 0 ? f : 0 }')
filled=completed
if [ "$fill" != 0.00 ]; then
  run_submit "$BS" filler run "$HERE/suites/tensord/holder.py" gib:="$fill" seconds:=1 --gpus=2 --on "$M" > /dev/null
  run_wait "$BS" filler
  filled=$(run_get "$BS" filler .status)
fi
used=$(machine_gpus memory.used | paste -sd,)
cp "$HERE/suites/tensord/holder.py" "$BS/beside.py"
run_submit "$BS" beside run "$BS/beside.py" gib:=1 seconds:=1 --gpus=2 --on "$M" > /dev/null
run_wait "$BS" beside
s=$(run_get "$BS" beside .status) passed=$(run_get "$BS" beside .result.passed)
expect tensord-beside "$([ "$s" = completed ] && [ "$passed" = true ] && [ "$filled" = completed ] && echo PASS || echo FAIL)" \
  "run $(cat "$BS/beside.number") $s beside a filler $filled ($fill GiB each, run $(cat "$BS/filler.number" 2>/dev/null || echo none)), $used MiB in use"
