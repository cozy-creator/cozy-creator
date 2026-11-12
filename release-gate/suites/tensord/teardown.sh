# A killed executor still releasing its memory in the driver is waited on, not called stuck (cl-935: bercouli's pid 1161 was,
# 100 ms into its exit): a run holding three quarters of each GPU and 16 GiB of pinned host memory is canceled; nothing is
# called stuck, TensorD logs how long the exit took, the GPUs' memory comes back, and a run after it completes. Two GPUs, SSH.
TD=$OUT/teardown; mkdir -p "$TD"
smi() { "$CLI" machine ssh "$M" "nvidia-smi --query-gpu=index,memory.used,memory.total --format=csv,noheader,nounits" 2>/dev/null; }
used() { smi | awk -F', ' '{s += $2} END {print s + 0}'; }
base=$(used)
gib=$(( $(smi | awk -F', ' '{print $3}' | sort -n | head -1) * 3 / 4 / 1024 ))
run_submit "$TD" hold run "$HERE/suites/tensord/teardown.py" gib:="$gib" host_gib:=16 seconds:=900 --gpus=2 --on "$M" > /dev/null
held=0
for _ in $(seq 150); do
  held=$(used)
  [ "$held" -ge $(( base + 2 * gib * 1024 )) ] && break
  sleep 2
done
smi > "$TD/held-smi.txt"
at=$(date -u +%Y-%m-%dT%H:%M:%S)
"$CLI" run cancel "$(cat "$TD/hold.number")" -y > "$TD/cancel.log" 2>&1
run_wait "$TD" hold
s=$(run_get "$TD" hold .status)
"$CLI" machine logs "$M" tensord -n 0 2>/dev/null | awk -v at="$at" 'substr($1, 1, 19) >= at' > "$TD/tensord.log"
stuck=$(grep -c 'stuck in the kernel' "$TD/tensord.log")
took=$(grep -oE 'pid [0-9]+ exited [0-9.]+ s after its kill' "$TD/tensord.log" | paste -sd ';' -)
listed=$("$CLI" --json machine show "$M" 2>/dev/null | jq '.status.stuck // [] | length')
after=$(used); smi > "$TD/after-smi.txt"
n=$(submit "sdxl/generate@$NEW" prompt="$SCENE, after the teardown" seed:=$((R + 70))); until_status "$n" terminal
expect tensord-teardown-waited "$([ "$s" = canceled ] && [ "$held" -ge $(( base + 2 * gib * 1024 )) ] && [ "$stuck" = 0 ] && [ "$listed" = 0 ] \
  && [ "$after" -le $(( base + 1024 )) ] && [ "$(get "$n" status)" = completed ] && echo PASS || echo FAIL)" \
  "run $(cat "$TD/hold.number") held ${held} MiB (${gib} GiB a GPU over ${base}) and 16 GiB pinned, $s; exit: ${took:-under 1 s}; stuck lines $stuck, \
listed $listed; after ${after} MiB; run $n $(get "$n" status)"
