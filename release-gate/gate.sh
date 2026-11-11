#!/bin/bash
# Release regression gate (tracker design ci-v2 §3): before every publish of cozy, the Runtime, TensorFS or a
# first-party package. Any FAIL blocks the publish. Runs by hand through the default ~/.cozy, and from
# .github/workflows/release-gate.yaml on a runner signed in with `cozy login --key`.
#   gate.sh <SKU> --suite name,... [--gpus N] [--out DIR] [--establish] [--publish] [--runtime F|URL] [--tensorfs F|URL]
#           [--tensord BIN|URL] [--packages DIR] [--pins name@release,...] [--only cell,cell] [--before FILE]...
#           [--cells FILE]... [--first pkg,...] [--cli BIN] [--disk GB] [--on MACHINE] [--stall MIN]
#           [--cap MIN]
# --on runs on that machine as it is (a dev proof on `local`): no rental, no --before, nothing ended.
# --stall: how long no run on the machine may stand still before the gate calls its cell hung (`hold`; default 15 minutes).
# --cap: the job's hard stop in minutes from the gate's start (the workflow's cap_minutes; 300). A download that can't land
# before it, at the rate it has kept, fails the host as too slow (`hold`), and another host is rented (th-364).
# --suite runs those suites (suites/<name>/: README.md says what each proves and costs); there is no default, the
# release agent chooses. A suite's cells.sh runs once the candidate is on, and its check.py in analyze.py; the baseline
# is gate-<SKU>-<suites>.json.
# A candidate URL is https://…#sha256=<hex>: fetched here, held by its digest, and sent as a file
# (`cozy machine update --runtime/--tensorfs/--tensord <path>`).
# One FRESH rental. The candidate goes on with $CLI's `machine update`, never the candidate binary's: a cozy of another
# version talking to a shared Calcifer restarts it on a route it lacks. The update fails unless the
# machine then runs it; the set the machine reports is the gate's label. Then each cell runs ONCE (e2e/run.sh)
# with a scenario prompt from scenarios.tsv and fresh seeds. A cell runs the root (first-party) package users get;
# --pins runs exact releases of them instead (a yanked candidate installs by its exact pin), --packages a packages
# checkout's dirs. A cell another suite ran, or one known-failing.json parks on the SKU (its "cells"), does not run.
# candidate.json records the set the verdict is for (release-gate/verify.sh reads it before a promotion).
# analyze.py holds each cell against the SKU's baseline, gate-<SKU>.json on the `release-gate-baselines`
# release; a SKU with none needs --establish. --publish uploads a PASS as the SKU's new baseline.
set -uo pipefail
# Run from a private copy: editing this file while a gate runs must never change the running gate. It carries when the
# gate began and which host this is: a host too slow for the cap is ended and the copy runs again on the next (`again`).
if [ "${1:-}" != --copy ]; then
  c=$(mktemp "${TMPDIR:-/tmp}/release-gate.XXXXXX.sh"); cp "$0" "$c"
  exec bash "$c" --copy "$(cd "$(dirname "$0")" && pwd)" "$(date +%s)" 1 "$@"
fi
HERE=$2 BEGAN=$3 HOST=$4 BEGUN_IN=$PWD; shift 4; ARGS=("$@")
. "$HERE/machines.sh"
REPO=cozy-creator/cozy-creator TAG=release-gate-baselines
SKU=$1; shift; CAND=() ONLY= EXTRA=() BEFORE=() CLI=cozy DISK=400 OUT= PKGS= ESTABLISH= PUBLISH= PINS= SUITES= K= CLIP= GPUS= FIRST= ON= STALL=15 STALL_STAGE=30 SEEDS= SCENARIO=
BIN= RW= TW= TD= CAP=300 HOSTS=5
while [ $# -gt 0 ]; do
  case $1 in
    --establish) ESTABLISH=1; shift; continue;;
    --publish) PUBLISH=1; shift; continue;;
    --out) OUT=$2;;
    --only) ONLY=",$2,";;
    --cells) EXTRA+=("$2");;
    --before) BEFORE+=("$2");;
    --suite) SUITES=$2;;
    --gpus) GPUS=$2;;
    --first) FIRST=$2;;
    --cli) CLI=$2;;
    --disk) DISK=$2;;
    --on) ON=$2;;
    --stall) STALL=$2;;
    --cap) CAP=$2;;
    --packages) PKGS=$(cd "$2" && pwd);;
    --pins) PINS=",$2,";;
    --runtime|--runtime-wheel) RW=$2;;
    --tensorfs|--tensorfs-wheel) TW=$2;;
    --tensord) TD=$2;;
    --seeds) SEEDS=$2;;
    --scenario) SCENARIO=$2;;
    *) echo "unknown argument $1" >&2; exit 2;;
  esac; shift 2
