#!/bin/sh
# kill-at.sh <bytes>, on a rental: TensorD killed as a crash kills it, once this machine's network has taken <bytes> more.
# Decided here, so the kill lands mid-download however fast the machine is: one sent over SSH takes seconds, and gate
# 41822763629's landed 74 ms after its 2 GiB had finished, with nothing cut.
# Its target is the service: the `cozy machine serve` whose parent is one too, which that parent starts again. The parent
# isn't always PID 1, and killing it ends the machine.
rx() { awk 'NR > 2 && $1 !~ /^lo:/ { sub(":", " "); s += $2 } END { printf "%.0f", s }' /proc/net/dev; }
want=$(( $(rx) + $1 ))
while [ "$(rx)" -lt "$want" ]; do sleep 0.05; done
ps -eo pid,ppid,args | grep 'machine serve'
pid=$(ps -eo pid=,ppid=,args= | awk '$3 ~ /cozy$/ && $4 == "machine" && $5 == "serve" { parent[$1] = $2 }
  END { for (p in parent) if (parent[p] in parent) print p }')
echo "killing $pid at $(date -u +%T.%N)"
kill -9 $pid
