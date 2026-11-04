#!/bin/sh
# kill-at.sh <bytes> [machine], on a rental: TensorD killed as a crash kills it, once this machine's network has taken
# <bytes> more. With `machine`, the container instead, as a broken service ends it (minatsu's dead table): the service is
# killed, then twice more as soon as it is started again, and its supervisor stops after two exits before readiness.
# Signals from inside the container never reach PID 1, so a kill of the supervisor itself is dropped (gate 41932418438).
rx() { awk 'NR > 2 && $1 !~ /^lo:/ { sub(":", " "); s += $2 } END { printf "%.0f", s }' /proc/net/dev; }
want=$(( $(rx) + $1 ))
while [ "$(rx)" -lt "$want" ]; do sleep 0.05; done
ps -eo pid,ppid,args | grep 'machine serve'
service() {
  ps -eo pid=,ppid=,args= | awk '$3 ~ /cozy$/ && $4 == "machine" && $5 == "serve" { parent[$1] = $2 }
    END { for (p in parent) if (parent[p] in parent) print p }'
}
pid=$(service)
echo "killing $pid at $(date -u +%T.%N)"
kill -9 $pid
[ "$2" = machine ] || exit 0
for _ in 1 2; do
  while :; do new=$(service); [ -n "$new" ] && [ "$new" != "$pid" ] && break; sleep 0.02; done
  pid=$new; echo "killing its restart $pid at $(date -u +%T.%N)"; kill -9 $pid
done
