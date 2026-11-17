# release-gate/sessions/lib.sh: what every session shares (pod-session.yaml). A session sources it, checks what it
# will run, calls `quote <seconds it needs> [pods]` (the $0 pre-flight ends there unless RENT=true), then
# `rent` once per pod (or `order` each, then `ready` each, to boot them together), and works on $NAME; the EXIT trap
# ends every rental by its own rental id and checks each is gone. RENT_ARGS: more `machine rent` flags (`--ssh`).
# From the workflow: OUT, SKU, DISK, CAP (USD, all in), RENT, PARAMS (a JSON object), PACKAGES (packages/<name>
# checkouts), REF (release-gate/ref), HUB (prod or isolated: e2e/test-hub.sh require_ci_rental_hub). PARAMS'
# `floor_mb_s` (50; 0 for none) and `floor_after_s` (120) set the slow-host floor (`step`).
# Every failure is handled here (the trap ends the rental), not by bash -e.
set +e -uo pipefail
. "$(dirname "${BASH_SOURCE[0]}")/../../e2e/test-hub.sh"
require_ci_rental_hub "${HUB:-isolated}" || exit 2
MODEL_DETAILS=$(cd "$(dirname "${BASH_SOURCE[0]}")/../../e2e" && pwd)/model-details.py
model_details() { python3 "$MODEL_DETAILS" "$1"; }
mkdir -p "$OUT"; OUT=$(cd "$OUT" && pwd); R=$OUT/results.txt
say() { echo "$(date -u +%T) $*" | tee -a "$R"; }
# Made up, and here so each published session shows its scrub ran (e2e/publish): in its log, results.txt and this file it reads `Bearer [scrubbed]`.
say 'scrub canary: authorization: Bearer 0000-made-up-0000' | tee "$OUT/canary.err"
RC=0 RID='' NAME='' WD='' BG='' DL='' RENT_ARGS=''  # BG: a session's own background loop, ended with it
RIDS=()  # every rental this session ordered; $OUT/rental lists them, one a line
fail() { say "FAIL: $*"; RC=1; exit 1; }
# param <key> [default]: the session's setting from PARAMS (a non-string as JSON).
param() { jq -r --arg k "$1" --arg d "${2-}" 'if has($k) then (.[$k] | if type == "string" then . else tojson end) else $d end' <<<"$PARAMS"; }
# The slow-host floor (`floor_mb_s`, 0 for none; `floor_after_s`): a step whose upload to the pod or weights download still
# reports under it that long in ends the session, `host too slow`, before the host's link eats the cap (th-377).
FLOOR_MBS=$(param floor_mb_s 50) FLOOR_AFTER=$(param floor_after_s 120)
[[ $FLOOR_MBS =~ ^[0-9]+$ && $FLOOR_AFTER =~ ^[0-9]+$ ]] || fail "floor_mb_s $FLOOR_MBS, floor_after_s $FLOOR_AFTER"
state() { cozy machine show "$1" --json 2>/dev/null | jq -r '.state // empty'; }
. "$(dirname "${BASH_SOURCE[0]}")/../machines.sh"
gone() { local s; s=$(state "$1"); { [ "$s" = ended ] || [ -z "$s" ]; } && ! listed "$1"; }
# wheel_from <dir> <source>: a manylinux x86_64 wheel fetched into dir, its path printed. source: an https URL,
# `artifact:<run id>/<name>` of a cozy-creator run (a dev-build's tensorfs-wheel-<sha>) or a cozy-runtime dev-build run id
# (its cozy-runtime-<sha> carries the kernels a machine's Runtime must have: a pure wheel is `runtime_without_kernels`).
wheel_from() {
  mkdir -p "$1"
  case $2 in
    https://*) curl -fsSL --retry 5 -o "$1/${2##*/}" "$2" || return 1 ;;
    artifact:*/*) local a=${2#artifact:}; gh run download "${a%%/*}" -R cozy-creator/cozy-creator -n "${a#*/}" -D "$1" >/dev/null 2>&1 || return 1 ;;
    *) gh run download "$2" -R cozy-creator/cozy-runtime -p 'cozy-runtime-*' -D "$1" >/dev/null 2>&1 || return 1 ;;
  esac
  find "$1" -name '*manylinux*x86_64*.whl' | head -1
}
# made <name>: the checkpoint a step's upload or quantize made, as sha256:<hex>: its `checkpoint <org/name>@sha256:…` line
# (a machine's own `checkpoint sha256:…` line names no model); cozy 0.24's quantize says `commit`.
made() { awk '($1=="checkpoint" || $1=="commit") && $2 ~ /@sha256:/ {c=$2} END{sub(/.*@/, "", c); print c}' "$OUT/$1.txt"; }
# runno <name>: the run a step's output names last (its own, after a local source's sync run): its KEY-N.
runno() { awk '$1=="run" && NF==2{print $2}' "$OUT/$1.txt" | tail -1; }
# handle <ndjson>: what names the run or job its events tell of from any folder: a run's KEY-N (cl-987), a job's id (cl-1028: no
# project's run, it has no name or number of a person's). A line may lead with the runner's clock.
handle() { sed 's/^[0-9.]* {/{/' "$1" 2>/dev/null | grep '^{' | jq -r 'select(.run.number != null) | if (.run.name // "") != "" then .run.name else .run.id end' 2>/dev/null | head -1; }
finish() {
  local status=$?; [ "$RC" != 0 ] || RC=$status
  # From here a signal is noted, not obeyed: nothing the session started (the deadline watcher, a helper) or a cancel stops
  # the rental ends below. 43971316155's watcher TERMed this trap and the session exited without its RESULT.
  trap 'RC=1' INT TERM HUP
  # The helpers stop first, each one's children before itself: a watcher whose sleep is killed exits without acting.
  for p in $DL $WD $BG; do pkill -P "$p"; kill "$p"; done 2>/dev/null
  local r
  for r in "${RIDS[@]}"; do
    local tries=0
    until gone "$r"; do
      if [ $((tries % 12)) = 0 ]; then
        cozy machine end "$r" -y >> "$OUT/end.txt" 2>&1 || cozy machine end "$r" -y --force >> "$OUT/end.txt" 2>&1
        say "END requested $r"
      fi
      tries=$((tries + 1)); sleep 10
    done
    say "GONE $r: ended, and the machine listing holds no live machine of it; billed $(billed "$r")"
  done
  say "RESULT $([ "$RC" = 0 ] && echo PASSED || echo FAILED)"
  exit "$RC"
}
trap finish EXIT
trap 'RC=1; exit 1' INT TERM HUP
# The step's deadline (SESSION_UNTIL, pod-session.yaml): 5 min before it the session ends itself, so its trap ends its rentals and
# says END, GONE and the bill before the step is killed.
if [ -n "${SESSION_UNTIL:-}" ]; then
  ( left=$((SESSION_UNTIL - $(date +%s) - 300)); [ "$left" -le 0 ] || sleep "$left" || exit 0  # killed by `finish`
    say "DEADLINE: the step ends in 5 min; this session ends now"; kill -TERM $$ ) 9>&- & DL=$!