done
[ -n "$SUITES" ] || { echo "gate.sh needs --suite: $(ls "$HERE/suites" | paste -sd, -) (release-gate/README.md)" >&2; exit 2; }
for s in ${SUITES//,/ }; do
  [ -f "$HERE/suites/$s/cells.sh" ] || { echo "no suite $s: $(ls "$HERE/suites" | paste -sd, -)" >&2; exit 2; }
done
OUT=${OUT:-$PWD/gate-$SKU-$(date -u +%Y%m%dT%H%MZ)}
B=$OUT/baseline
mkdir -p "$B"; cd "$OUT"; : > cells.tsv
log() { echo "$(date -u +%H:%M:%SZ) $*" | tee -a gate.log; }
if [ -n "$PKGS" ]; then
  cargo build --quiet --locked --manifest-path "$HERE/../Cargo.toml" -p api --example stage-code --target-dir "$PWD/stage-code-build" > stage-code-build.log 2>&1 \
    || { log "could not build local-source staging helper: $(tail -c 500 stage-code-build.log)"; exit 1; }
  RUN_CODE_STAGE=$PWD/stage-code-build/debug/examples/stage-code
fi
# expect <check> PASS|FAIL <what happened>: a CLI-behaviour check (cells/*.sh); analyze.py fails the gate on a FAIL.
expect() { printf '%s\t%s\t%s\n' "$1" "$2" "$(tr '\n\t' '  ' <<< "$3" | cut -c1-400)" >> expect.tsv; log "$1: $2 $3"; }
# fetch <file|URL#sha256=hex>: the candidate as a file here, its digest checked.
fetch() {
  local f
  case $1 in
    '') return 0;;
    *://*#sha256=*) mkdir -p "$OUT/candidate"; n=$(basename "${1%%#*}")
      f=$OUT/candidate/$(printf '%b' "${n//%/\\x}")  # the file's own name: a URL's %2B is a local version's +
      [ -f "$f" ] || curl -fsSL --retry 5 --retry-all-errors "${1%%#*}" -o "$f" || { log "cannot fetch ${1%%#*}"; return 1; }
      [ "$(sha256sum "$f" | cut -d' ' -f1)" = "${1##*#sha256=}" ] || { log "$f does not hash to ${1##*#sha256=}"; return 1; };;
    *://*) log "$1 names no #sha256="; return 1;;
    *) f=$1; [ -f "$f" ] || { log "no file $f"; return 1; };;
  esac
  echo "$f"
}
RWF=$(fetch "$RW") && TWF=$(fetch "$TW") && TDF=$(fetch "$TD") || exit 1
if [ -n "$TDF" ]; then
  chmod +x "$TDF"; BIN=$("$TDF" --version | awk '{print $2}')
  [ -n "$BIN" ] || { log "$TD is not a cozy binary"; exit 1; }
