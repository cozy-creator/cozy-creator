# e2e/cli/lifetime.sh: how a run ends now that nothing judges its progress (cl-663, design stalls.md), the part
# cli/run.sh calls `lifetime`, with the local machine up. A run silent in a native wait far past the old 30 s floor
# completes, and `run show` says how long it has made no progress; a cancel ends a silent run at once (its executor is
# killed); an executor that dies while a child it forked holds its channel fails its run at once; an update waits for
# the runs in flight, says which, holds new runs behind it, and goes on once a halted run is canceled.
here=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
J() { jq -e "$@" > /dev/null; }

# launch <name> <entrypoint> [inputs...]: the run in the background, followed into $OUT/<name>.ndjson; its follower's
# pid in $begun, its number in $n once its executor names its stage (silent's "waiting silently", orphan's pids).
launch() {
  local name=$1 entry=$2
  shift 2
  (cd "$here" && cozy --json run "./weightless/$entry" "$@" --on local > "$OUT/$name.ndjson" 2>&1) &
  begun=$! n=''
  for _ in $(seq 180); do grep -qE '"stage": *"(waiting silently|executor [0-9]+ forked [0-9]+)"' "$OUT/$name.ndjson" && break; sleep 1; done
  grep -qE '"stage": *"(waiting silently|executor [0-9]+ forked [0-9]+)"' "$OUT/$name.ndjson" || { check "$name: its executor never began"; return 1; }
  n=$(grep -oE '"number": *[0-9]+' "$OUT/$name.ndjson" | head -1 | grep -oE '[0-9]+$')
}
# settled <name> <seconds>: run $n's record once it ended, polled at most that long (a hang fails the step, never the
# suite), into $OUT/<name>.show.json; its follower is stopped.
settled() {
  for _ in $(seq "$2"); do
    cozy --json run show "$n" --once > "$OUT/$1.show.json" 2> /dev/null || true
    J '.status | IN("completed", "failed", "canceled")' "$OUT/$1.show.json" && break
    sleep 1
  done
  kill "$begun" 2> /dev/null || true
  wait "$begun" 2> /dev/null || true
}
# update_waiting <name>: `cozy machine update local --gpu-memory-limit off` (a restart) in the background, its pid in
# $updating, until `machine show` and `run ls` say it waits for run $n.
update_waiting() {
  (cozy --json machine update local --gpu-memory-limit off -y > "$OUT/$1.update.json" 2>&1; echo $? > "$OUT/$1.update.code") &
  updating=$!
  local said="waiting for run $n to end"
  for _ in $(seq 60); do
    cozy --json machine show local > "$OUT/$1.machine.json" 2> /dev/null || true
    J --arg w "$said" '.status.update == $w' "$OUT/$1.machine.json" && break
    sleep 1
  done
  J --arg w "$said" '.status.update == $w' "$OUT/$1.machine.json" \
    || check "$1: machine show local never said \"$said\": $(jq -c .status.update "$OUT/$1.machine.json")"
  cozy --json run ls > "$OUT/$1.list.json" || true
  J --arg w "$said" 'any(.runs[]; .kind == "update" and ((.waiting // "") | contains($w)))' "$OUT/$1.list.json" \
    || check "$1: run ls shows no update $said: $(jq -c '[.runs[] | select(.kind == "update") | .waiting]' "$OUT/$1.list.json")"
}
# update_done <name>: the update's end (at most 5 min), and the machine ready after its restart.
update_done() {
  for _ in $(seq 300); do kill -0 "$updating" 2> /dev/null || break; sleep 1; done
  kill "$updating" 2> /dev/null || true
  wait "$updating" 2> /dev/null || true
  [ "$(cat "$OUT/$1.update.code" 2> /dev/null)" = 0 ] || check "$1: the update did not succeed: $(tail -c 300 "$OUT/$1.update.json")"
  machine_ready
}

