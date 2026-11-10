#!/bin/sh
# Suite bench's pod side, run as root over `cozy machine ssh` (cells.sh copies it, hog.py and vram.py to /var/tmp):
#   hold <bytes>   leave the pod <bytes> of memory: hold the rest of its limit (MemTotal, or a lower cgroup v2 or v1 limit,
#                  at the cgroup root or this process's own cgroup) in a /dev/shm file when the pod cannot swap and there is
#                  room (no OOM pick, never paged), else in touched anonymous memory of hog.py, mlocked when the pod can
#                  swap (a refused lock leaves it unlocked: `check` then sees it paged out); start the MemAvailable log;
#                  print the facts. Exit 1, saying why, when the hog does not stand.
#   check          the bytes held now, and what hold was asked to hold
#   gpu            GPU 0's name, driver, PCIe link and power limit, one key=value a line (bench.py: the pod's facts)
#   vram <bytes|off>  GPU 0 as a card of <bytes> for every other process (vram.py): the previous holder ended first;
#                  prints '<pid> <held> <free> <total> <others>'; off ends it
#   release        drop the hog and the VRAM holder; stop the log
set -u
limit() {  # the lowest memory limit this process sees, in bytes; MemTotal when none is lower
  own=$(sed -n 's/^0::\(.*\)/\1/p' /proc/self/cgroup 2>/dev/null)
  v1=$(sed -n 's/^[0-9]*:\([^:]*,\)*memory\(,[^:]*\)*:\(.*\)/\3/p' /proc/self/cgroup 2>/dev/null)
  total=$(awk '/^MemTotal:/{print $2 * 1024}' /proc/meminfo)
  cat /sys/fs/cgroup/memory.max "/sys/fs/cgroup${own:-/-}/memory.max" /sys/fs/cgroup/memory/memory.limit_in_bytes \
    "/sys/fs/cgroup/memory${v1:-/-}/memory.limit_in_bytes" 2>/dev/null | grep -E '^[0-9]+$' |
    awk -v t="$total" 'BEGIN {m = t} $1 < m {m = $1} END {printf "%d\n", m}'
}
swaps() {  # 1 when this pod can page out: swap on the host and none of its cgroup limits forbids it
  [ "$(awk '/^SwapTotal:/{print $2}' /proc/meminfo)" != 0 ] || { echo 0; return; }
  own=$(sed -n 's/^0::\(.*\)/\1/p' /proc/self/cgroup 2>/dev/null)
  for f in /sys/fs/cgroup/memory.swap.max "/sys/fs/cgroup${own:-/-}/memory.swap.max"; do
    [ "$(cat "$f" 2>/dev/null)" != 0 ] || { echo 0; return; }
  done
  echo 1
}
inuse() {  # what this pod's cgroup holds that it cannot reclaim without swap: anonymous memory and shmem (v2, else v1)
  own=$(sed -n 's/^0::\(.*\)/\1/p' /proc/self/cgroup 2>/dev/null)
  for f in "/sys/fs/cgroup${own:-/-}/memory.stat" /sys/fs/cgroup/memory.stat /sys/fs/cgroup/memory/memory.stat; do
    [ -r "$f" ] && awk '$1 == "anon" || $1 == "shmem" || $1 == "total_rss" || $1 == "total_shmem" {n += $2} END {printf "%d\n", n}' "$f" && return
  done
  echo 0
}
room() {  # what a hog may take: MemAvailable, or the cgroup limit less what it already holds, whichever is less
  avail=$(awk '/^MemAvailable:/{print $2 * 1024}' /proc/meminfo)
  echo $(( $(limit) - $(inuse) < avail ? $(limit) - $(inuse) : avail ))
}
pressure() {  # memory pressure, the share of the last 10 s some task waited on memory (0 where the kernel does not say)
  awk -F'[ =]' '/^some/ {printf "%d\n", $3; found = 1} END {if (!found) print 0}' /proc/pressure/memory 2>/dev/null || echo 0
}
facts() {
  grep -E '^(MemTotal|MemAvailable|SwapTotal|Shmem):' /proc/meminfo
  echo "limit $(limit) inuse $(inuse) room $(room) swaps $(swaps) pressure $(pressure)"
  echo "shm $(df -B1 --output=avail /dev/shm 2>/dev/null | tail -1)"
}
case $1 in
  hold)
    echo "before: $(facts | tr '\n' ' ')"  # first, so a pod that stalls still says what it was
    if [ -s /var/tmp/bench-hog.want ] && kill -0 "$(cat /var/tmp/memavail.pid 2>/dev/null)" 2>/dev/null; then
      read -r mode held < /var/tmp/bench-hog.want  # a previous gate's hold on this pod stands: kept, not stacked
      echo "hog $mode $held (kept)"; facts; exit 0
    fi
    swap=$(swaps)
    shm=$(df -B1 --output=avail /dev/shm 2>/dev/null | tail -1)
    hog=$(( ($(room) - $2) / 2147483648 * 2147483648 ))
    mode=none held=0 short=
    if [ "$hog" -gt 2147483648 ]; then
      if [ "$swap" = 0 ] && [ -n "$shm" ] && [ "$shm" -ge $((hog + 1073741824)) ]; then
        # 2 GiB at a time: a step that stalls or raises memory pressure is a limit this pod hides; it backs off a step.
        mode=shm; : > /dev/shm/cozy-bench-hog
        while [ "$held" -lt "$hog" ]; do
          t0=$(date +%s)
          dd if=/dev/zero of=/dev/shm/cozy-bench-hog bs=64M count=32 oflag=append conv=notrunc status=none || { short="dd failed"; break; }
          held=$((held + 2147483648))
          if [ $(($(date +%s) - t0)) -gt 20 ] || [ "$(pressure)" -gt 10 ]; then
            short="a 2 GiB step stalled or raised memory pressure"; held=$((held > 4294967296 ? held - 4294967296 : 0))
            truncate -s "$held" /dev/shm/cozy-bench-hog; break
          fi
        done
      else
        mode=anon; rm -f /var/tmp/bench-hog.ready
        setsid nohup python3 /var/tmp/bench-hog.py "$hog" "$swap" > /var/tmp/bench-hog.out 2>&1 < /dev/null &
        pid=$!
        i=0; while [ ! -s /var/tmp/bench-hog.ready ] && kill -0 "$pid" 2>/dev/null && [ $i -lt 300 ]; do sleep 2; i=$((i + 1)); done
        [ -s /var/tmp/bench-hog.ready ] || { kill "$pid" 2>/dev/null; echo "hog $hog did not stand: $(tail -c 300 /var/tmp/bench-hog.out)"; facts; exit 1; }
        read -r _ state held short < /var/tmp/bench-hog.ready
        mode=anon-$state
      fi
    fi
    echo "$mode $held" > /var/tmp/bench-hog.want
    setsid nohup sh -c 'while :; do echo "$(date +%s) $(awk "/^MemAvailable:/{print \$2}" /proc/meminfo)"; sleep 5; done' \
      > /var/tmp/memavail.log 2>&1 < /dev/null &
    echo $! > /var/tmp/memavail.pid
    [ -z "$short" ] || echo "SHORT of $hog: $short"
    echo "hog $mode $held"; facts ;;
  check)
    read -r mode want < /var/tmp/bench-hog.want
    case $mode in
      shm) held=$(stat -c %s /dev/shm/cozy-bench-hog 2>/dev/null || echo 0) ;;
      anon*) held=$(awk '/^VmRSS:/{print $2 * 1024}' "/proc/$(cut -d' ' -f1 /var/tmp/bench-hog.ready)/status" 2>/dev/null || echo 0) ;;
      *) held=0 ;;
    esac
    echo "$held $want" ;;
  gpu)
    nvidia-smi -i 0 --query-gpu=name,driver_version,pcie.link.gen.max,pcie.link.width.max,power.limit --format=csv,noheader,nounits |
      awk -F', *' '{print "gpu=" $1; print "driver=" $2; print "pcie=gen" $3 " x" $4; print "power_limit_w=" $5}' ;;
  vram)
    if [ -s /var/tmp/bench-vram.ready ]; then
      pid=$(cut -d' ' -f1 /var/tmp/bench-vram.ready); kill "$pid" 2>/dev/null
      i=0; while kill -0 "$pid" 2>/dev/null && [ $i -lt 100 ]; do sleep 0.2; i=$((i + 1)); done
    fi
    rm -f /var/tmp/bench-vram.ready
    [ "$2" != off ] || { echo off; exit 0; }
    setsid nohup python3 /var/tmp/bench-vram.py "$2" /var/tmp/bench-vram.ready > /var/tmp/bench-vram.out 2>&1 < /dev/null &
    i=0; while [ ! -s /var/tmp/bench-vram.ready ] && kill -0 $! 2>/dev/null && [ $i -lt 240 ]; do sleep 0.5; i=$((i + 1)); done
    [ -s /var/tmp/bench-vram.ready ] || { echo "no holder: $(tail -c 300 /var/tmp/bench-vram.out)"; exit 1; }
    cat /var/tmp/bench-vram.ready; echo ;;
  release)
    sh "$0" vram off > /dev/null
    rm -f /dev/shm/cozy-bench-hog
    [ ! -s /var/tmp/bench-hog.ready ] || kill "$(cut -d' ' -f1 /var/tmp/bench-hog.ready)" 2>/dev/null
    [ ! -s /var/tmp/memavail.pid ] || kill "$(cat /var/tmp/memavail.pid)" 2>/dev/null
    echo released ;;
esac
