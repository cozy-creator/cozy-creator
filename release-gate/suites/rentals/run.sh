#!/bin/bash
# By hand, in the default home: release-gate/suites/rentals/run.sh <out dir> [--tensord BIN] [idle|stop]...
# The idle cell rents its own `cpu` (any provider) and puts BIN on it first; the stop cell rents a vast `cpu`.
set -uo pipefail
OUT=$1; shift; TD= CELLS=()
while [ $# -gt 0 ]; do case $1 in --tensord) TD=$2; shift;; *) CELLS+=("$1");; esac; shift; done
[ ${#CELLS[@]} -gt 0 ] || CELLS=(stop idle)
CLI=${CLI:-cozy} HERE=$(cd "$(dirname "$0")/../.." && pwd); mkdir -p "$OUT" && cd "$OUT" || exit 1
log() { echo "$(date -u +%H:%M:%SZ) $*" | tee -a gate.log; }
expect() { printf '%s\t%s\t%s\n' "$1" "$2" "$3" >> expect.tsv; log "$1: $2 $3"; }
. "$HERE/suites/rentals/lib.sh"
for c in "${CELLS[@]}"; do
  case $c in
    stop) rental_stop_closes vast ;;
    idle)
      "$CLI" --json machine rent cpu -y >idle-rent.out 2>&1
      r=$(grep -m1 '"type":"rental.accepted"' idle-rent.out | jq -r '.rental_id // empty') m=$(grep -o '"machine":{"name":"[^"]*"' idle-rent.out | head -1 | sed 's/.*"name":"//; s/"$//')
      [ -n "$m" ] || { expect rental-idle-through-updates FAIL "no cpu rental: $(tail -c 300 idle-rent.out)"; [ -z "$r" ] || "$CLI" machine end "$r" -y; continue; }
      [ -z "$TD" ] || "$CLI" machine update "$m" --tensord "$TD" -y >idle-tensord.out 2>&1 || log "tensord update: $(tail -c 300 idle-tensord.out)"
      rental_idle_through_updates "$m" "$r" ;;
  esac
done
cat expect.tsv
