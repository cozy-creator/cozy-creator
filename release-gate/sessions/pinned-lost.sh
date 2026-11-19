#!/bin/bash
# pinned-lost (proof only, cl-1316; never merged): a run asked on its pod (`--on`, `--gpus` from params `gpus`) whose pod ends under it
# never moves. e2e/cli/weightless `wait` (600 s) runs on the pod; once it runs, the pod is ended:
# - `lost` (Calcifer, #4094): ended under it (`machine end --force`): the run fails machine_lost, naming the pod, and runs nowhere else.
# - `unpin` (the harness, lib.sh `unpin`, as the trap and guard end a pod): its runs are canceled first: the run ends canceled on it.
# params `legs` (default "lost"), each on a pod of its own, one after the other. A run ever placed on another machine is canceled at once
# and the session fails.
. "$(dirname "$0")/lib.sh"
LEGS=$(param legs lost) GPUS_ASKED=$(param gpus '')
[[ -z $GPUS_ASKED || $GPUS_ASKED =~ ^[1-8]$ ]] || fail "gpus $GPUS_ASKED"
# The open RC's Runtime and TensorFS (params `runtime_url`, `tensorfs_url`), fetched before renting: this cozy's Runtime minor.
RW=$(wheel_from "$OUT/wheels/runtime" "$(param runtime_url)") && [ -n "$RW" ] || fail "no Runtime wheel from $(param runtime_url)"
TW=$(wheel_from "$OUT/wheels/tensorfs" "$(param tensorfs_url)") && [ -n "$TW" ] || fail "no TensorFS wheel from $(param tensorfs_url)"
quote $((600 * $(wc -w <<< "$LEGS")))
W=$(cd "$(dirname "$0")/../../e2e/cli/weightless" && pwd)
# events <ndjson>: the run's events, as its `cozy --json run` follow printed them.
events() { sed 's/^[0-9.]* {/{/' "$1" 2> /dev/null | grep '^{'; }
# started <ndjson>: `yes` once its machine began executing it. last <ndjson>: the run as its last event carrying it says.
started() { events "$1" | jq -se 'any(.[]; .run.started_at_ms != null)' > /dev/null 2>&1 && echo yes || echo no; }
last() { events "$1" | jq -sc 'map(select(.run != null)) | last | .run'; }
record() { last "$1" | jq -c '{status, machine, reason: {code: .reason.code, message: .reason.message}}' | cut -c1-400; }
for leg in $LEGS; do
  [[ $leg == lost || $leg == unpin ]] || fail "leg $leg"
  rent
  cozy machine update "$NAME" --runtime "$RW" --tensorfs "$TW" --tensord "$(command -v cozy)" -y > "$OUT/update-$leg.txt" 2>&1 ||
    fail "$leg: update $NAME: $(tail -c 300 "$OUT/update-$leg.txt")"
  say "$leg: $NAME $(cozy machine show "$NAME" 2>&1 | grep -E '^(version|runtime|GPU 0) ' | paste -sd' ' | tr -s ' ')"
  # A run belongs to the session's project (lib.sh RUN_PROJECT): asked from inside it.
  (cd "$RUN_PROJECT" && exec cozy --json run "$W/wait" seconds:=600 --on "$NAME" ${GPUS_ASKED:+--gpus=$GPUS_ASKED}) \
    > "$OUT/$leg.ndjson" 2> "$OUT/$leg.err" & P=$!
  h=
  for _ in $(seq 1 120); do
    h=$(handle "$OUT/$leg.ndjson"); [ "$(started "$OUT/$leg.ndjson")" = yes ] && break
    kill -0 "$P" 2> /dev/null || fail "$leg: the run ended before it ran: $(tail -c 300 "$OUT/$leg.err") $(tail -c 300 "$OUT/$leg.ndjson")"
    sleep 5
  done
  [ "$(started "$OUT/$leg.ndjson")" = yes ] || fail "$leg: run $h never ran on $NAME"
  say "$leg: run $h${GPUS_ASKED:+ (--gpus=$GPUS_ASKED)} is running on $(last "$OUT/$leg.ndjson" | jq -r '.machine // empty'); ending $NAME under it"
  if [ "$leg" = unpin ]; then unpin "$RID"; cozy machine end "$RID" -y > "$OUT/end-$leg.txt" 2>&1 || fail "unpin: end $NAME: $(tail -c 300 "$OUT/end-$leg.txt")"
  else cozy machine end "$NAME" -y --force > "$OUT/end-$leg.txt" 2>&1 || fail "lost: end $NAME: $(tail -c 300 "$OUT/end-$leg.txt")"; fi
  for _ in $(seq 1 60); do
    kill -0 "$P" 2> /dev/null || break
    m=$(events "$OUT/$leg.ndjson" | jq -r '.run.machine // empty' | grep -vx "$NAME" | head -1)
    if [ -n "$m" ] && [ "$m" != "$NAME" ]; then cozy run cancel "$h" -y > "$OUT/cancel.txt" 2>&1; fail "$leg: run $h moved to $m: canceled"; fi
    sleep 5
  done
  kill -0 "$P" 2> /dev/null && { cozy run cancel "$h" -y > "$OUT/cancel.txt" 2>&1; fail "$leg: run $h still unsettled 5 min after $NAME ended: canceled"; }
  wait "$P"; code=$?
  last "$OUT/$leg.ndjson" > "$OUT/$leg-show.json"
  say "$leg: the run's follow exited $code: $(grep -v '^{' "$OUT/$leg.err" | tail -2 | paste -sd' ' | cut -c1-300)"
  say "$leg: its record: $(record "$OUT/$leg.ndjson")"
  [ -z "$(events "$OUT/$leg.ndjson" | jq -r '.run.machine // empty' | grep -vx "$NAME")" ] || fail "$leg: an event names another machine"
  ! grep -qs 'goes wherever\|places it again' "$OUT/$leg.ndjson" "$OUT/$leg.err" || fail "$leg: its log says it would be placed again"
  if [ "$leg" = lost ]; then
    jq -e --arg m "$NAME" '.status == "failed" and .reason.code == "machine_lost" and .machine == $m and (.reason.message | test("runs nowhere else"))' \
      "$OUT/$leg-show.json" > /dev/null || fail "lost: not failed machine_lost on $NAME, saying it runs nowhere else"
    say "PINNED lost: $h failed at once on $NAME's end and ran nowhere else"
  else
    jq -e --arg m "$NAME" '.status == "canceled" and .machine == $m' "$OUT/$leg-show.json" > /dev/null || fail "unpin: not canceled on $NAME"
    say "PINNED unpin: $h canceled on $NAME before it ended, and ran nowhere else"
  fi
  for _ in $(seq 1 60); do gone "$RID" && break; sleep 10; done
  gone "$RID" && say "$leg: GONE $RID" || fail "$leg: $RID still live 10 min after its end"
done
