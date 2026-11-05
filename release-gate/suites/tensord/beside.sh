# A process comes up on cards another left nearly full, idle (run 31653): a holder fills both GPUs, then a second plan starts beside it.
BS=$OUT/beside; mkdir -p "$BS"
machine_gpus() { "$CLI" machine ssh "$M" "nvidia-smi --query-gpu=$1 --format=csv,noheader,nounits" 2>/dev/null | tr -d ' \r'; }
fill=$(machine_gpus memory.free | sort -n | head -n 1 | awk '{ printf "%.2f", ($1 - 700) / 1024 }')
run_submit "$BS" filler run "$HERE/suites/tensord/holder.py" gib:="$fill" seconds:=1 --gpus=2 --on "$M" > /dev/null
run_wait "$BS" filler
used=$(machine_gpus memory.used | paste -sd,)
cp "$HERE/suites/tensord/holder.py" "$BS/beside.py"
run_submit "$BS" beside run "$BS/beside.py" gib:=1 seconds:=1 --gpus=2 --on "$M" > /dev/null
run_wait "$BS" beside
s=$(run_get "$BS" beside .status) passed=$(run_get "$BS" beside .result.passed)
expect tensord-beside "$([ "$s" = completed ] && [ "$passed" = true ] && echo PASS || echo FAIL)" \
  "run $(cat "$BS/beside.number") $s beside filler $(cat "$BS/filler.number") $(run_get "$BS" filler .status), $fill GiB each, $used MiB in use"