fi
[ -z "$RWF" ] || CAND+=(--runtime "$RWF"); [ -z "$TWF" ] || CAND+=(--tensorfs "$TWF"); [ -z "$TDF" ] || CAND+=(--tensord "$TDF")
BK=$SKU${SUITES:+-${SUITES//,/-}}  # the baseline's name
echo "$BK" > "$OUT/baseline-key"  # release-gate.yml's baseline job publishes under it
gh release download "$TAG" -R "$REPO" -D "$B" -p "gate-$BK.json" -p "ref-$SKU.*" -p "ref-$SKU-clip.mp4" 2>/dev/null
if [ -f "$B/gate-$BK.json" ]; then
  [ -z "$ESTABLISH" ] || { log "$BK already has a baseline; --establish is only for a SKU without one"; exit 1; }
else
  [ -n "$ESTABLISH" ] || { log "no baseline for $BK on $REPO@$TAG: --establish starts one"; exit 1; }
fi
REF=$(ls "$B/ref-$SKU".* 2>/dev/null | grep -v -- -clip | head -1); REF=${REF:-$HERE/ref/seed.jpg}
mapfile -t ROWS < <(grep -v '^#' "$HERE/scenarios.tsv")
# --scenario and --seeds replay a logged gate ("scenario <name>, seeds from <n>") exactly.
I=$(( $(date +%s) / 60 + $(cksum <<< "$SKU" | cut -d' ' -f1) ))
[ -z "$SCENARIO" ] || for I in "${!ROWS[@]}"; do [ "${ROWS[$I]%%$'\t'*}" = "$SCENARIO" ] && break; done
IFS=$'\t' read -r NAME CHAR MOTION OBJECT SCENE <<< "${ROWS[$((I % ${#ROWS[@]}))]}"
[ -z "$SCENARIO" ] || [ "$NAME" = "$SCENARIO" ] || { echo "no scenario $SCENARIO in scenarios.tsv" >&2; exit 2; }
R=${SEEDS:-$(( $(date +%s%N) / 1000 % 4000000000 ))}
M= RID= RP= KEEP= AQ=
# The machine's own logs as they stand. A pod that's gone can't answer, so what was last saved stays.
save_machine_logs() {
  local log
  for log in memory tensord; do
    if timeout 60 "$CLI" machine logs "$M" "$log" -n 0 >"machine-$log.log.new" 2>&1; then mv -f "machine-$log.log.new" "machine-$log.log"; else rm -f "machine-$log.log.new"; fi
  done
}
end_it() {
  [ -z "$KEEP" ] || kill "$KEEP" 2>/dev/null
  [ -z "$AQ" ] || kill "$AQ" 2>/dev/null
  [ -z "$RP" ] || kill "$RP" 2>/dev/null  # the boot's follower only: the order is ended below
  if [ -n "$M" ]; then
    # The machine's own logs leave with it: keep them for every gate (a poisoned executor is diagnosed from them).
    save_machine_logs
    "$CLI" --json machine show "$M" >machine-end.json 2>/dev/null
    if "$CLI" logs --help >/dev/null 2>&1; then "$CLI" logs calcifer; else "$CLI" machine logs local calcifer -n 0; fi >calcifer.log 2>&1
  fi
  [ -z "$ON" ] || { M=; return; }
  [ -n "$RID" ] || return
  # `machine end` returns once the Hub affirms the pod gone.
  "$CLI" machine end "$RID" -y >end.log 2>&1 && ! listed "$RID" "$CLI" && { log "ended $RID${M:+ ($M)}, gone"; M= RID=; return; }
  log "WARNING $RID${M:+ ($M)} still listed: $(tail -c 300 end.log)"
}
# again: a host found too slow (`hold`, `host-slow`) whose rental is gone: its files go to slow-<machine>/ and the gate starts
# over on another host, at most HOSTS, as an order under the market is ordered again. Under --on the verdict stands.
again() {
  [ -s host-slow ] && [ -z "$ON" ] && [ -z "$RID" ] && (( HOST < HOSTS )) || return 0
  local aside; aside=slow-$(cat machine 2>/dev/null || echo "$HOST")
  mkdir -p "$aside" && find . -mindepth 1 -maxdepth 1 ! -name 'slow-*' -exec mv -t "$aside" {} +
  cd "$BEGUN_IN" && exec bash "$0" --copy "$HERE" "$BEGAN" $((HOST + 1)) "${ARGS[@]}" --out "$OUT"
}
trap 'end_it; again' EXIT
trap 'exit 143' TERM INT
RUN_CLI=$CLI; . "$HERE/../e2e/run.sh"
run_project "$PWD/project" || { log "cozy project new failed"; exit 1; }
# one_beat [args...]: H3's call in $BEAT: without `beats`, a text and its length are one beat, which goes to the model as
# written, and a reference's `kind` is its `asset_type`, or nothing for a clip or a sound, which say what they are by their
# file. What is already in H3's form passes through.
one_beat() {
  local text='' seconds='' arg beats=''
  BEAT=()
  for arg in "$@"; do [[ $arg != beats:=* ]] || beats=1; done
  for arg in "$@"; do
    case $arg in
      prompt=*) [ -n "$beats" ] && BEAT+=("$arg") || text=${arg#prompt=} ;;
      duration_s:=*) [ -n "$beats" ] && BEAT+=("$arg") || seconds=${arg#duration_s:=} ;;
      references:=*) BEAT+=("references:=$(jq -c 'map(if has("kind") | not then . elif .kind == "video" or .kind == "audio" then del(.kind) else (.asset_type = .kind | del(.kind)) end)' <<< "${arg#references:=}")") ;;
      *) BEAT+=("$arg") ;;
    esac
  done
  [ -z "$seconds" ] || BEAT+=("beats:=$(jq -cn --arg d "$text" --argjson s "$seconds" '[{description: $d, duration: $s}]')")
}
# ahead <name> <target> [args...]: the function's weights on $M's disk before its first timed call, its on-demand models'
# too (cl-790), as a user who has used it has them: TensorD admits a call on its up-front models and fetches on-demand ones
# behind it, so a cold upsampler's download was timed as Klein's execution. API warming to `downloaded` ends once they're here, and
# `off` takes the function out of the warm set, so nothing stays held from eviction. Once per function and the
# arguments that choose its weights.
declare -A AHEAD
ahead() {
  local name=$1 target=$2 want=() key; shift 2
  while [ $# -gt 0 ]; do
    case $1 in model.*=*|--lane=*|--lora=*) want+=("$1") ;; --lane|--lora) want+=("$1" "${2:-}"); shift ;; esac
    shift
  done
  key="$target ${want[*]}"
  [ -z "${AHEAD[$key]:-}" ] || return 0
  AHEAD[$key]=1
  run_warm_cell "$OUT" "$name-weights" "$target" downloaded --on "$M" "${want[@]}" ||
    expect "$name-weights" FAIL "run $(cat "$name-weights.number") $(run_get "$OUT" "$name-weights" .status): $(run_get "$OUT" "$name-weights" '.reason.message')"
  run_warm_cell "$OUT" "$name-weights-off" "$target" off --on "$M" > "$name-weights-off.log" 2>&1 || log "$name: API warming off: $(tail -c 300 "$name-weights-off.log")"
}
# cell <name> <target> [args...]: one run on $M (e2e/run.sh's run_cell): its record <name>.json, its outputs <name>/.
# --only names cells or packages. $TWIN, when a suite sets it, names a second run (<name>$TWIN) of the cells $TWINS lists.
# A cell another suite already ran, or one known-failing.json parks on this SKU ("cells"; sku "*" is every SKU), does
# not run.
cell() {
  local base=$1 name=$1${TWIN:-} target=$2 p issue n code=0; shift 2
  p=${target%/*}; p=${p##*/}
  [ -z "${TWIN:-}" ] || [[ ,${TWINS:-}, == *",$base,"* ]] || return 0
  [ -z "$ONLY" ] || [[ $ONLY == *",$base,"* || $ONLY == *",$p,"* ]] || return 0
  ! cut -f1 cells.tsv | grep -qx -- "$name" || return 0
  issue=$(jq -r --arg s "$SKU" --arg c "$base" '.[] | select((.sku == $s or .sku == "*") and ((.cells // []) | index($c))) | .issue' \
    "$HERE/known-failing.json" | head -1)
  [ -z "$issue" ] || { log "$name: pending fix $issue, not run on $SKU"; return 0; }
  for p in "$@"; do  # quality.py
    case $p in prompt=*) printf '%s\t%s\n' "$name" "${p#prompt=}" >> prompts.tsv ;; seed:=*) printf '%s\t%s\n' "$name" "${p#seed:=}" >> seeds.tsv ;; esac
  done
  [[ $target != *minimax-h3*/generate* ]] || { one_beat "$@"; set -- "${BEAT[@]}"; }
  ahead "$name" "$target" "$@"
  run_cell "$OUT" "$name" "$target" --on "$M" "$@" || code=$?
  n=$(cat "$name.number")
  printf '%s\t%s\t%s\n' "$name" "$target" "$n" >> cells.tsv
  log "$name run $n $(run_get "$OUT" "$name" .status)$(jq -r 'select(.error) | ": " + .error.message' "$name.ndjson" 2>/dev/null | head -1)"
}
# pkg <package> <fn>: the root package's function users get, its --pins release, or the --packages checkout's dir.
pkg() {
  local pin; pin=$(tr , '\n' <<< "$PINS" | grep -m1 "^$1@" | cut -d@ -f2)
  if [ -n "$pin" ]; then echo "$1/$2@$pin"
  elif [ -n "$PKGS" ] && grep -qs cozy.application "$PKGS/$1/pyproject.toml"; then ln -sfn "$PKGS/$1" "$OUT/$1"; echo "./$1/$2"
  else echo "$1/$2"; fi
}
output() { run_output "$OUT" "$1" "$2"; }
. "$HERE/products.sh"
log "gate on $SKU (suites ${SUITES//,/ }): scenario $NAME, seeds from $R, candidate: ${CAND[*]:-released}${PKGS:+ packages $PKGS}${PINS:+ pins ${PINS//,/ }}, cli: $CLI"
[ "$HOST" = 1 ] || log "host $HOST of $HOSTS: $(cat slow-*/host-slow | paste -sd';')"
[ -z "$ON" ] || { M=$ON; BEFORE=(); }
# acquisition <rental>: the Hub's acquisition attempt while the rental boots, every 5 s into acquisition.jsonl (cl-684).
# An attempt whose create is still unanswered (obligated, ambiguous) one inventory pass (120 s, plus 30 s for the
# absence reads) after it began is the 10-29 hang: RunPod refused the create, the pod was read absent, the attempt's
# failure rolled back and nothing replanned for 5 h. Its rent is stopped there and the gate fails.
ONE_PASS=150
# An order buys nothing over the price it accepted and waits for capacity however long: on 11-03 two gates sat 3 h 40 min
# a cent under the market. A moved price gets a new order at it, at most REORDERS times. RENT_WAIT s with no pod bought
# and the gate fails saying so, and ends the order. A pod already bought isn't timed here: a stuck boot is the Hub's to replace.
RENT_WAIT=1200 REORDERS=5
acquisition() {
  local show
  while :; do
    show=$("$CLI" --json machine show "$1" 2>/dev/null) || { sleep 5; continue; }
    jq -c --argjson t "$(date +%s%3N)" '{t: $t, state, attempt}' <<< "$show" >> acquisition.jsonl
    [ "$(jq -r .state <<< "$show")" = booting ] || return 0
    if jq -e --argjson pass $((ONE_PASS * 1000)) '.attempt | select(.state == "obligated" or .state == "ambiguous")
        | .observed_at_ms - .started_at_ms > $pass' <<< "$show" > /dev/null 2>&1; then
      jq -r '"attempt \(.attempt.number) still \(.attempt.state) \((.attempt.observed_at_ms - .attempt.started_at_ms) / 1000 | floor) s after it began"' \
        <<< "$show" > acquisition.stuck
      [ -z "$RP" ] || kill "$RP" 2>/dev/null
      return 0
    fi
    if jq -e '.attempt == null or .attempt.state == "replanning"' <<< "$show" > /dev/null 2>&1; then
      if tail -n 1 rent.err | grep -q 'rent again to take it'; then
        tail -n 1 rent.err > acquisition.repriced
      elif (( SECONDS - RENT_BEGAN > RENT_WAIT )); then
        echo "no pod after $(( (SECONDS - RENT_BEGAN) / 60 )) min: $(tail -n 1 rent.err)" > acquisition.stuck
      fi
      if [ -s acquisition.repriced ] || [ -s acquisition.stuck ]; then
        [ -z "$RP" ] || kill "$RP" 2>/dev/null
        return 0
      fi
    fi
    sleep 5
  done
}
# acquired: the cell's verdict. Each replan the rent saw is named; a refused create shows as an attempt replaced for
# provider.no_capacity. A cozy whose machines name no attempt (before 0.13.16) leaves only a log line.
acquired() {
  local v
  [ -s acquisition.jsonl ] || return 0
  if [ -s acquisition.stuck ]; then expect "acquisition replans" FAIL "$(cat acquisition.stuck)"; return; fi
  v=$(python3 -I - acquisition.jsonl <<'PY'
import json, sys
rows = [json.loads(l) for l in open(sys.argv[1])]
attempts = [r['attempt'] for r in rows if r.get('attempt')]
if not attempts:
    sys.exit()
seen = {}
for a in attempts:
    seen.setdefault(a['number'], a)
    if a.get('started_at_ms'):
        seen[a['number']] = a
said = []
for n, a in sorted(seen.items()):
    r = a.get('replaced')
    if r:
        gap = f"{(a['started_at_ms'] - r['ended_at_ms']) / 1000:.0f} s after it ended" if a.get('started_at_ms') else 'still replanning'
        said.append(f"attempt {n} replaced attempt {r['number']} ({r['failure']}), {gap}")
print('; '.join(said) or 'its create was answered: no refused create to replan this time')
PY
)
  [ -n "$v" ] && expect "acquisition replans" PASS "$v" || log "acquisition replans: this cozy names no acquisition attempt"
}
# rented <machine>: its rental id into RID and `rental`, which end_it and the workflow's cleanup end.
rented() { RID=$("$CLI" --json machine show "$1" 2>/dev/null | jq -r '.rental_id // empty'); [ -z "$RID" ] || echo "$RID" > rental; }
if [ -z "$ON" ]; then
  # Suite tensorfs's `kill` case kills the rental's TensorD over SSH, suite tensord measures the pod's link over it, and suite
  # varena's faults run there (faults.sh), with this computer's key, or a new one on a runner. An --only without them rents no SSH.
  SSH=
  if [[ ,$SUITES, == *,tensord,* ]] || { [[ ,$SUITES, == *,tensorfs,* ]] && { [ -z "$ONLY" ] || [[ $ONLY == *,kill,* ]]; }; } ||
     { [[ ,$SUITES, == *,varena,* ]] && { [ -z "$ONLY" ] || [[ $ONLY == *,faults,* ]]; }; }; then
    SSH=1; [ -f ~/.ssh/id_ed25519.pub ] || ssh-keygen -q -t ed25519 -N '' -f ~/.ssh/id_ed25519
  fi
  # Followed in the background: the order's first booting line names it, so a gate cancelled mid-boot still ends it.
  RENT_BEGAN=$SECONDS
  for ((order = 1; order <= REORDERS; order++)); do
    : > rent.err
    "$CLI" machine rent "$SKU" ${GPUS:+--gpus "$GPUS"} ${SSH:+--ssh} --disk-gb "$DISK" -y >rent.out 2>rent.err & RP=$!
    until first=$(grep -m1 -oE '^[a-z0-9][a-z0-9-]*: (booting|ready|offline)' rent.err | cut -d: -f1); [ -n "$first" ] || ! kill -0 "$RP" 2>/dev/null; do
      sleep 2
    done
    for _ in 1 2 3; do [ -z "$first" ] || [ -n "$RID" ] || rented "$first"; [ -n "$RID" ] || sleep 5; done
    [ -z "$RID" ] || { acquisition "$RID" & AQ=$!; }
    wait "$RP"; rc=$? RP=
    [ -z "$AQ" ] || { kill "$AQ" 2>/dev/null; wait "$AQ" 2>/dev/null; AQ=; }
    # A boot that never starts is released unbilled (th-358: `cpu` with SSH, often): a new order may place elsewhere.
    if [ "$rc" != 0 ] && grep -q boot_idle rent.err && [ "$order" -lt "$REORDERS" ]; then
      log "order $order ended boot_idle, nothing billed: ordering again"
      cat rent.err >> rent-orders.err
      RID= first=; rm -f acquisition.jsonl rental
      continue
    fi
    [ -s acquisition.repriced ] || break
    log "order $order, $(head -n 1 rent.err), is under the market: $(grep -oE 'waiting for an offer.*' acquisition.repriced)"
    cat rent.err >> rent-orders.err
    if [ "$order" = "$REORDERS" ]; then
      echo "$REORDERS orders, each under the market by the time it could place" > acquisition.stuck
      break
    fi
    "$CLI" machine end "$RID" -y >end.log 2>&1 && ! listed "$RID" "$CLI" || { log "rent failed: $RID won't end: $(tail -c 300 end.log)"; exit 1; }
    RID= first=; rm -f acquisition.repriced acquisition.jsonl rental
  done
  acquired
  [ -n "$RID" ] || { first=$(awk '$1=="machine"{print $2; exit}' rent.out); [ -z "$first" ] || rented "$first"; }
  [ "$rc" != 0 ] || [ -z "$RID" ] || M=$("$CLI" --json machine show "$RID" 2>/dev/null | jq -r '.name // empty')
