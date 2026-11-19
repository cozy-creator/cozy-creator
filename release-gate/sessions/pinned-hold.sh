#!/bin/bash
# pinned-hold (proof only, cl-1316 cleanup; never merged): a run asked on the session's pod (`--on`), taken there, and then the
# session holds (900 s) for `gh run cancel` to kill its step before its trap: pod-session.yaml's always() cleanup must stop the
# run's follower, cancel the run (machines.sh `unpin_runs`) and end the pod. Nothing here ends anything.
. "$(dirname "$0")/lib.sh"
quote 600
rent
W=$(cd "$(dirname "$0")/../../e2e/cli/weightless" && pwd)
(cd "$RUN_PROJECT" && exec cozy --json run "$W/wait" seconds:=300 --on "$NAME") > "$OUT/held.ndjson" 2> "$OUT/held.err" &
for _ in $(seq 1 60); do
  grep -qs '"type":"run.in_progress"' "$OUT/held.ndjson" && break; sleep 5
done
say "HOLDING: $(handle "$OUT/held.ndjson") is on $NAME; cancel this run now"
sleep 600
