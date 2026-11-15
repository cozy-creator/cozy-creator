# e2e/cli/stuck.sh: TensorD's stop, restart, update and start finish within their bounds when a TensorD can't exit (cl-977), on this
# computer's machine with no GPU (Linux). A service that won't exit (stopped) is killed 10 s after its stop. A service whose exit sits
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
# A thread of <pid> in the kernel (D).
stuck_in_d() { grep -lqE '^[0-9]+ \(.*\) D ' /proc/"$1"/task/*/stat 2> /dev/null; }
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
stuck_ready() {
  for _ in $(seq "$1"); do
    cozy --json machine show local > "$OUT/stuck-machine.json" 2> "$OUT/stuck-machine.err" || true
    J '.state == "ready" and (.unreachable // "") == ""' "$OUT/stuck-machine.json" && return 0
    sleep 1
  done
  return 1
}

stuck_stop() {
[ -n "$LOCAL" ] || return 0
step "stuck: a stop with a service that won't exit ends 10 s after it (cl-977)"
local parent service t0 took
stuck_ready 180 || { check "stuck: the local machine isn't ready: $(tail -c 300 "$OUT/stuck-machine.json")"; return; }
parent=$(machine_pid) service=$(stuck_service)
[ -n "$service" ] || { check "stuck: no service under the machine's pid $parent"; return; }
kill -STOP "$service"
t0=$SECONDS
kill "$parent"
stuck_gone "$parent" 60 || check "stuck: the machine (pid $parent) runs 60 s after its stop, its service stopped: $(ps -o pid,stat,args -p "$parent","$service")"
took=$((SECONDS - t0))
[ "$took" -le 20 ] || check "stuck: a stop with a service that won't exit took ${took} s (its bound: 10 s)"
stuck_log | grep -q "the service (pid $service) did not exit within 10 s; killing it" || check "stuck: TensorD's log doesn't say it killed pid $service"
echo "stuck: stopped in ${took} s with service $service stopped"
stuck_ready 120 || check "stuck: the machine didn't start again after its stop: $(tail -c 300 "$OUT/stuck-machine.json")"
}

stuck_exit() {
[ -n "$LOCAL" ] || return 0
step "stuck: a TensorD whose exit sits in the kernel (cl-977)"
local fs=$OUT/stuck-fs mnt=$RUNNER_TEMP/stuck-mnt trap=$OUT/stuck-trap log=$STUCK_ROOT/machine/start.log
local service parent t0 took port first second
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

step "stuck: an update's restart past a service that can't exit"
first=$(stuck_service)
echo "$first" > "$trap"
t0=$SECONDS
cozy --json machine update local --gpu-memory-limit off -y > "$OUT/stuck-update.json" 2>&1 || check "stuck: the update failed: $(tail -c 300 "$OUT/stuck-update.json")"
took=$((SECONDS - t0))
grep -q "holding a flush" "$OUT/trapfs.log" || check "stuck: the filesystem held no flush from pid $first"
stuck_in_d "$first" || check "stuck: pid $first, the service before, isn't in the kernel: $(ps -o pid,stat,wchan:30 -p "$first")"
# 10 s to exit, 60 s with no progress, then the next service's boot.
[ "$took" -le 200 ] || check "stuck: the update took ${took} s past a service that can't exit"
stuck_log | grep -q "pid $first can't exit: thread .* is in the kernel" || check "stuck: TensorD's log doesn't say pid $first can't exit"
stuck_ready 30 || check "stuck: the machine isn't ready after its update: $(tail -c 300 "$OUT/stuck-machine.json")"
J --argjson pid "$first" 'any(.status.stuck[]?; .pid == $pid and .command == "the TensorD before this one")' "$OUT/stuck-machine.json" \
  || check "stuck: machine show local lists no stuck pid $first: $(jq -c .status.stuck "$OUT/stuck-machine.json")"
cozy machine show local > "$OUT/stuck-show.txt" 2>&1 || true
grep -qE "pid $first \(the TensorD before this one\), killed .* stuck in the kernel" "$OUT/stuck-show.txt" \
  || check "stuck: machine show local doesn't say pid $first is stuck: $(grep -i stuck "$OUT/stuck-show.txt")"
cozy machine ls > "$OUT/stuck-ls.txt" 2>&1 || true
grep -qE '^local .*1 stuck' "$OUT/stuck-ls.txt" || check "stuck: machine ls doesn't mark local stuck: $(grep '^local' "$OUT/stuck-ls.txt")"
J '.broken == null' "$OUT/stuck-machine.json" || check "stuck: a machine with no GPU is broken: $(jq -c .broken "$OUT/stuck-machine.json")"
(cd "$here" && cell stuck-after-update ./weightless/reverse "text=stuck$RANDOM") || check "stuck: no run went to the machine after its update"
echo "stuck: updated in ${took} s past pid $first, which waits in the kernel"

step "stuck: a stop and a start beside a TensorD that can't exit"
parent=$(machine_pid)
kill "$parent"
stuck_gone "$parent" 30 || check "stuck: the machine (pid $parent) runs 30 s after its stop"
t0=$SECONDS
stuck_ready 120 || check "stuck: the machine didn't start beside pid $first: $(tail -c 300 "$OUT/stuck-machine.json")"
took=$((SECONDS - t0))
[ "$(cat "$STUCK_ROOT/machine/api-port")" = "$port" ] || check "stuck: the machine left its port $port to the stuck pid $first: now $(cat "$STUCK_ROOT/machine/api-port")"
J --argjson pid "$first" 'any(.status.stuck[]?; .pid == $pid)' "$OUT/stuck-machine.json" || check "stuck: after its start, pid $first isn't listed"
(cd "$here" && cell stuck-after-start ./weightless/reverse "text=stuck$RANDOM") || check "stuck: no run went to the machine started beside pid $first"
echo "stuck: started in ${took} s beside pid $first, on port $port"

step "stuck: a stop whose service can't exit"
second=$(stuck_service) parent=$(machine_pid)
echo "$second" > "$trap"
t0=$SECONDS
kill "$parent"
stuck_gone "$parent" 150 || check "stuck: the machine (pid $parent) runs 150 s after its stop, its service stuck: $(ps -o pid,stat,args -p "$parent")"
took=$((SECONDS - t0))
# 10 s to exit, 60 s with no progress.
[ "$took" -le 90 ] || check "stuck: a stop with a service that can't exit took ${took} s"
stuck_in_d "$second" || check "stuck: pid $second isn't in the kernel after the stop"
stuck_ready 120 || check "stuck: the machine didn't start beside pids $first and $second: $(tail -c 300 "$OUT/stuck-machine.json")"
J '[.status.stuck[]? | select(.command == "the TensorD before this one")] | length == 2' "$OUT/stuck-machine.json" \
  || check "stuck: machine show local doesn't list both stuck TensorDs: $(jq -c .status.stuck "$OUT/stuck-machine.json")"
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

# A start whose every write to its log is held: Calcifer's deadline (3 min) refuses it, naming why and the next command; it is refused
# at once while silent, placement takes no run there, and it serves once the filesystem answers.
stuck_silent() {
local trap=$1 t0 took code
step "stuck: a start that hangs is refused at Calcifer's deadline"
machine_stop || check "stuck: the machine did not stop"
echo writes > "$trap"
t0=$SECONDS code=0
timeout 300 cozy --json machine show local > "$OUT/stuck-silent.json" 2>&1 || code=$?
took=$((SECONDS - t0))
grep -q '"machine_not_answering"' "$OUT/stuck-silent.json" || check "stuck: a hung start (exit $code, ${took} s) isn't refused machine_not_answering: $(tail -c 400 "$OUT/stuck-silent.json")"
grep -q "local's TensorD isn't answering" "$OUT/stuck-silent.json" || check "stuck: the refusal doesn't say local's TensorD isn't answering"
grep -q 'cozy machine logs local' "$OUT/stuck-silent.json" || check "stuck: the refusal names no next command: $(tail -c 300 "$OUT/stuck-silent.json")"
[ "$took" -ge 170 ] && [ "$took" -le 230 ] || check "stuck: the hung start was refused after ${took} s (the deadline: 180 s)"
t0=$SECONDS
cozy --json machine show local > "$OUT/stuck-silent-again.json" 2>&1 || true
[ $((SECONDS - t0)) -le 10 ] || check "stuck: while silent, machine show local waited $((SECONDS - t0)) s"
grep -q '"machine_not_answering"' "$OUT/stuck-silent-again.json" || check "stuck: while silent, machine show local isn't refused: $(tail -c 300 "$OUT/stuck-silent-again.json")"
cozy --json machine ls > "$OUT/stuck-silent-ls.json" 2>&1 || true
J '.machines[]? | select(.name == "local") | (.unreachable // "") | contains("isn'"'"'t answering")' "$OUT/stuck-silent-ls.json" \
  || check "stuck: machine ls doesn't say local isn't answering: $(jq -c '.machines[]? | select(.name == "local") | {state, unreachable}' "$OUT/stuck-silent-ls.json")"
t0=$SECONDS
(cd "$here" && ALLOW="TensorD isn't answering" cell stuck-silent-run ./weightless/reverse "text=silent$RANDOM")
[ -f "$OUT/stuck-silent-run.refused" ] || check "stuck: a run on a silent local wasn't refused saying it isn't answering"
[ $((SECONDS - t0)) -le 30 ] || check "stuck: a run on a silent local took $((SECONDS - t0)) s to be refused"
echo "stuck: a hung start refused after ${took} s"
step "stuck: released, the silent start serves"
: > "$trap"
stuck_ready 90 || check "stuck: the machine didn't serve once its log answered: $(tail -c 300 "$OUT/stuck-machine.json")"
(cd "$here" && cell stuck-after-silent ./weightless/reverse "text=late$RANDOM") || check "stuck: no run went to the machine once it served"
}
