# e2e/cli/hangs.sh: runs never get stuck (cl-977 G9), with the local machine up, on a runner that denies TensorD ptrace over
# its executors as a rental host can (Yama `ptrace_scope` 2: emul, pod-session 43785452864), so only each executor's own vigil
# can say what its threads wait on.
#   hangs   A call that deadlocks fails `hung` once nothing of it moved for 60 s, and the machine goes idle after. Nothing judges
#           a call by its pace: one busy on its CPU without a word past that window and one waiting on a TCP peer that answers
#           late complete, each followed by `cozy run` to its end, exit 0.
#   asleep  A call asleep past Calcifer's 10-minute "no progress": `cozy run` says so and follows it on to its end, exit 0.

# hang_threads: each executor's threads (name, wait channel, run time), when a cell went wrong.
hang_threads() {
  local pid task
  for pid in $(pgrep -f 'cozy_runtime.internal.executor' || true); do
    echo "executor $pid:"
    for task in /proc/"$pid"/task/*; do
      echo "  $(cat "$task/comm" 2> /dev/null) $(cat "$task/wchan" 2> /dev/null) $(cut -d' ' -f1 "$task/schedstat" 2> /dev/null)"
    done
  done
}

# unptraced: Yama ptrace_scope 2 here, checked: a parent can't read its own child's syscall. `ptraced` puts 1 back.
unptraced() {
  sudo -n sysctl -qw kernel.yama.ptrace_scope=2 || { check "hangs: kernel.yama.ptrace_scope=2 not set"; return 1; }
  python3 -I -c 'import subprocess
child = subprocess.Popen(["sleep", "5"])
try:
    open(f"/proc/{child.pid}/syscall").read()
    print("readable")
except PermissionError:
    print("denied")
child.kill()' | grep -qx denied || { check "hangs: a parent still reads its child's syscall under ptrace_scope 2"; return 1; }
}
ptraced() { sudo -n sysctl -qw kernel.yama.ptrace_scope=1 || true; }

# followed <name> <target> [inputs...]: `cozy run` on local in the foreground, at most 15 min; its exit in $OUT/<name>.exit and
# its last record in $OUT/<name>.last.
followed() {
  local name=$1 code=0
  shift
  (cd "$RUN_PROJECT" && timeout 900 cozy --json run "$@" --on local > "$OUT/$name.ndjson" 2> "$OUT/$name.err") || code=$?
  echo "$code" > "$OUT/$name.exit"
  grep '^{' "$OUT/$name.ndjson" | jq -c 'select(.run?) | .run' | tail -1 > "$OUT/$name.last"
  [ "$code" = 0 ] && J '.status == "completed"' "$OUT/$name.last" && return 0
  check "$name: cozy run exited $code, run $(jq -c '[.number, .status, .reason]' "$OUT/$name.last" 2> /dev/null): $(tail -c 300 "$OUT/$name.err")"
  hang_threads
  return 1
}

hangs() {
local server port t0 took
[ "${RUNNER_OS:-Linux}" = Linux ] || return 0
unptraced || return 0
step "a deadlocked call fails hung, its executor killed"
t0=$SECONDS
(cd "$here" && ALLOW=hung bounded_cell 300 deadlocked ./weightless/deadlock) || hang_threads
took=$((SECONDS - t0))
J '.status == "failed" and .reason.code == "hung"' "$OUT/deadlocked.json" \
  || { check "deadlocked: run $(cat "$OUT/deadlocked.number") is $(jq -c '[.status, .reason]' "$OUT/deadlocked.json"), not failed hung"; hang_threads; }
[ "$took" -ge 60 ] || check "deadlocked: it failed after ${took} s, inside the 60 s window"
echo "deadlocked: run $(cat "$OUT/deadlocked.number") failed $(jq -c '.reason' "$OUT/deadlocked.json") after ${took} s"
cozy machine logs local -n 0 > "$OUT/hangs-machine.log" 2>&1 || true
grep -q "its vigil's line is up" "$OUT/hangs-machine.log" || check "deadlocked: no executor's vigil line came up (TensorD's log)"

step "a call busy on its CPU without a word for 75 s completes"
followed spin "$here/weightless/spin" seconds:=75 && echo "spin: run $(jq -r .number "$OUT/spin.last") completed, cozy run exit 0"

step "a call waiting 75 s on a TCP peer that answers late completes"
python3 -I - "$OUT/peer.port" <<'PY' > "$OUT/peer.log" 2>&1 &
import socket, sys, time
server = socket.create_server(("127.0.0.1", 0))
open(sys.argv[1], "w").write(str(server.getsockname()[1]))
asker, _ = server.accept()
asker.makefile().readline()
time.sleep(75)
asker.sendall(b"late\n")
time.sleep(5)
PY
server=$!
for _ in $(seq 50); do [ -s "$OUT/peer.port" ] && break; sleep 0.1; done
port=$(cat "$OUT/peer.port" 2> /dev/null)
if [ -n "$port" ]; then
  followed peer "$here/weightless/remote" host=127.0.0.1 "port:=$port" \
    && { J '.result.reply == "late"' "$OUT/peer.last" || check "peer: run $(jq -r .number "$OUT/peer.last") answered $(jq -c .result "$OUT/peer.last")"; } \
    && echo "peer: run $(jq -r .number "$OUT/peer.last") completed with the peer's late answer, cozy run exit 0"
else
  check "peer: the peer never listened: $(tail -c 300 "$OUT/peer.log")"
fi
kill "$server" 2> /dev/null || true
wait "$server" 2> /dev/null || true

step "the machine goes idle once its runs ended"
for _ in $(seq 60); do
  cozy --json machine show local > "$OUT/hangs-machine.json" 2> /dev/null || true
  J '.status.idle_deadline_ms > 0' "$OUT/hangs-machine.json" && break
  sleep 1
done
J '.status.idle_deadline_ms > 0' "$OUT/hangs-machine.json" \
  || check "hangs: the machine isn't idle after its runs ended: $(jq -c '.status | {phase, runs, idle_deadline_ms}' "$OUT/hangs-machine.json")"
ptraced
}

asleep() {
[ "${RUNNER_OS:-Linux}" = Linux ] || return 0
unptraced || return 0
step "a call asleep past Calcifer's 10 minutes: cozy run says it made no progress and follows it on to its end"
followed asleep "$here/weightless/silent" seconds:=660 || { ptraced; return 0; }
ptraced
grep -qE '"stalled": ?\{' "$OUT/asleep.ndjson" || check "asleep: cozy run's events never said the run made no progress"
echo "asleep: run $(jq -r .number "$OUT/asleep.last") completed after 11 min asleep, cozy run exit 0"
}
