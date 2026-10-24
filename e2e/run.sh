# e2e/run.sh: how every suite runs `cozy` and reads what a run made (e2e/lib.sh, release-gate/gate.sh and its suites).
# RUN_CLI is the cozy to call (default `cozy`). A run is kept in <dir> under its <name>: <name>.ndjson (its events),
# <name>.err, <name>.number (`none`: it never got one) and <name>.json, its record (`cozy run show --once --json`).
# `run_cell` lands its outputs under <dir>/out/<name> (`--out`); `run_output` finds them by the record's output[].path.
#
# The watchdog: a run whose events, record (status, machine state, why it waits, progress, outputs) and landed bytes
# stay the same for RUN_STALL seconds is canceled, and one whose cancel does not take in as long again is wedged.
# Nothing ends a run for taking long while it moves. RUN_STALL is a rental's idle window: a rental with no work that
# long releases itself. The watchdog's own reads give up after 120 s, so a stuck Calcifer cannot hang it.
RUN_CLI=${RUN_CLI:-cozy} RUN_STALL=900 RUN_POLL=5

# run_submit <dir> <name> <cozy args...>: starts `cozy --json <args>` (a verb that makes a numbered run) in the
# background and prints the run's number once it has one; nothing when the command ended without one.
run_submit() {
  local dir=$1 name=$2 n=''; shift 2
  rm -f "$dir/$name".{ndjson,err,json,number,pid,stalled}
  "$RUN_CLI" --json "$@" > "$dir/$name.ndjson" 2> "$dir/$name.err" &
  local pid=$! seen='' size since=$SECONDS
  echo "$pid" > "$dir/$name.pid"
  until [ -n "$n" ]; do
    n=$(run_number "$dir/$name.ndjson")
    [ -n "$n" ] || kill -0 "$pid" 2> /dev/null || { n=$(run_number "$dir/$name.ndjson"); break; }
    # Before it has a number, what it prints is all there is to watch.
    size=$(cat "$dir/$name.ndjson" "$dir/$name.err" 2> /dev/null | wc -c || true)
    [ "$size" = "$seen" ] || { seen=$size since=$SECONDS; }
    if [ -z "$n" ] && [ $((SECONDS - since)) -ge "$RUN_STALL" ]; then
      echo "cozy $*: no run and no output in ${RUN_STALL}s: stopped it" >> "$dir/$name.stalled"; kill "$pid" 2> /dev/null || true; break
    fi
    [ -n "$n" ] || sleep 1
  done
  echo "${n:-none}" > "$dir/$name.number"
  [ -z "$n" ] || echo "$n"
}
run_number() { { jq -r 'select(.run.number != null) | .run.number' "$1" 2> /dev/null || true; } | head -1; }