fi
[ -n "$M" ] || { log "rent failed: $(cat acquisition.stuck 2>/dev/null) $(tail -c 300 rent.err) $(tail -c 300 rent.out)"; exit 1; }
echo "$M" > machine
log "${ON:+on }${ON:-rented} $M"
# A host this gate already found too slow is ended at once: the Hub may offer it again.
"$CLI" --json machine show "$M" 2>/dev/null | jq -r '.address // empty' | sed 's/:[0-9]*$//' > address
if [ -z "$ON" ] && [ -s address ] && grep -qsxF -f address slow-*/address; then
  echo "host too slow: $M ($(cat address)) was found so earlier in this gate" > host-slow
  printf 'FAIL %s\nVERDICT FAIL (host too slow)\n' "$(cat host-slow)" | tee verdict.txt
  exit 1
fi
# hold: every 240 s while this gate lives (a killed gate's loop ends at its next look), the runs' own clock decides.
# A move within --stall minutes restarts the rental's 15-minute idle clock, which the gate's work between runs needs
# (fetches, uploads, analysis). No move that long with a run open is a hung cell (the longest quiet in 786 real cells was
# 7.2 min): its runs are canceled, and the gate fails and ends the rental. With none open, nothing renews. So does a
# listing that hasn't answered for that long, and a run that has said it stands in one place for STALL_STAGE minutes: a run
# whose every event moves the clock can crawl inside one stage until the job's cap (41904557331, 54 min at decode_video).
# A download that can't land before the cap at the rate it has kept since hold first saw it (4 to 8 min in) fails the host
# as too slow (too_slow), so it is ended and another rented (`again`): moving bytes keep the clock above going, and a host far
# below the link floor waited out a 120-minute cap (th-364: 7 MB/s, half of H3's 48.9 GiB in 1 h 36 min).
DEADLINE=$(( BEGAN + CAP * 60 ))
too_slow() {  # into $why; in hold's shell, which keeps `first`
  local n got all t0 b0 a0 now; now=$(date +%s)
  while read -r n got all; do
    read -r t0 b0 a0 <<< "${first[$n]:-}"
    if [ -z "$t0" ] || [ "$all" != "$a0" ] || (( got < b0 )); then first[$n]="$now $got $all"; continue; fi
    (( got > b0 )) || continue
    why=$(awk -v n="$n" -v moved=$((got - b0)) -v s=$((now - t0)) -v left=$((all - got)) -v cap=$((DEADLINE - now)) 'BEGIN {
      need = left * s / moved; if (need <= cap) exit 1
      printf "host too slow: %.1f MB/s, %.1f GiB needs %d min, and the cap leaves %d (run %s)", moved / s / 1e6, left / 2^30, need / 60 + 0.5, cap / 60, n }') && return 0
  done < <(run_downloads "$M" "$CLI")
  return 1
}
# beat: the gate's newest line as a commit status: off the runner, nothing else shows a gate's cells (a running job's log
# isn't served, and its pod answers only the runner's Calcifer).
beat() {
  [ -n "${GITHUB_RUN_ID:-}" ] || return 0
  gh api -X POST "repos/$GITHUB_REPOSITORY/statuses/$GITHUB_SHA" -f state=success -f context="gate/$GITHUB_RUN_ID/$SKU" \
    -f description="$(printf '%s | %s' "$(tail -n 1 gate.log | cut -c1-70)" "$(paste -sd';' <<< "$1" | cut -c1-60)")" > /dev/null 2>&1 || true
}
hold() {
  local clock at active seen= moved=$SECONDS stands stood= still=$SECONDS why verdict
  local -A first
  while sleep 240 && kill -0 $$ 2> /dev/null; do
    stands=$(run_stands "$M" "$CLI")
    [ -n "$ON" ] || save_machine_logs
    beat "$stands"
    [ "$stands" = "$stood" ] || { stood=$stands still=$SECONDS; }
    why=
    if clock=$(run_clock "$M" "$CLI"); then
      read -r at active <<< "$clock"
      [ "$at" = "$seen" ] || { seen=$at moved=$SECONDS; }
      if (( SECONDS - moved >= STALL * 60 )); then why="no run on $M moved for $(( (SECONDS - moved) / 60 )) min"
      elif [ -n "$stands" ] && (( SECONDS - still >= STALL_STAGE * 60 )); then why="a run on $M stood for $(( (SECONDS - still) / 60 )) min at: $(paste -sd';' <<< "$stands" | cut -c1-120)"
      elif too_slow; then :
      else
        [ -n "$ON" ] || "$CLI" machine keepalive "$M" > /dev/null 2>&1
        continue
      fi
    elif (( SECONDS - moved >= STALL * 60 )); then why="no listing of $M's runs answered for $(( (SECONDS - moved) / 60 )) min"; active=
      printf 'FAIL hung: %s\nVERDICT FAIL (hung)\n' "$why" | tee verdict.txt
      expect cell-progress FAIL "$why"
      kill -TERM $$
      return
    else
      continue
    fi
    # On a machine it did not rent, only its own runs (each cell's <name>.number).
    [ -z "$ON" ] || active=$(find "$OUT" -name '*.number' -exec cat {} + | grep -xF -f <(tr ' ' '\n' <<< "$active") | paste -sd' ')
    [ -n "$active" ] || continue
    why="$why; canceled $active"
    case $why in
      'host too slow'*) echo "$why" > host-slow; expect host-speed FAIL "$why"; verdict='host too slow' ;;
      *) expect cell-progress FAIL "$why"; why="hung: $why" verdict=hung ;;
    esac
    for at in $active; do "$CLI" run cancel "$at" > /dev/null 2>&1; done
    printf 'FAIL %s\nVERDICT FAIL (%s)\n' "$why" "$verdict" | tee verdict.txt
    kill -TERM $$
    return
  done
}
hold & KEEP=$!
"$CLI" machine show "$M" > machine-fresh.txt 2>&1
# reported: $M's cozy, Runtime and TensorFS into $v $rt $tfs once it reports them (a fresh pod does once up); empty
# when it ends first, or Calcifer answers a typed error.
reported() {
  local show st
  while show=$("$CLI" --json machine show "$M" 2>/dev/null); do
    IFS='|' read -r v rt tfs st <<< "$(jq -r '[(.status // {} | .version, .runtime, .tensorfs), .state] | map(. // "") | join("|")' <<< "$show")"
    [ -z "$v" ] && [ "$st" != ending ] && [ "$st" != ended ] || return 0; sleep 5
  done
}
# Sourced on the fresh pod, before the candidate goes on: the `--before FILE`s.
for f in "${BEFORE[@]}"; do . "$f"; done
# A machine already on the candidate cozy (a --before FILE moved it) keeps it: no second upload or restart.
if [ -n "$BIN" ] && [ "$("$CLI" --json machine show "$M" | python3 -c 'import json,sys; print(json.load(sys.stdin)["status"]["version"])' 2>/dev/null)" = "$BIN" ]; then
  for ((i = 0; i < ${#CAND[@]}; i += 2)); do [ "${CAND[i]}" = --tensord ] && CAND=("${CAND[@]:0:i}" "${CAND[@]:i+2}") && break; done
fi
# notinstalled <why>: the cells would run the old software, so the gate stops here: logs kept, pod ended, FAIL.
notinstalled() {
  expect candidate-installed FAIL "$1"
  printf 'FAIL candidate not installed: %s\nVERDICT FAIL (candidate not installed)\n' "$1" | tee verdict.txt
  exit 1
}
# cozy's `machine update` fails (update_unapplied) unless the machine then runs what it was sent.
if [ ${#CAND[@]} -gt 0 ]; then
  "$CLI" --json machine update "$M" "${CAND[@]}" -y >update.json 2>update.log; code=$?
  log "candidate on $M: exit $code $(jq -c 'select(.machine and .tensord)' update.json 2>/dev/null)"
  [ "$code" = 0 ] || notinstalled "cozy machine update exit $code: $(jq -r 'select(.error) | "\(.error.code): \(.error.message)"' update.json 2>/dev/null) $(tail -c 300 update.log | tr '\n' ' ')"
fi
"$CLI" machine show "$M" > machine.txt 2>&1
reported
[ -n "$v" ] && [ -n "$rt" ] && [ -n "$tfs" ] || notinstalled "$M reports no cozy, Runtime and TensorFS (cozy ${v:-?}, Runtime ${rt:-?}, TensorFS ${tfs:-?})"
expect candidate-installed PASS "$M runs cozy $v, Runtime $rt, TensorFS $tfs"
# The set the verdict is for (verify.sh, promote.sh): each candidate's version, as the machine reports it, and digest.
# Components with no candidate are not named: a --before FILE may move them on its SKU only.
LABEL=cozy$v-rt$rt-tfs$tfs${PKGS:+-packages$(git -C "$PKGS" rev-parse --short HEAD)}${PINS:+-pins$(cksum <<< "$PINS" | cut -d' ' -f1)}
echo "$LABEL" > label; log "gate $LABEL"
digest() { [ -z "$1" ] || sha256sum "$1" | cut -d' ' -f1; }
jq -n --arg label "$LABEL" --arg cozy "${TDF:+$v}" --arg runtime "${RWF:+$rt}" --arg tensorfs "${TWF:+$tfs}" --arg cozy_sha256 "$(digest "$TDF")" \
  --arg runtime_wheel "$(basename "${RWF:-}")" --arg runtime_sha256 "$(digest "$RWF")" --arg tensorfs_wheel "$(basename "${TWF:-}")" \
  --arg tensorfs_sha256 "$(digest "$TWF")" --arg pins "${PINS//,/ }" --arg packages_ref "${PKGS:+$(git -C "$PKGS" rev-parse HEAD)}" \
  -S '$ARGS.named | .pins |= (split(" ") | map(select(. != "")) | sort)' > candidate.json
printf '%s\n' ${SUITES//,/ } > suites
printf '%s\n' ${ONLY//,/ } > only  # what --only chose, for a suite's check.py (empty: everything)
for s in ${SUITES//,/ }; do . "$HERE/suites/$s/cells.sh"; done
# More cells (`--cells FILE`): `name<TAB>target<TAB>args`, args expanded here ($SCENE, $CHAR, $R, $K, $M, $CLI ...).
for f in "${EXTRA[@]}"; do
  [[ $f == *.sh ]] && continue
  while IFS=$'\t' read -r name target args; do
    [ -n "$name" ] && [ "${name:0:1}" != "#" ] && eval "cell \"\$name\" \"\$target\" $args"
  done < "$f"
done
# A --cells FILE ending in .sh is sourced last, for cells that change the machine between them.
for f in "${EXTRA[@]}"; do [[ $f != *.sh ]] || . "$f"; done
end_it
uv run -q --no-project --with pillow --with numpy --with torch --with transformers --index https://download.pytorch.org/whl/cpu \
  --index-strategy unsafe-best-match python -I "$HERE/analyze.py" "$OUT" "$SKU" "$B/gate-$BK.json" | tee verdict.txt
[ "${PIPESTATUS[0]}" = 0 ] || exit 1
# A PASS hands the next gate new references of the same shape: this Klein image and 3 s of this H3 video.
if [ -z "$ONLY" ] && [ -n "$K" ] && [ -n "$CLIP" ]; then
  rm -f "$B/ref-$SKU".* && cp "$K" "$B/ref-$SKU.${K##*.}"
  nice -n 19 ffmpeg -v error -y -i "$CLIP" -t 3 -an -c:v libx264 -pix_fmt yuv420p "$B/ref-$SKU-clip.mp4"
fi
[ -z "$PUBLISH" ] || "$HERE/publish.sh" "$OUT" "$BK" | tee -a gate.log