fi
# derived_state <label> <reset|settle|census>: the pod's derived state (release-gate/derived-state.sh, machines.sh
# `pod_derived`): `reset` before an update's restart begins an arm, `settle` after it clears what the ended executors still
# wrote and counts what is left. DERIVED holds the census line; a cold arm
# starts with no derived page, memo entry or learned peak (`derived_zero`). The call memo can't be cleared there: a record
# it answered says `memoized`, and ab.sh never compares one. No `cozy` verb clears any of this (tfs-299): the root shell
# is the only way in.
DERIVED=''
# The pod-side script is checked before anything is rented: a session without it fails here, at no cost.
[ -f "$POD_DERIVED_SCRIPT" ] || fail "no $POD_DERIVED_SCRIPT (pod-session.yaml's sparse checkout)"
derived_state() {
  local try line='' code
  for try in 1 2 3 4 5 6; do
    line=$(pod_derived cozy "$NAME" "$2" 2> "$OUT/derived-$1-$2.err"); code=$?
    [ -z "$line" ] || break
    [ "$code" != 2 ] || fail "$1: $(head -c 200 "$OUT/derived-$1-$2.err")"
    say "$1: derived state $2, try $try: $(grep -v '^Warning: Permanently added' "$OUT/derived-$1-$2.err" | head -c 200)"; sleep 10
  done
  [ -n "$line" ] || fail "$1: no derived-state $2 from $NAME"
  DERIVED=$line; say "$1: derived state after $2: $line"
}
derived_zero() { pod_derived_zero "$DERIVED"; }
# Arms on one pod (tfs-299): each starts cold or warm by declaration, never by what an earlier arm left. Cold: `derived_state
# <arm> reset` before the update that restarts the machine (`cold_restart` when the arm updates nothing), then `arm_cold`.
# Warm: its own warm-up first.
# arm_cold <label>: after the restart that begins a cold arm, the census finds no derived page, memo entry or learned peak, or
# the session fails.
arm_cold() { derived_state "$1" census; derived_zero || fail "$1: its arm starts with derived state it did not make: $DERIVED"; }
# cold_restart <label>: a cold arm with no update of its own: the derived state goes, a settings-only update (`--gpu-memory-limit
# off`) restarts TensorD and ends every executor, then `arm_cold`.
cold_restart() {
  derived_state "$1" reset
  step "restart-$1" machine update "$NAME" --gpu-memory-limit off -y || fail "$1: the restart"
  arm_cold "$1"
}
# states.tsv: each compared record's line (`state_row`): its arm, its temperature (cold: its function's first request since its
# arm began cold; warm: a later one; warmup: never compared), LoRA bake blocks, derived bytes filled and persisted pages (the
# run's measurements and its calls', every GPU), whether the call memo answered it, its seconds and whether it fetched weights.
STATES=$OUT/states.tsv
# cellstate <record json>: its bake blocks, derived bytes, persisted pages and whether the call memo answered it, tab-separated.
cellstate() { jq -r '. as $r | [.measurements, (.calls[]?.measurements)] | map(select(. != null) | .memory.stats.varena[]?.stats // empty) as $v
  | [($v | map(.moved.bake.blocks // 0) | add // 0), ($v | map(.moved.derived.bytes // 0) | add // 0),
     ($v | map(.baked_pages.persisted // 0) | max // 0), (($r.memo.memoized // false) or ([$r.calls[]?.memoized] | any) | tostring)] | @tsv' \
  "$1" 2>/dev/null; }
# fetched_weights <output>: whether a run's output (text or --json) shows weights downloaded inside the request: its seconds
# are then the link's (44006261838: 0.25.0's first H3 spent 454 s of 822 there).
fetched_weights() { if grep -qE 'downloading .* weights [0-9.]+ [KMG]i?B/|"downloading [^"]* weights' "$1" 2>/dev/null; then echo true; else echo false; fi; }
# state_row <label> <arm> <temperature> <record json> [output]: its line in states.tsv, said.
state_row() {
  local bake derived persisted memo f s
  [ -s "$STATES" ] || printf 'cell\tarm\ttemperature\tbake_blocks\tderived_bytes\tpersisted\tmemoized\texec_s\tdownloaded\n' > "$STATES"
  read -r bake derived persisted memo < <(cellstate "$4")
  f=$(fetched_weights "${5:-/dev/null}") s=$(jq -r '(.execution_ms // 0) / 1000' "$4" 2>/dev/null)
  printf '%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\n' "$1" "$2" "$3" "${bake:-0}" "${derived:-0}" "${persisted:-0}" "${memo:-false}" "${s:-0}" "$f" >> "$STATES"
  say "$1 state: $3, bake ${bake:-0}, derived $(( ${derived:-0} / 1000000000 )) GB, persisted ${persisted:-0}, memoized ${memo:-false}, fetched weights $f"
}
# unfit <label> [fetches]: why its record measures nothing of its temperature (empty: it does): the call memo answered it, it
# fetched weights inside the request (unless `fetches`: what the session measures), or, cold, it read derived pages it did not make.
unfit() {
  local row t bake derived memo dl
  row=$(awk -F'\t' -v k="$1" '$1 == k' "$STATES" 2>/dev/null | tail -1)
  [ -n "$row" ] || { echo "$1 did not run"; return; }
  IFS=$'\t' read -r _ _ t bake derived _ memo _ dl <<<"$row"
  if [ "$memo" = true ]; then echo "the call memo answered $1"
  elif [ "$dl" = true ] && [ "${2:-}" != fetches ]; then echo "$1 fetched its weights inside the request"
  elif [ "$t" = cold ] && [ "$derived" != 0 ] && [ "$bake" = 0 ]; then echo "cold $1 read derived pages it did not make"
  fi
}
# states_unfit: `unfit` of every record in states.tsv, `;`-joined (empty: each measures its temperature).
states_unfit() { awk -F'\t' 'NR > 1 {print $1}' "$STATES" 2>/dev/null | while read -r l; do unfit "$l"; done | paste -sd';'; }
# incomparable <a> <b> [fetches]: why two records can't be compared (empty: they can): a warm-up, temperatures that differ, or
# either `unfit`.
incomparable() {
  local ta tb
  ta=$(awk -F'\t' -v k="$1" '$1 == k {t = $3} END {print t}' "$STATES" 2>/dev/null)
  tb=$(awk -F'\t' -v k="$2" '$1 == k {t = $3} END {print t}' "$STATES" 2>/dev/null)
  if [ -z "$ta" ] || [ -z "$tb" ]; then echo "a cell did not run"
  elif [ "$ta" = warmup ] || [ "$tb" = warmup ]; then echo "a warm-up is never compared"
  elif [ "$ta" != "$tb" ]; then echo "$1 is $ta, $2 is $tb"
  else { unfit "$1" "${3:-}"; unfit "$2" "${3:-}"; } | head -1
  fi
}
# claim: what this runner's Calcifer ordered (machines.sh `own`) joins RIDS and $OUT/rental, for a session whose own work
# orders too (a page's Rent). Only on a runner, whose Calcifer started with none.
claim() {
  local r
  for r in $(own); do
    [[ " ${RIDS[*]:-} " == *" $r "* ]] || { RIDS+=("$r"); echo "$r" >> "$OUT/rental"; say "this runner's Calcifer also holds $r"; }
  done
}
# step <name> <cozy args…>: one call followed to its end (looked at each second, its output every 30 s); output that
# stops growing for 15 min cancels it.
# A run belongs to this session's working-directory project.
RUN_PROJECT=$OUT/project
# A folder is a project once `cozy new` made it one (0.24: `status` reads one, it makes none).
. "$(dirname "${BASH_SOURCE[0]}")/../../e2e/run.sh"
{ run_project "$RUN_PROJECT" && (cd "$RUN_PROJECT" && "$RUN_CLI" --json status); } > "$OUT/project.json" 2>> "$OUT/project.err" \
  || fail "project initialization: $(cat "$OUT/project.err") $(tail -c 300 "$OUT/project.json")"
# kernels <package dir>: a packages checkout's package, run from source, carries the Triton sets its release ships, built
# into the checkout once by the checkout's scripts/triton.sh, so its first request is a Hub install's (xs-194). Call it
# for each package before `rent`: a build after it bills the pod while the runner compiles.
declare -A KERNELS=()
kernels() {
  local d root
  d=$(cd "$1" 2>/dev/null && pwd) || return 0
  root=$(dirname "$d")
  [ -x "$root/scripts/triton.sh" ] && [ -f "$d/pyproject.toml" ] && [ -z "${KERNELS[$d]:-}" ] || return 0
  KERNELS[$d]=1
  local t0=$SECONDS
  "$root/scripts/triton.sh" "$d" > "$OUT/kernels-${d##*/}.log" 2>&1 || fail "${d##*/}'s Triton sets did not build: $(tail -c 400 "$OUT/kernels-${d##*/}.log")"
  say "kernels ${d##*/}: $(find "$d" -path '*/_triton/manifest.json' -printf '%P ' 2>/dev/null)built in $((SECONDS - t0)) s"
}
# floor FILE KINDS: the transfer of KINDS (`downloading`, `uploading|downloading`) FILE reports last, when it is a link's
# measure (16 MiB or more; a result's never) still in progress and under FLOOR_MBS MB/s. A finished or small transfer
# judges nothing: a run's small input uploads report a few KiB/s.
floor() {
  grep -aoE "($2) .{0,160} [0-9.]+ (B|KiB|MiB|GiB)/[0-9.]+ (B|KiB|MiB|GiB) at [0-9.]+ (B|KiB|MiB|GiB)/s" "$1" |
    grep -v 'the result' | awk -v f="$FLOOR_MBS" '
      function b(x, u) { return x * (u == "GiB" ? 2^30 : u == "MiB" ? 2^20 : u == "KiB" ? 2^10 : 1) }
      { n = split($0, w, /[ \/]+/); done = b(w[n - 7], w[n - 6]); total = b(w[n - 5], w[n - 4])
        if (total >= 16 * 2^20) { last = $0; rate = b(w[n - 2], w[n - 1]) / 1e6; open = done < total } }
      END { if (last != "" && open && rate < f) { printf "%.1f MB/s, under the %s MB/s floor (%s)", rate, f, last; exit 0 } exit 1 }'
}
# heard SLOW|ended: whether a slow host can end now, and why (HEARD). The Hub learns a host is slow only from the pod's own
# speed report, which TensorD sends once 1 GiB of weights has arrived (machine/speed.rs): a weights download in flight is
# let reach it (the guard still ends the pod at the cap); a slow upload with no report sent ends at once, unrecorded.
heard() {
  local line
  line=$(cozy machine logs "$NAME" --since 6h -n 0 2>/dev/null |
    grep -aoE "reported this host's download speed to the Hub|the Hub took no download speed report: .{0,200}" | tail -1)
  case $line in
    reported*) HEARD="its speed report reached the Hub" ;;
    the\ Hub*) HEARD="${line}" ;;
    *) [ "$1" != ended ] || { HEARD="host not recorded slow at the Hub (its step ended before a speed report)"; return 0; }
       [[ $1 != *downloading* ]] || return 1
       HEARD="host not recorded slow at the Hub (a slow upload, no speed report sent)" ;;
  esac
}
step() {
  local n=$1 directory=$PWD; shift
  local words=("$@")
  if [ "$1" = run ]; then
    [[ ${words[1]:-} != .* ]] || words[1]="$PWD/${words[1]}"
    directory=$RUN_PROJECT
  elif [ "$1" = model ] && [ "${2:-}" = quantize ]; then
    # A recipe from local code runs in a project (`project_required`): the session's, its package by absolute path.
    local i
    for i in "${!words[@]}"; do
      [ "${words[$i]}" = --package ] && [[ ${words[$((i + 1))]:-} == .* ]] && words[$((i + 1))]="$PWD/${words[$((i + 1))]}"
    done
    directory=$RUN_PROJECT
  fi
  # A package run or installed from a packages checkout: its kernels first.
  if [ "$1" = run ] || { [ "$1" = package ] && [ "${2:-}" = install ]; }; then
    local t=${words[1]}; [ "$1" = run ] || t=${words[2]:-}
    case $t in /* | .*) if [ -d "$t" ]; then kernels "$t"; else kernels "$(dirname "$t")"; fi ;; esac
  fi
  say "step $n: cozy ${words[*]}"
  # The floor judges the pod's link: a machine update's uploads to it, and the weights a step on it downloads.
  local kinds=''
  if [ "$1" = machine ] && [ "${2:-}" = update ]; then kinds='uploading|downloading'; elif [[ " ${words[*]} " == *" --on "* ]]; then kinds=downloading; fi
  (cd "$directory" && cozy "${words[@]}") > "$OUT/$n.txt" 2>&1 & local p=$! last=-1 still=0 size t=0 slow=''
  while kill -0 "$p" 2>/dev/null; do
    sleep 1; t=$((t + 1))
    if [ "$FLOOR_MBS" != 0 ] && [ -n "$kinds" ] && [ "$t" -ge "$FLOOR_AFTER" ] && [ $((t % 2)) = 0 ]; then
      if [ -z "$slow" ] && slow=$(floor "$OUT/$n.txt" "$kinds"); then
        say "$n: host too slow: $slow on $NAME ($RID)"; [[ $slow == *downloading* ]] && say "$n: its download goes on until the pod's log shows its speed report (checked every 30 s)"
      fi
      if [ -n "$slow" ] && { [ $((t % 30)) = 0 ] || [[ $slow != *downloading* ]]; } && heard "$slow"; then
        kill "$p"; fail "host too slow: $(floor "$OUT/$n.txt" "$kinds" || echo "$slow") on $NAME ($RID): $HEARD"
      fi
    fi
    [ $((t % 30)) = 0 ] || continue
    size=$(stat -c %s "$OUT/$n.txt")
    if [ "$size" = "$last" ]; then still=$((still + 30)); else still=0; last=$size; fi
    [ "$still" -lt 900 ] || { kill "$p"; say "$n: no output for 15 min"; return 124; }
  done
  [ -z "$slow" ] || { heard ended; fail "host too slow: $slow on $NAME ($RID): $HEARD"; }
  wait "$p"; local c=$?
  tail -6 "$OUT/$n.txt" | cut -c1-300 | sed 's/^/    /' >> "$R"
  return $c
}

# guard: from the first order on, every rental this session ordered ($OUT/rental, by its recorded rental ids, and what $GUARD_ALSO
# prints) is ended once what the Hub bills for them would reach $CAP by the next look and an end: what each has cost so far, the
# providers' if that's more, and each live one at the price an hour it's locked at. Read from the Hub every GUARD_STEP s, never from a
# quote. No read for GUARD_BLIND s ends them too. Nothing else in the account is touched: other sessions' pods are in the same listing.
GUARD_STEP=20 GUARD_END_S=120 GUARD_BLIND=300 GUARD_ALSO=''
guard() {
  [ -z "$WD" ] || return 0
  ( blind=0
    while :; do
      sleep "$GUARD_STEP"
      ids=$({ cat "$OUT/rental" 2> /dev/null; [ -z "$GUARD_ALSO" ] || $GUARD_ALSO; } | sort -u | jq -Rsc 'split("\n") | map(select(length > 0))')
      [ "$ids" != '[]' ] || continue
      if spent=$(timeout 60 cozy machine ls --since 24h --json < /dev/null 2> /dev/null | jq -e --argjson ids "$ids" \
          '[.rentals[] | select(.rental_id as $r | any($ids[]; . == $r)) | [.spend_usd_micros // 0, .provider_cost_usd_micros // 0] | max] | add // 0' 2> /dev/null) &&
        rate=$(machines_json | jq -e --argjson ids "$ids" \
          '[.machines[] | select(.state != "ended" and (.rental_id as $r | .name as $n | any($ids[]; . == $r or . == $n))) | .rental.usd_micros_per_hour // 0] | add // 0' 2> /dev/null); then
        blind=0
        echo "$(date -u +%T) billed $spent, live $rate/h (micro-USD) of $(jq -r 'join(" ")' <<<"$ids")" >> "$OUT/guard.log"
        awk -v b="$spent" -v r="$rate" -v c="$CAP" -v s=$((GUARD_STEP + GUARD_END_S)) 'BEGIN { exit !(b + r * s / 3600 >= c * 1e6) }' || continue
        why="\$$CAP reached: the Hub bills $(awk -v b="$spent" 'BEGIN { printf "$%.4f", b / 1e6 }') so far and $(awk -v r="$rate" 'BEGIN { printf "$%.2f", r / 1e6 }')/h live"
      else
        blind=$((blind + GUARD_STEP))
        [ "$blind" -ge "$GUARD_BLIND" ] || continue
        why="no read of what the Hub bills for ${blind}s"
      fi
      echo "$(date -u +%T) GUARD: $why, ending $(jq -r 'join(" ")' <<<"$ids")" >> "$R"
      jq -r '.[]' <<<"$ids" | while read -r r; do cozy machine end "$r" -y --force >> "$OUT/end.txt" 2>&1; done
      exit 0
    done ) 9>&- & WD=$!
}

# billed <rental id>: what the Hub bills for it, and what its provider does, from the account's rentals of the last day.
billed() {
  timeout 60 cozy machine ls --since 24h --json < /dev/null 2> /dev/null | jq -r --arg r "$1" \
    '.rentals[] | select(.rental_id == $r) | "\(.spend_usd_micros // 0) micro-USD \(.spend_basis // "?"), the provider \(.provider_cost_usd_micros // "?") \(.provider_cost_basis // ""), \(.ran_s // "?") s"'
}

# quote <seconds> [SKU[:GPUS]…]: the quote and the guard for one rental of each SKU named ($SKU by default; GPUS of it, else one):
# the Hub's one price an hour, the machine with its disk and traffic, which its order carries as its cap; fails unless CAP buys the
# seconds. Ends the $0 pre-flight unless RENT=true.
quote() {
  local need=$1; shift
  local skus=("${@:-$SKU}") s price hourly=0 said='' f gpus
  say "cozy $(cozy --version); $(cozy whoami 2>&1 | grep -E '^account')"
  ! listed pr-pod-session-probe || fail "gone-check: a rental that never existed reads live (no machine listing answers?)"
  for s in "${skus[@]}"; do
    # A SKU no provider has is often offered again within minutes: asked again, six times over about 6 min, as `order` does.
    local try=1 pause=15
    f=$OUT/quote-${s/:/x}.json gpus=(); [[ $s != *:* ]] || gpus=(--gpus "${s#*:}")
    while cozy machine rent "${s%%:*}" "${gpus[@]}" --disk-gb "$DISK" $RENT_ARGS --json < /dev/null > "$f" 2>&1 || true
      grep -q sku_out_of_stock "$f" && [ "$try" -lt 6 ]; do
      say "no provider has $s now (attempt $try/6): asking again in ${pause}s"
      sleep "$pause"; try=$((try + 1)); pause=$((pause * 2 > 120 ? 120 : pause * 2))
    done
    grep -q quote_unagreed "$f" || fail "a quote without --yes did not refuse: $(tail -c 300 "$f")"
    price=$(jq -r 'select(.quote) | .quote.usd_micros_per_hour' "$f" 2>/dev/null | head -1)
    [ -n "$price" ] || fail "no quote for $s: $(tail -c 300 "$f")"
    hourly=$((hourly + price)); said="$said $s $price"
  done
  cp "$OUT/quote-${skus[0]/:/x}.json" "$OUT/quote.json"
  CAP_S=$(awk -v c="$CAP" -v h="$hourly" 'BEGIN{printf "%d", c*1e6 / h * 3600}')
  [ "$CAP_S" -ge "$need" ] || fail "\$$CAP leaves only ${CAP_S}s for$said micro-USD/h"
  say "quote$said micro-USD/h, $DISK GB each: \$$CAP buys about ${CAP_S}s; the guard ends them by what the Hub bills"
  [ "$RENT" = true ] || { say "pre-flight only: nothing rented"; exit 0; }
}

# accepted <file>: the rental id `cozy --json machine rent` prints the moment its Hub accepts the order (`rental.accepted`).
# An order waiting for capacity has it long before its machine has a name (cl-979).
accepted() { grep -m1 '"type":"rental.accepted"' "$1" 2>/dev/null | jq -r '.rental_id // empty' 2>/dev/null; }

# order [SKU]: one rental ($SKU by default) ordered, its boot followed in the background; `ready` waits for it. rent =
# order + ready. Its rental id goes into RIDS and $OUT/rental once its Hub accepts it, so the trap, the guard (the first
# order starts it) and the workflow's cleanup end it whether or not it was ever placed.
order() {
  local i=${#RIDS[@]} RP sku=${1:-$SKU} try=1 pause=15 r
  # A SKU no provider has is often offered again within minutes: the order is placed again, six times over about 6 min.
  while :; do
    # The rent never holds a session's rent lock (fd 9): the session does, until its Hub accepts the order.
    cozy --json machine rent "$sku" --disk-gb "$DISK" $RENT_ARGS -y > "$OUT/rent$i.out" 2> "$OUT/rent$i.err" 9>&- & RP=$!
    until r=$(accepted "$OUT/rent$i.out"); [ -n "$r" ] || ! kill -0 "$RP" 2>/dev/null; do sleep 1; done
    [ -n "$r" ] || r=$(accepted "$OUT/rent$i.out")
    [ -z "$r" ] || break
    wait "$RP"
    [ "$try" -lt 6 ] && grep -qs sku_out_of_stock "$OUT/rent$i.out" "$OUT/rent$i.err" ||
      fail "the order was not accepted: $(tail -c 300 "$OUT/rent$i.out") $(tail -3 "$OUT/rent$i.err")"
    say "no provider has $sku now (attempt $try/6): ordering again in ${pause}s"
    sleep "$pause"; try=$((try + 1)); pause=$((pause * 2 > 120 ? 120 : pause * 2))
  done
  RID=$r; RIDS+=("$RID"); echo "$RID" >> "$OUT/rental"; eval "RP_$i=$RP"
  say "rental $RID ordered"
  guard
}

# ready [i]: the i-th order (the last by default) booted: RID and NAME are its, its record in show<i>.json/txt (the
# first's also in show.json/txt).
ready() {
  local i=${1:-$((${#RIDS[@]} - 1))} p RRC
  RID=${RIDS[$i]}; p=RP_$i
  wait "${!p}"; RRC=$?
  [ "$RRC" = 0 ] || fail "boot failed: $(tail -c 300 "$OUT/rent$i.out") $(tail -3 "$OUT/rent$i.err")"
  [ "$(state "$RID")" = ready ] || fail "$RID is $(state "$RID"), not ready"
  cozy machine show "$RID" --json > "$OUT/show$i.json" 2>&1; cozy machine show "$RID" > "$OUT/show$i.txt" 2>&1
  [ "$i" != 0 ] || { cp "$OUT/show0.json" "$OUT/show.json"; cp "$OUT/show0.txt" "$OUT/show.txt"; }
  NAME=$(jq -r .name "$OUT/show$i.json")
  [ "$(jq -r .rental_id "$OUT/show$i.json")" = "$RID" ] && [ -n "$NAME" ] || fail "show $RID: $(head -c 300 "$OUT/show$i.json")"
  say "READY $RID as $NAME, image $(jq -r .rental.base_worker_image_digest "$OUT/show$i.json")"
  grep -E '^(version|runtime|tensorfs|GPU 0|disk free) ' "$OUT/show$i.txt" | tee -a "$R"
}

# rent: one rental, ordered and booted ($RID, its machine $NAME), under the guard.
rent() { order; ready; }
