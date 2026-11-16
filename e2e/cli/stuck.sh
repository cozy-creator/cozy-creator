# e2e/cli/stuck.sh: TensorD's stop, restart, update and start finish within their bounds when a TensorD can't exit (cl-977), on this
# computer's machine with no GPU (Linux). A service slow to exit is killed 10 s after its stop and ends as it exits. A service whose exit sits
# in the kernel, as muromisan's sat in the NVIDIA driver: its stdout and stderr (start.log) are on a FUSE filesystem
# (stuck/trapfs.py) that holds the FLUSH its exit sends, for that pid alone, so its last thread waits in D state. An update's
# restart, a stop, and a start beside it each finish; machine show/ls list it; runs still go there (no GPU: nothing it holds is
# needed, so not broken); once the filesystem answers it exits and Status drops it. Then a start that hangs (every write to its
# log held) is refused machine_not_answering at Calcifer's deadline, naming the next command, and serves once released.
# Not here: `broken` (GPUs a fresh process can't open, or a rental silent after an update) needs a GPU or a real rental.

STUCK_ROOT=$HOME/.cozy/machine/var/lib/cozy
stuck_log() { cat "$STUCK_ROOT"/machine/logs/tensord.*.log 2> /dev/null; }
# The live service of the local machine: its stable parent's child named tensord, not a zombie.
stuck_service() {
  local parent
  parent=$(machine_pid) || return 1
  ps -o pid=,stat=,comm= --ppid "$parent" | awk '$3 == "tensord" && $2 !~ /^Z/ { print $1 }' | tail -1
}
# A thread of <pid> not yet exited (its exit waits in the kernel: D, or S for the thread that called exit, whose kill the kernel drops).
stuck_in_kernel() { grep -lqE '^[0-9]+ \(.*\) [DS] ' /proc/"$1"/task/*/stat 2> /dev/null; }
stuck_states() { for t in /proc/"$1"/task/*; do echo "$(cut -d' ' -f3 "$t/stat" 2> /dev/null) $(cat "$t/wchan" 2> /dev/null)"; done | tr '\n' ' '; }
# gone <pid> <seconds>: every thread of it exited (a zombie leader counts only once its last thread is gone), within that long.
stuck_gone() {
  local t live
  for _ in $(seq "$2"); do
    live=''
    for t in /proc/"$1"/task/*/stat; do grep -qE '\) [^ZX] ' "$t" 2> /dev/null && live=1; done
    [ -n "$live" ] || return 0
    sleep 1
  done
  return 1
}
# ready <seconds>: `machine show local` says ready within that long (it starts the machine).
# diagnose <name>: what a failed step needs read, into $OUT/stuck-<name>.diag: the connections to the machine's port, TensorD's
# processes and threads, the runs, and the last lines of TensorD's and Calcifer's logs.
stuck_diagnose() {
  local port
  port=$(cat "$STUCK_ROOT/machine/api-port" 2> /dev/null)
  {
    echo "== connections to :$port"; ss -tnpoi state established "( sport = :$port or dport = :$port )" 2>&1
    echo "== tensord threads"; ps -eLo pid,tid,ppid,stat,wchan:32,comm | awk 'NR == 1 || $6 == "tensord" || $6 ~ /^tokio/ || $6 ~ /^machine-/'
    echo "== runs"; timeout 30 cozy --json run ls --all --limit 10 2>&1 | head -c 4000
    echo "== tensord log"; stuck_log | tail -40
    echo "== calcifer"; tail -40 "$HOME/.cozy/calcifer/diagnostics.log" 2>&1
  } > "$OUT/stuck-$1.diag" 2>&1
}
# The connections to the machine's port every 20 s while <pid> lives, into $OUT/stuck-<name>.ss.
stuck_watch_connections() {
  local port
  port=$(cat "$STUCK_ROOT/machine/api-port" 2> /dev/null)
  while kill -0 "$2" 2> /dev/null; do
    { date -u +%T; ss -tnpoi state established "( sport = :$port or dport = :$port )"; } >> "$OUT/stuck-$1.ss" 2>&1
    sleep 20
  done
}
# run <name> [--on local]: a weightless run (reverse) followed at most 240 s, into $OUT/stuck-<name>.ndjson; its exit.
stuck_run() {
  local name=$1 text=stuck$RANDOM
  shift
  (cd "$RUN_PROJECT" && timeout 240 cozy --json run "$here/weightless/reverse" "text=$text" "$@" > "$OUT/stuck-$name.ndjson" 2>&1)
}
# settled <seconds>: no update run on local left unsettled, within that long.
stuck_settled() {
  for _ in $(seq "$1"); do
    timeout 30 cozy --json run ls --all --on local > "$OUT/stuck-runs.json" 2> /dev/null || true
    J '[.runs[]? | select(.kind == "update" and (.status | IN("completed", "failed", "canceled") | not))] | length == 0' "$OUT/stuck-runs.json" && return 0
    sleep 1
  done
  return 1
}
# listed <pid...>: within 30 s, machine show local lists each as the TensorD before this one (the first frames after a start may
# still be the last service's).
stuck_listed() {
  local pid all
  for _ in $(seq 30); do
    cozy --json machine show local > "$OUT/stuck-machine.json" 2> /dev/null || true
    all=1
    for pid in "$@"; do
      J --argjson pid "$pid" 'any(.status.stuck[]?; .pid == $pid and .command == "the TensorD before this one")' "$OUT/stuck-machine.json" || all=''
    done
    [ -n "$all" ] && return 0
    sleep 1
  done
  return 1
}
stuck_ready() {
  for _ in $(seq "$1"); do
    cozy --json machine show local > "$OUT/stuck-machine.json" 2> "$OUT/stuck-machine.err" || true
    J '.state == "ready" and (.unreachable // "") == ""' "$OUT/stuck-machine.json" && return 0
    sleep 1
  done
  return 1
}

