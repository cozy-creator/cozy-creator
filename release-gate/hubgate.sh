#!/bin/bash
# hubgate.sh --suite hub|skew [--hub URL] [--on MACHINE] [--out DIR].
# hub: read-only production contracts plus real provider qualification, with no synthetic publication.
# skew: an already-running isolated fixture; normally use e2e/skew/run.sh to create and clean it.
set -uo pipefail
HERE=$(cd "$(dirname "$0")" && pwd)
SUITE= HUB_URL=${HUB_URL:-} ON= OUT=
while [ $# -gt 0 ]; do
  case $1 in
    --suite) SUITE=$2;; --swap) echo "--swap is retired; use e2e/skew/run.sh --hub-src DIR --cozy-bin FILE" >&2; exit 2;; --hub) HUB_URL=$2;; --on) ON=$2;; --out) OUT=$2;;
    *) echo "unknown argument $1" >&2; exit 2;;
  esac; shift 2
done
[ -f "$HERE/suites/$SUITE/cells.sh" ] || { echo "hubgate.sh needs --suite skew or hub" >&2; exit 2; }
OUT=$(realpath -m "${OUT:-$PWD/hubgate-$SUITE-$(date -u +%Y%m%dT%H%MZ)}"); mkdir -p "$OUT"; cd "$OUT" || exit 2
: > expect.tsv
log() { echo "$(date -u +%H:%M:%SZ) $*" | tee -a gate.log; }
expect() { printf '%s\t%s\t%s\n' "$1" "$2" "$(tr '\n\t' '  ' <<< "$3" | cut -c1-400)" >> expect.tsv; log "$1: $2 $3"; }
end_it() { :; }  # the suites end their own rentals
CLI=cozy M=${ON:-local} R=$(( $(date +%s%N) / 1000 % 4000000000 ))
. "$HERE/../e2e/run.sh"

# Synthetic suites require the ephemeral runner, not a swapped persistent developer Hub.
if [ "$SUITE" = skew ]; then
  . "$HERE/../e2e/test-hub.sh"
  require_test_hub "${HUB_URL:-}" || exit 2
fi
HUB_URL=${HUB_URL:-https://tensorhub.com}
log "hubgate $SUITE: Hub $HUB_URL, cozy $(command cozy --version), on $M, seeds from $R"
run_project "$OUT/project" || { log "cozy project new failed"; exit 1; }
( . "$HERE/suites/$SUITE/cells.sh" )
fails=$(grep -c $'\tFAIL\t' expect.tsv) passes=$(grep -c $'\tPASS\t' expect.tsv)
grep $'\tFAIL\t' expect.tsv | cut -f1,3 | sed 's/^/FAIL /' | tee verdict.txt
if [ "$fails" = 0 ] && [ "$passes" -gt 0 ]; then echo "VERDICT PASS ($passes cells)" | tee -a verdict.txt
else echo "VERDICT FAIL ($fails of $((fails + passes)) cells)" | tee -a verdict.txt; exit 1; fi
