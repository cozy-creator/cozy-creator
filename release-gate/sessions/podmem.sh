# podmem <bound|use>: run on the pod. bound: "<cores> <GiB>", the tightest memory limit on an executor's cgroup path (else TensorD's; v2
# memory.max and memory.high, v1 memory.limit_in_bytes) or MemTotal, as TensorD's host_memory bound reads it.
# use: the container's peak and current charge (v1 max_usage/usage, v2 peak/current) and the executors' summed RSS, bytes.
# An executor's path holds TensorD's limits and any its own scope adds.
pid=$(pgrep -o -f 'cozy_runtime.internal.executor' 2> /dev/null); [ -n "$pid" ] || pid=$(pgrep -o -f 'tensord' 2> /dev/null); [ -n "$pid" ] || pid=self
best=$(awk '/^MemTotal/ {printf "%d", $2 * 1024}' /proc/meminfo)
peak=- now=- v1peak=- v1now=-
while IFS=: read -r _ ctrl path; do
  case "$ctrl" in
    "") base=/sys/fs/cgroup; limits="memory.max memory.high" ;;
    *memory*) base=/sys/fs/cgroup/memory; limits="memory.limit_in_bytes" ;;
    *) continue ;;
  esac
  p=${path:-/}
  [ -z "$ctrl" ] && { peak=$(cat "$base$p/memory.peak" 2> /dev/null || echo -); now=$(cat "$base$p/memory.current" 2> /dev/null || echo -); }
  [ -n "$ctrl" ] && { v1peak=$(cat "$base$p/memory.max_usage_in_bytes" 2> /dev/null || echo -); v1now=$(cat "$base$p/memory.usage_in_bytes" 2> /dev/null || echo -); }
  while :; do
    for f in $limits; do
      v=$(cat "$base$p/$f" 2> /dev/null)
      case "$v" in '' | max) ;; *) [ "$v" -lt "$best" ] 2> /dev/null && best=$v ;; esac
    done
    [ "$p" = / ] && break
    p=$(dirname "$p")
  done
done < "/proc/$pid/cgroup"
if [ "$1" = bound ]; then echo "$(nproc) $((best >> 30))"; exit 0; fi
rss=$(ps -eo rss=,args= | awk '/cozy_runtime.internal.executor/ && !/awk/ {s += $1} END {printf "%d", s * 1024}')
echo "bound $best v1_peak $v1peak v1_now $v1now v2_peak $peak v2_now $now executor_rss $rss"
