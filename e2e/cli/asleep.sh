# e2e/cli/asleep.sh: a run that makes no progress is followed to its end (cl-977), the part cli/run.sh calls `asleep`, with the
# local machine up. A call asleep past Calcifer's 10-minute "no progress" is said to make none, and `cozy run` follows it on to its
# end, exit 0 (before, it stopped following with exit 4). While it sleeps, its executor's call thread is named in the kernel
# (`call_<n>`, cozy-runtime#2205).

# followed <name> <target> [inputs...]: `cozy run` on local in the foreground, at most 15 min; its exit in $OUT/<name>.exit and
# its last record in $OUT/<name>.last.
followed() {
  local name=$1 code=0
  shift
  (cd "$RUN_PROJECT" && timeout 900 cozy --json run "$@" --on local > "$OUT/$name.ndjson" 2> "$OUT/$name.err") || code=$?
  echo "$code" > "$OUT/$name.exit"
  grep '^{' "$OUT/$name.ndjson" | jq -c 'select(.run?) | .run' | tail -1 > "$OUT/$name.last"
  [ "$code" = 0 ] && jq -e '.status == "completed"' "$OUT/$name.last" > /dev/null && return 0
  check "$name: cozy run exited $code, run $(jq -c '[.number, .status, .reason]' "$OUT/$name.last" 2> /dev/null): $(tail -c 300 "$OUT/$name.err")"
  return 1
}

asleep() {
local follower names
step "a call asleep past Calcifer's 10 minutes: cozy run says it made no progress and follows it on to its end"
(followed asleep "$here/weightless/silent" seconds:=660) & follower=$!
for _ in $(seq 300); do grep -qE '"stage": *"waiting silently"' "$OUT/asleep.ndjson" 2> /dev/null && break; sleep 1; done
names=$(for p in $(pgrep -f 'cozy_runtime.internal.executor'); do cat /proc/"$p"/task/*/comm 2> /dev/null; done | sort | uniq -c | tr -s ' \n' ' ')
echo "asleep: its executors' threads: $names"
[[ $names == *" call_"* ]] || check "asleep: no executor thread is named call_<n> in the kernel: $names"
wait "$follower" || return 0
grep -qE '"stalled": ?\{' "$OUT/asleep.ndjson" || check "asleep: cozy run's events never said the run made no progress"
echo "asleep: run $(jq -r .number "$OUT/asleep.last") completed after 11 min asleep, cozy run exit 0"
}
