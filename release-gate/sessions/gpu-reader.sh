# gpu-reader: sourced by a session once it has a pod ($NAME, $OUT): starts the pod's own GPU reader and defines
# gpuwatch (start it again after an update), gpu (its last reading), readings (its whole log into gpu-pod.log) and
# before_end (lib.sh's finish fetches the log from a live pod).
# The pod reads its own cards every 5 s into /var/tmp/ab-gpu.log: each compute process's pid, MiB and command, then each
# card's used and free MiB. ssh to some pods answers nothing for minutes (43733785568: empty readings, then
# machine_ssh_unheard; each of those cells waited out a 60 s timeout first), so the pod keeps the readings and the session
# fetches them whole, in the background each 5 min and before the pod ends (gpu-pod.log).
# A pid nvidia-smi names in the host's namespace has no /proc entry in the pod: its hwm is then the largest of an executor's.
WATCH='[ -s /var/tmp/ab-gpu.pid ] && kill -0 "$(cat /var/tmp/ab-gpu.pid)" 2> /dev/null && { echo running; exit 0; }
cat > /var/tmp/ab-gpu.sh << "EOS"
while :; do
  t=$(date -u +%T)
  x=0
  for d in /proc/[0-9]*; do
    case "$(tr "\0" " " 2> /dev/null < "$d/cmdline")" in
      *cozy_runtime.internal.executor*) h=$(awk "/^VmHWM/ {print \$2}" "$d/status" 2> /dev/null); [ "${h:-0}" -gt "$x" ] && x=$h ;;
    esac
  done
  nvidia-smi --query-compute-apps=pid,used_memory --format=csv,noheader,nounits | while IFS=", " read -r pid mib; do
    h=$(awk "/^VmHWM/ {print \$2}" "/proc/$pid/status" 2> /dev/null)
    echo "$t pid $pid $mib MiB hwm ${h:-$x} $(tr "\0" " " 2> /dev/null < "/proc/$pid/cmdline" | cut -c1-200)"
  done
  echo "$t card $(nvidia-smi --query-gpu=index,memory.used,memory.free --format=csv,noheader,nounits | paste -sd";")"
  sleep 5
done
EOS
setsid nohup sh /var/tmp/ab-gpu.sh >> /var/tmp/ab-gpu.log 2>&1 < /dev/null &
echo $! > /var/tmp/ab-gpu.pid; echo started'
# watch: start the pod's reader unless it runs (after the rental and each update: a restart may end it).
gpuwatch() {
  local try
  for try in 1 2 3 4 5 6; do
    timeout 60 cozy machine ssh "$NAME" "echo $(base64 -w0 <<< "$WATCH") | base64 -d | sh" 2>&1 | grep -qE '^(started|running)$' && return 0
    sleep 10
  done
  say "the pod's GPU reader did not start: readings only where ssh answers"
}
# gpu: the pod's last reading (one ssh; empty when it doesn't answer).
gpu() { timeout 20 cozy machine ssh "$NAME" 't=$(tail -1 /var/tmp/ab-gpu.log | cut -d" " -f1); grep "^$t " /var/tmp/ab-gpu.log | cut -d" " -f2-' 2>&1 | grep -v '^Warning: Permanently added'; }
# readings [tries]: the pod's whole log into gpu-pod.log, kept when the fetch returned it.
readings() {
  local try
  for try in $(seq "${1:-1}"); do
    timeout 120 cozy machine ssh "$NAME" "cat /var/tmp/ab-gpu.log" 2> /dev/null | grep -E '^[0-9:]{8} (pid|card) ' > "$OUT/gpu-pod.tmp"
    [ -s "$OUT/gpu-pod.tmp" ] && { mv "$OUT/gpu-pod.tmp" "$OUT/gpu-pod.log"; return 0; }
    [ "$try" = "${1:-1}" ] || sleep 15
  done
  return 1
}
before_end() { readings 8 || say "the pod's GPU readings were not fetched before it ended"; }
gpuwatch
