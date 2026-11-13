#!/usr/bin/env bash
# e2e/skew/rehub.sh [--set key=value]...: the isolated Hub e2e/skew/run.sh started, restarted with these settings over its own (a
# cell's Hub of other software, cl-978). The exact process its pid file names stops; the new one's pid goes there, for run.sh's
# cleanup, and it logs where the first did.
set -euo pipefail
pidfile=${COZY_TEST_HUB_PID_FILE:?run by e2e/skew/run.sh} argv=${COZY_TEST_HUB_COMMAND_FILE:?} log=${COZY_TEST_HUB_SERVE_LOG:?} health=${COZY_TEST_HUB_HEALTH:?}
hub=()
while IFS= read -r -d '' word; do hub+=("$word"); done < "$argv"
pid=$(cat "$pidfile")
kill "$pid" 2> /dev/null || true
for _ in $(seq 60); do kill -0 "$pid" 2> /dev/null || break; sleep 0.5; done
kill -0 "$pid" 2> /dev/null && { echo "rehub: the Hub ($pid) didn't stop" >&2; exit 1; }
nice -n 19 "${hub[@]}" "$@" serve >> "$log" 2>&1 &
echo $! > "$pidfile"
for _ in $(seq 90); do [ "$(curl -s -o /dev/null -w '%{http_code}' "$health")" = 204 ] && exit 0; sleep 1; done
echo "rehub: the Hub didn't come up with $*: $(tail -c 400 "$log")" >&2
exit 1
