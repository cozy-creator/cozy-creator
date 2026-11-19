#!/bin/bash
# pinned-lost (proof only, cl-1316; never merged): a run asked on its pod (`--on`) whose pod ends under it fails at once, naming the
# pod, and runs nowhere else. One cpu pod: e2e/cli/weightless `wait` (600 s) runs on it; once it runs, the pod is ended (--force).
# If the run is ever placed on another machine, it is canceled at once and the session fails.
. "$(dirname "$0")/lib.sh"
quote 600
rent
W=$(cd "$(dirname "$0")/../../e2e/cli/weightless" && pwd)
cozy --json run "$W/wait" seconds:=600 --on "$NAME" > "$OUT/pinned.ndjson" 2> "$OUT/pinned.err" & P=$!
h=
for _ in $(seq 1 120); do
  h=$(handle "$OUT/pinned.ndjson"); [ -n "$h" ] && [ "$(cozy --json run show "$h" 2> /dev/null | jq -r '.state // empty')" = running ] && break
  kill -0 "$P" 2> /dev/null || fail "the run ended before it ran: $(tail -c 300 "$OUT/pinned.err")"
  sleep 5
done
[ "$(cozy --json run show "$h" 2> /dev/null | jq -r '.state // empty')" = running ] || fail "run $h never ran on $NAME"
say "run $h is running on $(cozy --json run show "$h" | jq -r '.machine // empty'); ending $NAME under it"
cozy machine end "$NAME" -y --force > "$OUT/end-pod.txt" 2>&1 || fail "end $NAME: $(tail -c 300 "$OUT/end-pod.txt")"
for _ in $(seq 1 60); do
  kill -0 "$P" 2> /dev/null || break
  m=$(cozy --json run show "$h" 2> /dev/null | jq -r '.machine // empty')
  if [ -n "$m" ] && [ "$m" != "$NAME" ]; then cozy run cancel "$h" > "$OUT/cancel.txt" 2>&1; fail "run $h moved to $m: canceled"; fi
  sleep 5
done
kill -0 "$P" 2> /dev/null && { cozy run cancel "$h" > "$OUT/cancel.txt" 2>&1; fail "run $h still unsettled 5 min after $NAME ended: canceled"; }
wait "$P"; code=$?
cozy --json run show "$h" > "$OUT/pinned-show.json" 2>&1
cozy run show "$h" > "$OUT/pinned-show.txt" 2>&1
say "the run's follow exited $code: $(grep -v '^{' "$OUT/pinned.err" | tail -2 | paste -sd' ' | cut -c1-300)"
say "its record: $(jq -c '{status, machine, attempt: .retry.attempt, reason: {code: .reason.code, message: .reason.message}}' "$OUT/pinned-show.json" 2> /dev/null | cut -c1-400)"
jq -e --arg m "$NAME" '.status == "failed" and .reason.code == "machine_lost" and .machine == $m and (.reason.message | test("runs nowhere else"))' \
  "$OUT/pinned-show.json" > /dev/null || fail "not failed machine_lost on $NAME, saying it runs nowhere else"
! grep -qs 'goes wherever\|places it again' "$OUT/pinned.ndjson" "$OUT/pinned.err" "$OUT/pinned-show.txt" || fail "its log says it would be placed again"
say "PINNED: $h failed at once on $NAME's end and ran nowhere else"
