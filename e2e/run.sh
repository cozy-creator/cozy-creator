# e2e/run.sh: how every suite runs `cozy` and reads what a run made (e2e/lib.sh, release-gate/gate.sh and its suites).
# RUN_CLI is the cozy to call (default `cozy`). A run is kept in <dir> under its <name>: <name>.ndjson (its events),
# <name>.err, <name>.number (`none`: it never got one) and <name>.json, its whole record (`cozy run watch --once --json --full`).
# A run belongs to a project (`run_project`): its outputs land in the project's files/, and `run_output` downloads them by
# their digest (`cozy run watch -o`): Calcifer's paths are its own (cl-819).
#
# A run is followed by the CLI's own stream (`cozy --json run`, <name>.ndjson): it ends when that follower exits, at the
# run's terminal event once its outputs landed, or with a typed error. Nothing here ends a run or a read for taking
# long: a slow run watchs its progress in its events, a Calcifer that stops answering is named on the follower's stderr
# (<name>.err). Only a run that never gets its number ends its follower (`run_submit`).
RUN_CLI=${RUN_CLI:-cozy}
RUN_WARM_API=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)/warm.py

# run_project <dir>: <dir> made a project unless it is one, for this script's runs wherever the script goes.
run_project() {
  mkdir -p "$1" || return 1
  if [ -f "$1/project.db" ] || [ -f "$1/project.yaml" ]; then
    (cd "$1" && "$RUN_CLI" --json status > /dev/null) || return 1
  else
    (cd "$1" && "$RUN_CLI" --json new > /dev/null) || return 1
  fi
  RUN_PROJECT=$(cd "$1" && pwd)
}

# run_submit <dir> <name> <cozy args...>: starts `cozy --json <args>` (a verb that makes a numbered run) in the
# background and prints the run's number once its events name one; nothing when the command ended without one, or
# when none came in RUN_NUMBER_S (default 1860 s: local code installs before its call is sent, cold on a new pod). Each doubling wait
# without one is said on stderr (attempt k/5, with the follower's last line); after the fifth the follower is ended,
# never a run.
run_submit() {
  local dir=$1 name=$2; shift 2
  local words=("$@")
  if [[ ${words[0]} = run && ${words[1]:-} = .* ]]; then
    words[1]="$PWD/${words[1]//\\//}"
  fi
  run_start "$dir" "$name" "$RUN_CLI" --json "${words[@]}"
}

run_start() {
  local dir=$1 name=$2 n='' k=1 limit=${RUN_NUMBER_S:-1860} start=$SECONDS at; shift 2
  rm -f "$dir/$name".{ndjson,err,json,number,pid}
  (cd "${RUN_PROJECT:-$PWD}" && "$@") > "$dir/$name.ndjson" 2> "$dir/$name.err" &
  local pid=$!
  echo "$pid" > "$dir/$name.pid"
  at=$((limit / 31))
  until n=$(run_number "$dir/$name.ndjson"); [ -n "$n" ] || ! kill -0 "$pid" 2> /dev/null; do
    if [ $((SECONDS - start)) -ge "$at" ]; then
      if [ "$k" -ge 5 ]; then
        echo "$name: no run number in $((SECONDS - start)) s; its follower ends: $(tail -n 1 "$dir/$name.err" 2> /dev/null)" >&2
        kill "$pid" 2> /dev/null || true
        break
      fi
      echo "$name: no run number yet after $((SECONDS - start)) s (attempt $k/5): $(tail -n 1 "$dir/$name.err" 2> /dev/null)" >&2
      k=$((k + 1)) at=$((at * 2 + limit / 31))
    fi
    sleep 0.2
  done
  [ -n "$n" ] || n=$(run_number "$dir/$name.ndjson")
  echo "${n:-none}" > "$dir/$name.number"
  [ -z "$n" ] || echo "$n"
}

# API warming stays available to fixtures after the public CLI option is removed.
run_warm_submit() {
  local dir=$1 name=$2; shift 2
  run_start "$dir" "$name" python3 "$RUN_WARM_API" --cozy "${COZY:-$RUN_CLI}" "$@"
}

run_warm_cell() {
  local dir=$1 name=$2; shift 2
  [ -n "$(run_warm_submit "$dir" "$name" "$@")" ] || return 1
  run_wait "$dir" "$name" && [ "$(run_get "$dir" "$name" .status)" = completed ]
}
# Its events' lines (a merged stderr's are not JSON).
run_number() { { grep '^{' "$1" 2> /dev/null | jq -r 'select(.run.number != null) | .run.number' 2> /dev/null || true; } | head -1; }
# run_reached <ndjson> <status>: whether the run's events reached the status.
run_reached() { grep '^{' "$1" 2> /dev/null | jq -en --arg s "$2" 'any(inputs; .run.status? == $s)' > /dev/null 2>&1; }

