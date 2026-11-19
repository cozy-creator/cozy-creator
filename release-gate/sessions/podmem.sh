# podmem <bound|use>: run on the pod. The tightest memory limit on an executor's cgroup path (else TensorD's, else this shell's), every level
# up to the controller's root: v2 memory.max and memory.high; v1 memory.limit_in_bytes under the memory controller's mount, whose root is the
# part of the membership path a container's mount hides; else MemTotal. It is what TensorD's `host_memory::read().bound` reads and what
# varena's host admission counts against.
# bound: "<cores> <GiB>". use: that limit, the cgroup holding it, its charge now and at its peak, the file cache in that charge (the
# admission's `held` is the charge less it) and the executors' summed RSS, bytes.
pid=$(pgrep -o -f 'cozy_runtime.internal.executor' 2> /dev/null); [ -n "$pid" ] || pid=$(pgrep -o -f 'cozy machine serve' 2> /dev/null); [ -n "$pid" ] || pid=self
best=$(awk '/^MemTotal/ {printf "%d", $2 * 1024}' /proc/meminfo) at=''
while IFS=: read -r _ ctrl path; do
  case "$ctrl" in
    "") base=/sys/fs/cgroup; limits="memory.max memory.high" ;;
    *memory*)
      read -r base root << EOF
$(awk '{for (i = 7; i <= NF && $i != "-"; i++); if ($(i + 1) == "cgroup" && ("," $(i + 3) ",") ~ /,memory,/) {print $5, $4; exit}}' "/proc/$pid/mountinfo" 2> /dev/null)
EOF
      base=${base:-/sys/fs/cgroup/memory} limits="memory.limit_in_bytes"
      [ "${root:-/}" = / ] || path=${path#"$root"} ;;
    *) continue ;;
  esac
  p=${path:-/}
  while :; do
    for f in $limits; do
      v=$(cat "$base$p/$f" 2> /dev/null)
      case "$v" in '' | max | 0) ;; *) [ "$v" -lt "$best" ] 2> /dev/null && best=$v at=$base$p ;; esac
    done
    [ "$p" = / ] && break
    p=$(dirname "$p")
  done
done < "/proc/$pid/cgroup"
if [ "$1" = bound ]; then echo "$(nproc) $((best >> 30))"; exit 0; fi
charge=- peak=- file=-
if [ -f "$at/memory.current" ]; then
  charge=$(cat "$at/memory.current"); peak=$(cat "$at/memory.peak" 2> /dev/null || echo -)
  file=$(awk '$1 == "inactive_file" || $1 == "active_file" {s += $2} END {printf "%d", s}' "$at/memory.stat" 2> /dev/null)
elif [ -f "$at/memory.usage_in_bytes" ]; then
  charge=$(cat "$at/memory.usage_in_bytes"); peak=$(cat "$at/memory.max_usage_in_bytes" 2> /dev/null || echo -)
  file=$(awk '$1 == "total_inactive_file" || $1 == "total_active_file" {s += $2} END {printf "%d", s}' "$at/memory.stat" 2> /dev/null)
fi
rss=$(ps -eo rss=,args= | awk '/cozy_runtime.internal.executor/ && !/awk/ {s += $1} END {printf "%d", s * 1024}')
echo "bound $best at ${at:-MemTotal} charge $charge peak $peak file ${file:--} executor_rss $rss"
