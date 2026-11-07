# A function with no model holds its GPUs in one process (TensorD's `ranks`): TensorD places it on both, it allocates on each,
# both GPUs' ledgers show it holding and then idle, and TensorD's supervisors clear both when it exits. Two GPUs and SSH.
OP=$OUT/one-process; mkdir -p "$OP"
LEDGERS=$(base64 -w0 "$HERE/suites/tensord/ledgers.py")
ledgers() { "$CLI" machine ssh "$M" "echo $LEDGERS | base64 -d | python3 -" 2>/dev/null | tail -n 1; }
# Tables naming pid $1, and the GPUs TensorD last placed a plan on.
tables_of() { jq --argjson p "$1" '[.[] | select(any(.[]; .pid == $p))] | length' <<< "$2"; }
placed() { "$CLI" machine logs "$M" memory -n 0 2>/dev/null | grep '"event":"place"' | tail -n 1 | grep -oE '"gpus":"[^"]*"' | cut -d'"' -f4; }
run_submit "$OP" holder run "$HERE/suites/tensord/holder.py" gib:=1 seconds:=45 --gpus=2 --on "$M" > /dev/null
pid=
for _ in $(seq 60); do
  l=$(ledgers)
  pid=$(jq -r '[.[] | map(select(.held >= 1073741824) | .pid)] | select(length == 2) | (.[0] - (.[0] - .[1])) | .[0] // empty' <<< "$l" 2>/dev/null)
  [ -z "$pid" ] || break
  sleep 2
done
echo "$l" > "$OP/during.json"
during="no one process held both GPUs"
[ -z "$pid" ] || during="ledgers $(tables_of "$pid" "$l"), TensorD on $(placed)"
"$CLI" machine ssh "$M" "nvidia-smi --query-gpu=index,memory.used --format=csv,noheader" > "$OP/during-smi.txt" 2>/dev/null
run_wait "$OP" holder
s=$(run_get "$OP" holder .status) passed=$(run_get "$OP" holder .result.passed) detail=$(run_get "$OP" holder .result.detail)
after= idle= exited="not clear"
if [ -n "$pid" ]; then
  l=$(ledgers); echo "$l" > "$OP/after.json"
  after="$(tables_of "$pid" "$l") ledgers idle"
  idle=$(jq -c --argjson p "$pid" '[.[][] | select(.pid == $p) | {held, idle, running}]' <<< "$l")
  "$CLI" machine ssh "$M" "kill $pid" > /dev/null 2>&1
  for _ in $(seq 15); do
    l=$(ledgers)
    [ "$(tables_of "$pid" "${l:-\{\}}")" = 0 ] && { exited="both clear"; break; }
    sleep 2
  done
  echo "$l" > "$OP/exited.json"
fi
expect tensord-one-process "$([ "$s" = completed ] && [ "$passed" = true ] && [ "$during" = "ledgers 2, TensorD on 0,1" ] && [ "$(jq 'length == 2 and all(.[]; .idle >= 1073741824)' <<< "${idle:-[]}")" = true ] && [ "$exited" = "both clear" ] && echo PASS || echo FAIL)" \
  "run $(cat "$OP/holder.number") $s passed $passed; pid ${pid:-none} during: $during; after: $after $idle; exited: $exited; $detail"