# run_wait <dir> <name> [status]: run <name> followed to its end (or, given a status, until its events reach it), its
# record in <name>.json. 0: ended (or reached the status); 1: never started, ended without reaching the status, or its
# follower exited before the run ended (its typed error is in <name>.ndjson).
run_wait() {
  local dir=$1 name=$2 want=${3:-} n pid
  n=$(cat "$dir/$name.number" 2> /dev/null || true) pid=$(cat "$dir/$name.pid" 2> /dev/null || true)
  if [ -z "$n" ] || [ "$n" = none ]; then return 1; fi
  while kill -0 "$pid" 2> /dev/null; do
    if [ -n "$want" ] && run_reached "$dir/$name.ndjson" "$want"; then run_show "$dir" "$name" "$n"; return 0; fi
    sleep 0.2
  done
  run_show "$dir" "$name" "$n"
  if [ -n "$want" ]; then run_reached "$dir/$name.ndjson" "$want"; return; fi
  case $(run_get "$dir" "$name" .status) in completed | failed | canceled) return 0 ;; esac
  return 1
}

# run_show <dir> <name> <number>: the run's record now, into <name>.json (kept as it was when the read fails). `run
# show` exits non-zero for a run that failed or was canceled: the record is what it printed.
run_show() {
  "$RUN_CLI" --json --full run watch "$3" --once > "$1/$2.json.new" 2> /dev/null || true
  if jq -e .status "$1/$2.json.new" > /dev/null 2>&1; then
    mv "$1/$2.json.new" "$1/$2.json"
  else
    rm -f "$1/$2.json.new"
  fi
}

# run_cell <dir> <name> <target> [inputs and flags, --on among them]: one `cozy run`, followed to its end. 0: completed; 1: it ended otherwise or never started.
run_cell() {
  local dir=$1 name=$2 code=0; shift 2
  [ -n "$(run_submit "$dir" "$name" run "$@")" ] || return 1
  run_wait "$dir" "$name" || code=$?
  [ "$code" != 0 ] || [ "$(run_get "$dir" "$name" .status)" = completed ] || code=1
  return "$code"
}

# run_uploaded <ndjson>: "<first> <most> <total>" bytes of the run's upload to its machine, from its waiting reasons
# (`uploading <name>: 9.9 MiB/256.0 MiB at 5.5 MiB/s`); "0 0 0" with none. A resumed upload's first is what the machine held.
run_uploaded() {
  grep '^{' "$1" 2> /dev/null | jq -rn 'def b: capture("^(?<n>[0-9.]+) (?<u>[KMGT]?i?B)$") | (.n | tonumber) * {"B": 1, "KiB": 1024, "MiB": 1048576, "GiB": 1073741824, "TiB": 1099511627776}[.u];
    [inputs | select(.type? == "run.waiting") | .reason // "" | capture("^uploading .*: (?<d>[0-9.]+ [KMGT]?i?B)/(?<t>[0-9.]+ [KMGT]?i?B)") | [(.d | b), (.t | b)]]
    | if length == 0 then "0 0 0" else "\(.[0][0] | floor) \(map(.[0]) | max | floor) \(.[0][1] | floor)" end'
}

# run_get <dir> <name> <jq filter>: from its record, empty when absent.
run_get() { jq -r "($3) // empty" "$1/$2.json" 2> /dev/null || true; }

# run_outputs <dir> <name>: each landed output of the run, downloaded into <dir>/<name>.out by its digest, in its record's
# order, one path a line.
run_outputs() {
  local n name line path
  n=$(cat "$1/$2.number" 2> /dev/null || true)
  [ -n "$n" ] && [ "$n" != none ] || return 0
  # JSON's paths belong to Calcifer and are not sent; the CLI says each downloaded path on stderr.
  "$RUN_CLI" run watch "$n" --once -o "$1/$2.out" > /dev/null 2> "$1/$2.export.err" || true
  while IFS= read -r line; do
    while IFS= read -r name; do
      case "$line" in
        "$name: "*) path=${line#"$name: "}; if [ -e "$path" ]; then printf '%s\n' "$path"; break; fi ;;
      esac
    done < <(run_get "$1" "$2" '.output[]? | select(.url) | .name' | uniq)
  done < "$1/$2.export.err"
}

# run_output <dir> <name> <extensions, as a|b>: its first landed output with one of these extensions.
run_output() { run_outputs "$1" "$2" | grep -E "\.($3)$" | head -1 || true; }