# run_wait <dir> <name> [status]: run <name> followed to its end (or, given a status, until it reaches it), watched as
# above, its record in <name>.json. 0: ended (or reached the status); 1: never started, or ended without reaching the
# status; 2: wedged. Why the watchdog stepped in goes to <name>.stalled.
run_wait() {
  local dir=$1 name=$2 want=${3:-} n pid st sig last='' since=$SECONDS canceled='' ended alive landed i
  n=$(cat "$dir/$name.number" 2> /dev/null || true) pid=$(cat "$dir/$name.pid" 2> /dev/null || true)
  if [ -z "$n" ] || [ "$n" = none ]; then return 1; fi
  ended=0; [ -z "$want" ] || ended=1
  while :; do
    # Whether its follower still runs, asked before the record is read: one that ended had read the record whole.
    alive=; ! kill -0 "$pid" 2> /dev/null || alive=1
    run_show "$dir" "$name" "$n"
    st=$(run_get "$dir" "$name" .status)
    if [ -n "$want" ] && [ "$st" = "$want" ]; then return 0; fi
    # Done once it ended, every completed output landed (or will not, or is a checkpoint) and its follower ended.
    landed=; ! jq -e 'all(.output[]? | select(.status == "completed"); .path or .export_error or .checkpoint)' \
      "$dir/$name.json" > /dev/null 2>&1 || landed=1
    case $st in completed | failed | canceled) [ -n "$alive" ] || [ -z "$landed" ] || return "$ended" ;; esac
    sig="$(wc -c < "$dir/$name.ndjson" 2> /dev/null || true) $(wc -c "$dir/out/$name"/* 2> /dev/null | tail -1 || true)
      $(jq -c '[.status, .machine_state, .waiting, .progress, .cancel_requested,
      [.output[]? | [.status, .length, .path, .export_error]]]' "$dir/$name.json" 2> /dev/null || true)"
    [ "$sig" = "$last" ] || { last=$sig since=$SECONDS; }
    if [ $((SECONDS - since)) -ge "$RUN_STALL" ]; then
      case $st in
        completed | failed | canceled)
          echo "run $n ended; nothing more of it landed in ${RUN_STALL}s: stopped its follower" >> "$dir/$name.stalled"
          kill "$pid" 2> /dev/null || true
          return "$ended" ;;
      esac
      if [ -n "$canceled" ]; then
        echo "run $n: its cancel did not take in ${RUN_STALL}s" >> "$dir/$name.stalled"; kill "$pid" 2> /dev/null || true; return 2
      fi
      echo "run $n made no progress in ${RUN_STALL}s ($(jq -c '{status, machine_state, waiting, progress}' "$dir/$name.json" 2> /dev/null || true)): canceled" >> "$dir/$name.stalled"
      run_bounded "$RUN_CLI" run cancel "$n" -y >> "$dir/$name.stalled" 2>&1 || true
      canceled=1 since=$SECONDS
    fi
    # Look again in RUN_POLL seconds, or at once when the follower ends (a step's wall time is to its end).
    if kill -0 "$pid" 2> /dev/null; then
      for ((i = 0; i < RUN_POLL * 5; i++)); do sleep 0.2; kill -0 "$pid" 2> /dev/null || break; done
    else
      sleep "$RUN_POLL"
    fi
  done
}

# run_show <dir> <name> <number>: the run's record now, into <name>.json (kept as it was when the read fails). `run
# show` exits non-zero for a run that failed or was canceled: the record is what it printed.
run_show() {
  run_bounded "$RUN_CLI" --json run show "$3" --once > "$1/$2.json.new" 2> /dev/null || true
  if jq -e .status "$1/$2.json.new" > /dev/null 2>&1; then
    mv "$1/$2.json.new" "$1/$2.json"
  else
    rm -f "$1/$2.json.new"
  fi
}

# run_bounded <command...>: the command, stopped when it has not answered in 120 s; its exit status.
run_bounded() {
  local p i
  "$@" &
  p=$!
  for ((i = 0; i < 600; i++)); do kill -0 "$p" 2> /dev/null || break; sleep 0.2; done
  kill "$p" 2> /dev/null || true
  wait "$p"
}

# run_cell <dir> <name> <target> [inputs and flags, --on among them]: one `cozy run`, its outputs under <dir>/out/<name>,
# followed to its end. 0: completed; 1: it ended otherwise or never started; 2: wedged.
run_cell() {
  local dir=$1 name=$2 code=0; shift 2
  [ -n "$(run_submit "$dir" "$name" run "$@" --out "$dir/out/$name")" ] || return 1
  run_wait "$dir" "$name" || code=$?
  [ "$code" != 0 ] || [ "$(run_get "$dir" "$name" .status)" = completed ] || code=1
  return "$code"
}

# run_get <dir> <name> <jq filter>: from its record, empty when absent.
run_get() { jq -r "($3) // empty" "$1/$2.json" 2> /dev/null || true; }

# run_output <dir> <name> <extensions, as a|b>: its first landed output with one of these extensions.
run_output() { run_get "$1" "$2" '.output[]?.path' | grep -E "\.($3)$" | head -1 || true; }
