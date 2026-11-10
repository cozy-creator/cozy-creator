#!/bin/sh
# Suite bench's host RAM, run on the pod as root over `cozy machine ssh` (cells.sh copies it and hog.py to /var/tmp):
#   hold <bytes>   leave the pod <bytes> of memory: hold the rest of its limit (MemTotal, or a lower cgroup v2 or v1 limit) in
#                  a /dev/shm file when there is no swap and room (pinned; no OOM pick), else in touched anonymous memory of
#                  hog.py at oom_score_adj -1000, mlocked when there is swap; start the MemAvailable log; print the facts
#   check          the bytes held now, and what hold was asked to hold
#   release        drop the hog; stop the log
set -u
facts() {
  grep -E '^(MemTotal|MemAvailable|SwapTotal|Shmem):' /proc/meminfo
  echo "limit $(cat /sys/fs/cgroup/memory.max /sys/fs/cgroup/memory/memory.limit_in_bytes 2>/dev/null | grep -m1 -E '^[0-9]+$')"
  echo "shm $(df -B1 --output=avail /dev/shm 2>/dev/null | tail -1)"
}
case $1 in
  hold)
    total=$(awk '/^MemTotal:/{print $2 * 1024}' /proc/meminfo)
    limit=$(facts | awk '$1 == "limit" && $2 ~ /^[0-9]+$/ {print $2}')
    [ -n "$limit" ] && [ "$limit" -lt "$total" ] || limit=$total
    swap=$(awk '/^SwapTotal:/{print $2}' /proc/meminfo)
    shm=$(df -B1 --output=avail /dev/shm 2>/dev/null | tail -1)
    hog=$(( (limit - $2) / 67108864 * 67108864 ))
    mode=none
    if [ "$hog" -gt 1073741824 ]; then
      if [ "$swap" = 0 ] && [ -n "$shm" ] && [ "$shm" -ge $((hog + 1073741824)) ]; then
        mode=shm; dd if=/dev/zero of=/dev/shm/cozy-bench-hog bs=64M count=$((hog / 67108864)) status=none
      else
        mode=anon; rm -f /var/tmp/bench-hog.ready
        lock=0; [ "$swap" = 0 ] || lock=1
        setsid nohup python3 /var/tmp/bench-hog.py "$hog" "$lock" > /var/tmp/bench-hog.out 2>&1 < /dev/null &
        i=0; while [ ! -s /var/tmp/bench-hog.ready ] && [ $i -lt 300 ]; do sleep 2; i=$((i + 1)); done
      fi
    else
      hog=0
    fi
    echo "$mode $hog" > /var/tmp/bench-hog.want
    setsid nohup sh -c 'while :; do echo "$(date +%s) $(awk "/^MemAvailable:/{print \$2}" /proc/meminfo)"; sleep 5; done' \
      > /var/tmp/memavail.log 2>&1 < /dev/null &
    echo $! > /var/tmp/memavail.pid
    echo "hog $mode $hog"; facts ;;
  check)
    read -r mode want < /var/tmp/bench-hog.want
    case $mode in
      shm) held=$(stat -c %s /dev/shm/cozy-bench-hog 2>/dev/null || echo 0) ;;
      anon) held=$(awk '/^VmRSS:/{print $2 * 1024}' "/proc/$(cut -d' ' -f1 /var/tmp/bench-hog.ready)/status" 2>/dev/null || echo 0) ;;
      *) held=0 ;;
    esac
    echo "$held $want" ;;
  release)
    rm -f /dev/shm/cozy-bench-hog
    [ ! -s /var/tmp/bench-hog.ready ] || kill "$(cut -d' ' -f1 /var/tmp/bench-hog.ready)" 2>/dev/null
    [ ! -s /var/tmp/memavail.pid ] || kill "$(cat /var/tmp/memavail.pid)" 2>/dev/null
    echo released ;;
esac
