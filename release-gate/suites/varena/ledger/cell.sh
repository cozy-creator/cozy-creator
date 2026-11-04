#!/usr/bin/env bash
# cell.sh <machine> <out dir>: the ledger on a real GPU, through `cozy`, on a pod rented with --ssh.
#   trade   SDXL and Qwen-Image alternate on one card: two executors, trading memory, none restarted
#   kill    a donor killed the moment a peer asks: the asking run carries on
#   foreign a stand-in program (foreign.py, a plain process) ramps over an idle executor: dedicated
#           refuses it, shared gives way, and each block's wait is the response time
#   binary  the machine's cozy is one file: ldd, and it runs from an empty directory
# Writes each run's record and the table between runs to <out dir>. Every request differs in prompt and seed.
set -uo pipefail
M=$1 D=$2 HERE=$(cd "$(dirname "$0")" && pwd) R=$RANDOM
mkdir -p "$D" && cd "$D" || exit 1
[ -d project ] || cozy project new project >/dev/null || exit 1
cd project || exit 1
log() { echo "$(date -u +%T) $*" | tee -a "$D/cell.log"; }
pod() { cozy machine ssh "$M" "$@"; }
table() { pod python3 - <"$HERE/table.py" 2>&1 | tee -a "$D/cell.log"; }
run() { # run <name> <target> [input...]: follow to its end; the run's number and status go to the log
  local name=$1 target=$2 n status; shift 2
  cozy run "$target" --on "$M" --json "$@" >"$D/$name.ndjson" 2>&1
  n=$(grep -o '"number": *[0-9]*' "$D/$name.ndjson" | head -1 | grep -o '[0-9]*$')
  cozy run show "$n" --json >"$D/$name.json" 2>&1
  status=$(jq -r .status "$D/$name.json" 2>/dev/null)
  log "$name: run $n $status"
  [ "$status" = completed ] || log "$name: $(jq -c '.outcome.reason // .error // .' "$D/$name.json" 2>/dev/null | cut -c1-400)"
  echo "$n" >"$D/$name.number"
}
sdxl() { run "$1" sdxl/generate prompt="$2" seed:=$((R + $3)); }
qwen() { run "$1" qwen-image-2/generate prompt="$2" seed:=$((R + $3)) --lane int8; }

case ${3:-all} in all|trade)
log "trade on $M"; table
sdxl sdxl-1 "a lighthouse on a cliff at dawn, photo" 1; table
qwen qwen-1 "a red fox reading a newspaper in a sunlit cafe, film still" 2; table
sdxl sdxl-2 "an old stone bridge over a mountain river, autumn, photo" 3; table
qwen qwen-2 "a paper boat floating through a flooded library, painting" 4; table
sdxl sdxl-3 "a market stall piled with oranges at night, neon light, photo" 5; table
pod sh -c 'pgrep -af "cozy_runtime|executor" | cut -c1-120' | tee -a "$D/cell.log"
;; esac

case ${3:-all} in all|kill)
log "kill: a donor dies the moment a peer asks"
qwen qwen-3 "a snow globe holding a tiny city, macro photo" 6; table
pod python3 - kill-on-ask <"$HERE/table.py" >"$D/kill.log" 2>&1 & killer=$!
sdxl sdxl-4 "a brass telescope on a ship's deck at sunset, photo" 7
log "kill: $(cat "$D/kill.log")"; wait "$killer"; table
qwen qwen-4 "a greenhouse full of glass butterflies, soft light" 8; table
;; esac

case ${3:-all} in all|foreign)
log "foreign: dedicated gives nothing to a foreign program"
qwen qwen-5 "a cathedral made of ice under northern lights, wide shot" 9; table
pod python3 - 4 8 1 <"$HERE/foreign.py" | tee "$D/foreign-dedicated.json" | cut -c1-400 | tee -a "$D/cell.log"; table
cozy machine update "$M" --mode shared -y >"$D/mode.log" 2>&1; log "mode: $(tail -1 "$D/mode.log")"
qwen qwen-6 "a clockmaker's bench seen from above, warm lamp light, photo" 10; table
log "foreign: shared gives way"
pod python3 - 4 30 15 <"$HERE/foreign.py" | tee "$D/foreign-shared.json" | cut -c1-400 | tee -a "$D/cell.log"; table
sdxl sdxl-5 "a koi pond in the rain, seen from a wooden deck, photo" 11; table
;; esac

case ${3:-all} in all|binary)
pod sh -c 'c=$(command -v cozy || echo ~/.cozy/bin/cozy); ldd "$c"; d=$(mktemp -d); cp "$c" "$d/cozy"; cd "$d" && ./cozy --version; ls -la "$d"' | tee -a "$D/cell.log"
;; esac
log "done"