stuck_exit() {
[ -n "$LOCAL" ] || return 0
step "stuck: a TensorD whose exit sits in the kernel (cl-977)"
local fs=$OUT/stuck-fs mnt=$RUNNER_TEMP/stuck-mnt trap=$OUT/stuck-trap log=$STUCK_ROOT/machine/start.log
local service parent t0 took port first second named
mkdir -p "$fs" "$mnt"
: > "$trap"
# FUSE 2 (fusermount, libfuse.so.2), as the project-encryption driver installs it.
{ command -v fusermount > /dev/null && ldconfig -p | grep -q 'libfuse\.so\.2'; } || bash "$here/install-vault-driver.sh" > "$OUT/stuck-fuse.log" 2>&1 \
  || { check "stuck: no FUSE 2: $(tail -c 300 "$OUT/stuck-fuse.log")"; return; }
python3 -m venv "$RUNNER_TEMP/trapfs" > "$OUT/trapfs-venv.log" 2>&1 && "$RUNNER_TEMP/trapfs/bin/pip" -q install fusepy==3.0.1 >> "$OUT/trapfs-venv.log" 2>&1 \
  || { check "stuck: no fusepy: $(tail -c 300 "$OUT/trapfs-venv.log")"; return; }
"$RUNNER_TEMP/trapfs/bin/python" "$here/stuck/trapfs.py" "$fs" "$mnt" "$trap" > "$OUT/trapfs.log" 2>&1 &
STUCK_FS=$!
for _ in $(seq 30); do mountpoint -q "$mnt" && break; sleep 1; done
mountpoint -q "$mnt" || { check "stuck: no trap filesystem at $mnt: $(tail -c 300 "$OUT/trapfs.log")"; return; }
# TensorD's stdout and stderr there: its exit closes them, a FLUSH the filesystem holds for the pid in $trap.
machine_stop || check "stuck: the local machine did not stop"
: > "$fs/start.log"
ln -sf "$mnt/start.log" "$log"
stuck_ready 180 || { check "stuck: the machine didn't start on the trap filesystem: $(tail -c 300 "$OUT/stuck-machine.json")"; return; }
port=$(cat "$STUCK_ROOT/machine/api-port")

step "stuck: a stop whose service is slow to exit is killed at 10 s and ends as it exits"
service=$(stuck_service) parent=$(machine_pid)
# Each of its two log descriptors' FLUSH held 7 s: its exit takes about 14 s.
echo "$service:7" > "$trap"
t0=$SECONDS
kill "$parent"
stuck_gone "$parent" 60 || check "stuck: the machine (pid $parent) runs 60 s after its stop, its service slow to exit"
took=$((SECONDS - t0))
[ "$took" -ge 11 ] && [ "$took" -le 25 ] || check "stuck: a stop with a service about 14 s slow to exit took ${took} s"
stuck_log | grep -q "the service (pid $service) did not exit within 10 s; killing it" || check "stuck: TensorD's log doesn't say it killed pid $service"
stuck_log | grep -qE "the service \(pid $service\) exited [0-9]+ s after its kill" || check "stuck: TensorD's log doesn't say pid $service exited after its kill"
echo "stuck: stopped in ${took} s, service $service about 14 s slow to exit"
: > "$trap"
stuck_ready 120 || { check "stuck: the machine didn't start again after a slow stop: $(tail -c 300 "$OUT/stuck-machine.json")"; return; }

step "stuck: an update's restart past a service that can't exit"
first=$(stuck_service)
echo "$first" > "$trap"
t0=$SECONDS
timeout 300 cozy --json machine update local --gpu-memory-limit off -y > "$OUT/stuck-update.json" 2>&1 &
local updating=$!
stuck_watch_connections update "$updating" &
wait "$updating" || { check "stuck: the update failed: $(tail -c 300 "$OUT/stuck-update.json")"; stuck_diagnose update; }
took=$((SECONDS - t0))
grep -q "holding a flush" "$OUT/trapfs.log" || check "stuck: the filesystem held no flush from pid $first"
stuck_in_kernel "$first" || check "stuck: pid $first, the service before, isn't waiting in the kernel: $(stuck_states "$first")"
echo "stuck: pid $first's threads: $(stuck_states "$first")"
# 10 s to exit, 60 s with no progress, then the next service's boot.
[ "$took" -le 200 ] || check "stuck: the update took ${took} s past a service that can't exit"
stuck_log | grep -q "pid $first can't exit: thread " || check "stuck: TensorD's log doesn't say pid $first can't exit"
# Its thread named only as the machine names it (its place, or the Runtime's call_<n> / ~watcher), never by a comm any code can set:
# the line is kept in Calcifer's store once the machine ends (cl-1012, #3393's review).
named=$(stuck_log | grep -oE "pid $first can't exit: thread [0-9]+ \([^)]*\)" | head -1)
[[ $named =~ \((#[0-9]+|call_[0-9]+|~watcher)\)$ ]] || check "stuck: the line names its thread by a free-form name: $named"
stuck_ready 30 || { check "stuck: the machine isn't ready after its update: $(tail -c 300 "$OUT/stuck-machine.json")"; stuck_diagnose after-update; }
stuck_listed "$first" || check "stuck: machine show local lists no stuck pid $first: $(jq -c .status.stuck "$OUT/stuck-machine.json")"
cozy machine show local > "$OUT/stuck-show.txt" 2>&1 || true
grep -qE "pid $first \(the TensorD before this one\), killed .* stuck in the kernel" "$OUT/stuck-show.txt" \
  || check "stuck: machine show local doesn't say pid $first is stuck: $(grep -i stuck "$OUT/stuck-show.txt")"
cozy machine ls > "$OUT/stuck-ls.txt" 2>&1 || true
grep -qE '^local .*1 stuck' "$OUT/stuck-ls.txt" || check "stuck: machine ls doesn't mark local stuck: $(grep '^local' "$OUT/stuck-ls.txt")"
J '.broken == null' "$OUT/stuck-machine.json" || check "stuck: a machine with no GPU is broken: $(jq -c .broken "$OUT/stuck-machine.json")"
stuck_settled 120 || { check "stuck: the update run never settled: $(jq -c '[.runs[]? | select(.kind == "update") | {number, status, waiting}]' "$OUT/stuck-runs.json")"; stuck_diagnose unsettled; }
stuck_run after-update --on local || check "stuck: no run went to the machine after its update: $(tail -c 300 "$OUT/stuck-after-update.ndjson")"
echo "stuck: updated in ${took} s past pid $first, which waits in the kernel"

step "stuck: a stop and a start beside a TensorD that can't exit"
parent=$(machine_pid)
kill "$parent"
stuck_gone "$parent" 30 || check "stuck: the machine (pid $parent) runs 30 s after its stop"
t0=$SECONDS
stuck_ready 120 || { check "stuck: the machine didn't start beside pid $first: $(tail -c 300 "$OUT/stuck-machine.json")"; stuck_diagnose beside; }
took=$((SECONDS - t0))
[ "$(cat "$STUCK_ROOT/machine/api-port")" = "$port" ] || check "stuck: the machine left its port $port to the stuck pid $first: now $(cat "$STUCK_ROOT/machine/api-port")"
stuck_listed "$first" || check "stuck: after its start, pid $first isn't listed: $(jq -c .status.stuck "$OUT/stuck-machine.json")"
stuck_run after-start --on local || check "stuck: no run went to the machine started beside pid $first: $(tail -c 300 "$OUT/stuck-after-start.ndjson")"
echo "stuck: started in ${took} s beside pid $first, on port $port"

step "stuck: a stop whose service can't exit"
second=$(stuck_service) parent=$(machine_pid)
echo "$first $second" > "$trap"
t0=$SECONDS
kill "$parent"
stuck_gone "$parent" 150 || check "stuck: the machine (pid $parent) runs 150 s after its stop, its service stuck: $(ps -o pid,stat,args -p "$parent")"
took=$((SECONDS - t0))
# 10 s to exit, 60 s with no progress.
[ "$took" -le 90 ] || check "stuck: a stop with a service that can't exit took ${took} s"
stuck_in_kernel "$second" || check "stuck: pid $second isn't waiting in the kernel after the stop: $(stuck_states "$second")"
stuck_ready 120 || check "stuck: the machine didn't start beside pids $first and $second: $(tail -c 300 "$OUT/stuck-machine.json")"
stuck_listed "$first" "$second" || check "stuck: machine show local doesn't list both stuck TensorDs: $(jq -c .status.stuck "$OUT/stuck-machine.json")"
echo "stuck: stopped in ${took} s with pid $second stuck"

step "stuck: once the filesystem answers, they exit and Status drops them"
: > "$trap"
stuck_gone "$first" 30 || check "stuck: pid $first still there 30 s after the filesystem answered"
stuck_gone "$second" 30 || check "stuck: pid $second still there 30 s after the filesystem answered"
for _ in $(seq 30); do
  cozy --json machine show local > "$OUT/stuck-machine.json" 2> /dev/null || true
  J '(.status.stuck // []) | length == 0' "$OUT/stuck-machine.json" && break
  sleep 1
done
J '(.status.stuck // []) | length == 0' "$OUT/stuck-machine.json" || check "stuck: Status still lists exited TensorDs: $(jq -c .status.stuck "$OUT/stuck-machine.json")"
[ ! -e "$STUCK_ROOT/tensord/update/stuck.json" ] || check "stuck: stuck.json outlives them: $(cat "$STUCK_ROOT/tensord/update/stuck.json")"

stuck_silent "$trap"

step "stuck: the trap filesystem goes"
: > "$trap"
machine_stop || check "stuck: the machine did not stop at the end"
rm -f "$log"
cp "$fs/start.log" "$log" 2> /dev/null || : > "$log"
fusermount -u "$mnt" 2> /dev/null || fusermount -u -z "$mnt" || true
kill "$STUCK_FS" 2> /dev/null || true
stuck_ready 120 || check "stuck: the machine didn't start again off the trap filesystem"
}

# A start whose every write to its log is held: Calcifer's deadline (5 min) refuses it, naming why and the next command; it is refused
# at once while silent, placement takes no run there, and it serves once the filesystem answers.
stuck_silent() {
local trap=$1 t0 took code
step "stuck: a start that hangs is refused at Calcifer's deadline"
machine_stop || check "stuck: the machine did not stop"
echo writes > "$trap"
t0=$SECONDS code=0
timeout 420 cozy --json machine show local > "$OUT/stuck-silent.json" 2>&1 || code=$?
took=$((SECONDS - t0))
grep -q '"machine_not_answering"' "$OUT/stuck-silent.json" || check "stuck: a hung start (exit $code, ${took} s) isn't refused machine_not_answering: $(tail -c 400 "$OUT/stuck-silent.json")"
grep -q "This computer's TensorD isn't answering" "$OUT/stuck-silent.json" || check "stuck: the refusal doesn't say this computer's TensorD isn't answering"
grep -q 'cozy machine logs local' "$OUT/stuck-silent.json" || check "stuck: the refusal names no next command: $(tail -c 300 "$OUT/stuck-silent.json")"
[ "$took" -ge 290 ] && [ "$took" -le 350 ] || check "stuck: the hung start was refused after ${took} s (the deadline: 300 s)"
t0=$SECONDS
cozy --json machine show local > "$OUT/stuck-silent-again.json" 2>&1 || true
[ $((SECONDS - t0)) -le 10 ] || check "stuck: while silent, machine show local waited $((SECONDS - t0)) s"
grep -q '"machine_not_answering"' "$OUT/stuck-silent-again.json" || check "stuck: while silent, machine show local isn't refused: $(tail -c 300 "$OUT/stuck-silent-again.json")"
cozy --json machine ls > "$OUT/stuck-silent-ls.json" 2>&1 || true
J '.machines[]? | select(.name == "local") | (.unreachable // "") | contains("isn'"'"'t answering")' "$OUT/stuck-silent-ls.json" \
  || check "stuck: machine ls doesn't say local isn't answering: $(jq -c '.machines[]? | select(.name == "local") | {state, unreachable}' "$OUT/stuck-silent-ls.json")"
t0=$SECONDS
! stuck_run silent-run --on local || check "stuck: a run on a silent local completed"
grep -q "TensorD isn't answering" "$OUT/stuck-silent-run.ndjson" || check "stuck: a run on a silent local wasn't refused saying it isn't answering: $(tail -c 300 "$OUT/stuck-silent-run.ndjson")"
[ $((SECONDS - t0)) -le 30 ] || check "stuck: a run on a silent local took $((SECONDS - t0)) s to be refused"
echo "stuck: a hung start refused after ${took} s"
# A run with no --on is never placed back on the silent local: it waits for a machine (or is refused naming why), and goes there once
# local serves.
(cd "$RUN_PROJECT" && timeout 400 cozy --json run "$here/weightless/reverse" "text=any$RANDOM" > "$OUT/stuck-any.ndjson" 2> "$OUT/stuck-any.err"
  echo $? > "$OUT/stuck-any.code") &
local any=$! n
sleep 20
n=$(grep -oE '"number": *[0-9]+' "$OUT/stuck-any.ndjson" | head -1 | grep -oE '[0-9]+$' || true)
if [ -n "$n" ]; then
  cozy --json run watch "$n" --once > "$OUT/stuck-any.show.json" 2>&1 || true
  J '.machine != "local" or .status == "failed"' "$OUT/stuck-any.show.json" \
    || check "stuck: run $n (no --on) went to the silent local: $(jq -c '{machine, status, waiting}' "$OUT/stuck-any.show.json")"
  echo "stuck: run $n while local is silent: $(jq -c '{machine, status, waiting}' "$OUT/stuck-any.show.json")"
else
  grep -q "isn't answering" "$OUT/stuck-any.ndjson" "$OUT/stuck-any.err" || check "stuck: a run with no --on, refused, doesn't say local isn't answering: $(tail -c 300 "$OUT/stuck-any.err")"
fi
step "stuck: released, the silent start serves"
: > "$trap"
stuck_ready 90 || check "stuck: the machine didn't serve once its log answered: $(tail -c 300 "$OUT/stuck-machine.json")"
stuck_run after-silent --on local || check "stuck: no run went to the machine once it served: $(tail -c 300 "$OUT/stuck-after-silent.ndjson")"
for _ in $(seq 120); do kill -0 "$any" 2> /dev/null || break; sleep 1; done
kill "$any" 2> /dev/null || true
wait "$any" 2> /dev/null || true
echo "stuck: the run with no --on ended $(cat "$OUT/stuck-any.code" 2> /dev/null) once local served: $(tail -c 200 "$OUT/stuck-any.ndjson" | tr '\n' ' ')"
}