lifetime() {
step "a silent run is never judged, and shows how long it has not moved"
if launch silent silent seconds:=150; then
  sleep 75
  cozy run show "$n" --once > "$OUT/silent.still.txt" 2>&1 || true
  grep -q 'no progress for 1m' "$OUT/silent.still.txt" || check "silent: run show $n does not say how long it has made no progress: $(grep -i progress "$OUT/silent.still.txt")"
  settled silent 240
  J '.status == "completed"' "$OUT/silent.show.json" || check "silent: run $n is $(jq -c '[.status, .reason]' "$OUT/silent.show.json"), not completed"
  echo "silent: run $n completed after 150 s without a word; at 75 s: $(grep -o 'no progress for [0-9a-z]*' "$OUT/silent.still.txt")"
fi

step "a cancel ends a silent run at once"
if launch cancel-silent silent seconds:=600; then
  t0=$SECONDS
  cozy --json run cancel "$n" > "$OUT/cancel-silent.json" || check "run cancel $n failed"
  settled cancel-silent 120
  took=$((SECONDS - t0))
  J '.status == "canceled"' "$OUT/cancel-silent.show.json" || check "cancel-silent: run $n is $(jq -r .status "$OUT/cancel-silent.show.json"), not canceled"
  # The old rule waited at least its 30 s floor for the call to stop by itself.
  [ "$took" -lt 30 ] || check "cancel-silent: run $n took ${took} s to end after its cancel"
  echo "cancel-silent: run $n canceled ${took} s after its cancel"
  (cd "$here" && cell after-cancel ./weightless/reverse "text=after$RANDOM") || check "no run went after a canceled run's executor was killed"
fi

step "an executor that dies under a forked child fails its run at once"
if [ "$RUNNER_OS" != Windows ] && launch orphan orphan seconds:=600; then
  read -r executor child < <(grep -oE '"stage": *"executor [0-9]+ forked [0-9]+"' "$OUT/orphan.ndjson" | head -1 | grep -oE '[0-9]+' | paste -sd' ')
  t0=$SECONDS
  kill -9 "$executor" || check "orphan: kill -9 $executor failed"
  settled orphan 120
  took=$((SECONDS - t0))
  J '.status == "failed"' "$OUT/orphan.show.json" || check "orphan: run $n is $(jq -r .status "$OUT/orphan.show.json"), not failed"
  grep -q 'ended on its own' "$OUT/orphan.show.json" || check "orphan: run $n's reason names not its executor's end: $(jq -c .reason "$OUT/orphan.show.json")"
  [ "$took" -lt 30 ] || check "orphan: run $n took ${took} s to fail after its executor died"
  for _ in $(seq 10); do kill -0 "$child" 2> /dev/null || break; sleep 1; done
  ! kill -0 "$child" 2> /dev/null || check "orphan: the child $child its dead executor forked still runs"
  echo "orphan: run $n failed ${took} s after its executor $executor died ($(jq -r .reason.message "$OUT/orphan.show.json")); its child $child is gone"
fi

step "an update waits for a running run, which completes, and a run submitted meanwhile queues behind it"
if launch going silent seconds:=45; then
  update_waiting going
  (cd "$here" && cell behind ./weightless/reverse "text=behind$RANDOM") &
  behind=$!
  settled going 180
  J '.status == "completed"' "$OUT/going.show.json" || check "going: run $n is $(jq -c '[.status, .reason]' "$OUT/going.show.json"), not completed: the update must not end it"
  update_done going
  wait "$behind" || check "behind: the run submitted while the update waited did not complete"
  echo "going: run $n completed, then the update activated; run $(cat "$OUT/behind.number") queued behind it and completed"
fi

step "an update behind a halted run waits, visibly, until the run is canceled"
if launch halted silent seconds:=600; then
  update_waiting halted
  sleep 5
  kill -0 "$updating" 2> /dev/null || check "halted: the update did not wait for run $n"
  cozy --json run cancel "$n" > "$OUT/halted.cancel.json" || check "run cancel $n failed"
  settled halted 60
  J '.status == "canceled"' "$OUT/halted.show.json" || check "halted: run $n is $(jq -r .status "$OUT/halted.show.json"), not canceled"
  update_done halted
  echo "halted: the update said $(jq -c .status.update "$OUT/halted.machine.json") until run $n was canceled, then activated"
fi
}
