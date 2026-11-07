# Sourced by cells.sh: core/faults on this rental over `cozy machine ssh`, never through `cozy run`, because these faults
# are deliberate. One expect row per cell. `--on` (a machine the gate did not rent) runs none.
faults_run() {
  local F=/root/varena-faults D=$OUT/faults seen= size moved=$SECONDS ended= row
  fpod() { timeout 120 "$CLI" machine ssh "$M" -- "$@"; }
  mkdir -p "$D"
  # run.py takes a generation's Python, with this candidate's TensorFS in it: a gate whose --only ran nothing else has none.
  [ -f offload-adapter.json ] || run_cell "$OUT" faults-generation "$V/torch-offload/adapter" --on "$M" label="$NAME $R" --gpus=1
  cap off  # the core and offload cells' executors: run.py needs the card free
  tar -C "$V" -cz core/faults native-tensor-export public-source ledger/table.py -C "$HERE/../crates/varena" include |
    fpod "rm -rf $F && mkdir -p $F && tar -xz -C $F" || { expect faults FAIL "could not copy the suite to $M"; return; }
  fpod "cat > $F/start.sh" <<'POD' || { expect faults FAIL "could not copy start.sh to $M"; return; }
py=$(dirname "$(ls -t /var/lib/cozy/tensord/generations/*/env/pyvenv.cfg | head -1)")/bin/python
native=$("$py" -c 'import pathlib, tensorfs; print(pathlib.Path(tensorfs.__file__).parent / ".native")')
cc -std=c11 -Wall -Wextra -Werror -fPIC -shared -I include native-tensor-export/source_provider.c \
  -L "$native" -Wl,-rpath,"$native" -lvarena -ldl -pthread -o provider.so || exit 2
for _ in $(seq 60); do [ -z "$(nvidia-smi --query-compute-apps=pid --format=csv,noheader)" ] && break; sleep 1; done
FAULT_PROVIDER=$PWD/provider.so "$py" core/faults/run.py --out out
POD
  fpod "cd $F && { nohup bash -c 'bash start.sh; echo \$? > done' > run.log 2>&1 < /dev/null & }" < /dev/null ||
    { expect faults FAIL "could not start on $M"; return; }
  log "faults started on $M"
  # A hang is a failure: the suite's own files must keep growing. `hold` renews the rental only while runs move.
  while :; do
    sleep 60
    ended=$(fpod "cat $F/done 2>/dev/null" < /dev/null)
    [ -z "$ended" ] || break
    size=$(fpod "du -sb $F/out $F/run.log 2>/dev/null | awk '{s += \$1} END {print s}'" < /dev/null)
    [ -z "$size" ] || [ "$size" = "$seen" ] || { seen=$size moved=$SECONDS; }
    if (( SECONDS - moved >= STALL * 60 )); then
      expect faults FAIL "nothing moved on $M for $STALL min"
      break
    fi
    "$CLI" machine keepalive "$M" > /dev/null 2>&1
  done
  timeout 600 "$CLI" machine ssh "$M" -- "tar -C $F -cz run.log out" < /dev/null | tar -xz -C "$D"
  if [ ! -s "$D/out/summary.txt" ]; then
    [ -z "$ended" ] || expect faults FAIL "exit $ended, no summary: $(tail -n 5 "$D/run.log" 2>/dev/null)"
    return
  fi
  while read -r row; do
    expect "fault-$(jq -r .cell <<< "$row")" "$(jq -r 'if .result == "FAIL" or (.problems | length) > 0 then "FAIL" else "PASS" end' <<< "$row")" \
      "$(jq -c '{result, problems, xids, seconds}' <<< "$row")"
  done < "$D/out/summary.txt"
}
[ -n "$ON" ] || { [ -n "$ONLY" ] && [[ $ONLY != *,faults,* ]]; } || faults_run
