# Sourced by a suite's cells.sh. A library's own tests (TensorFS's, varena's) are plain Python programs on this rental over
# `cozy machine ssh`, never `cozy run`: they test the library itself, so the package contract, which keeps packages off a
# library, is not theirs (xs-174). Each uses that library's public API only; what it can't reach that way is a finding in
# the program's README, never a private import. They run in an environment package's environment, programs/env unless
# `--env <name>` names another (programs/<name>, module programs_<name>, e.g. `diffusers`): its TensorFS (varena with it)
# and Torch are the ones the machine installed, what ships.
#   programs <label> <dir> [--env <name>] <program.py>...
# A program prints `ROW {"cell": ..., "result": "PASS"|"FAIL", "detail": ...}` lines; each is an expect row. A program
# that exits non-zero, or prints no row, fails as itself. `--on` (a machine the gate did not rent) runs none, as faults.sh.
programs() {
  local label=$1 dir=$2 P=/root/programs-$1 D=$OUT/programs-$1 program name ended= seen= size moved=$SECONDS row env=env; shift 2
  [ "${1:-}" != --env ] || { env=$2; shift 2; }
  [ -z "$ON" ] || { log "programs $label: not on a machine this gate did not rent"; return 0; }
  [ -z "$ONLY" ] || [[ $ONLY == *",$label,"* ]] || return 0
  ppod() { timeout 120 "$CLI" machine ssh "$M" -- "$@"; }
  mkdir -p "$D"
  ONLY=${ONLY:+$ONLY,programs-$env,} cell "programs-$env" "$HERE/programs/$env/ready"
  [ "$(run_get "$OUT" "programs-$env" .status)" = completed ] || { expect "programs-$label" FAIL "programs/$env did not run"; return; }
  # No executor keeps the card or its memory: a restart ends them, and the environment stays on disk.
  "$CLI" machine update "$M" --gpu-memory-limit off -y > "$D/restart.log" 2>&1
  tar -C "$dir" -cz . | ppod "rm -rf $P && mkdir -p $P/out && tar -xz -C $P" || { expect "programs-$label" FAIL "could not copy $dir to $M"; return; }
  ppod "cat > $P/start.sh" <<POD || { expect "programs-$label" FAIL "could not copy start.sh to $M"; return; }
site=\$(ls -dt /var/lib/cozy/tensord/generations/*/env/lib/python3*/site-packages/programs_$env* 2>/dev/null | head -1)
[ -n "\$site" ] || { echo "no environment holds programs_$env"; exit 2; }
py=\${site%/lib/python3*}/bin/python
for _ in \$(seq 60); do [ -z "\$(nvidia-smi --query-compute-apps=pid --format=csv,noheader 2>/dev/null)" ] && break; sleep 1; done
for program in $*; do
  timeout 1800 "\$py" -s -E "\$program" > "out/\${program%.py}.log" 2>&1; echo \$? > "out/\${program%.py}.exit"
done
POD
  ppod "cd $P && { nohup bash -c 'bash start.sh; echo \$? > done' > run.log 2>&1 < /dev/null & }" < /dev/null ||
    { expect "programs-$label" FAIL "could not start on $M"; return; }
  log "programs $label started on $M: $*"
  # A hang is a failure: the programs' own output must keep growing. `hold` renews the rental only while runs move.
  while :; do
    sleep 30
    ended=$(ppod "cat $P/done 2>/dev/null" < /dev/null)
    [ -z "$ended" ] || break
    size=$(ppod "du -sb $P/out 2>/dev/null | cut -f1" < /dev/null)
    [ -z "$size" ] || [ "$size" = "$seen" ] || { seen=$size moved=$SECONDS; }
    if (( SECONDS - moved >= STALL * 60 )); then expect "programs-$label" FAIL "nothing moved on $M for $STALL min"; break; fi
    "$CLI" machine keepalive "$M" > /dev/null 2>&1
  done
  timeout 600 "$CLI" machine ssh "$M" -- "tar -C $P -cz run.log out" < /dev/null | tar -xz -C "$D"
  [ "$ended" = 0 ] || expect "programs-$label" FAIL "start.sh exit ${ended:-none}: $(tail -n 3 "$D/run.log" 2>/dev/null)"
  for program in "$@"; do
    name=${program%.py}
    if [ "$(cat "$D/out/$name.exit" 2>/dev/null)" != 0 ] || ! grep -q '^ROW ' "$D/out/$name.log" 2>/dev/null; then
      expect "$label-$name" FAIL "exit $(cat "$D/out/$name.exit" 2>/dev/null || echo none): $(tail -n 4 "$D/out/$name.log" 2>/dev/null)"
    fi
    while read -r row; do
      expect "$(jq -r .cell <<< "$row")" "$(jq -r .result <<< "$row")" "$(jq -c .detail <<< "$row")"
    done < <(sed -n 's/^ROW //p' "$D/out/$name.log" 2>/dev/null)
  done
}
