#!/bin/bash
# end-cancels (proof only, never merged): a session ended under its step, as its deadline watcher ends it (`kill -TERM $$`), stops
# the step's cozy and cancels its run before its pod ends (lib.sh `finish`, machines.sh `cancel_runs`). e2e/cli/weightless `wait`
# (300 s) is asked of the pod through `step`; once it runs, a watcher TERMs the session.
. "$(dirname "$0")/lib.sh"
quote 600
rent
W=$(cd "$(dirname "$0")/../../e2e/cli/weightless" && pwd) || fail "no e2e/cli/weightless in this checkout"
( for _ in $(seq 1 90); do
    sleep 5
    (cd "$RUN_PROJECT" && cozy --json run ls --all --limit 0 < /dev/null 2> /dev/null) | jq -e '.runs[]? | select(.status == "in_progress")' > /dev/null && break
  done
  sleep 10; say "HOLDING: the run is in progress on $NAME; the session is ended under its step, as its deadline watcher ends it"; kill -TERM $$ ) 9>&- & BG=$!
step held run "$W/wait" seconds:=300 --on "$NAME"
fail "the step ended before the session was ended under it"
