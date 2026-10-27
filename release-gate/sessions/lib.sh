# release-gate/sessions/lib.sh: what every session shares (pod-session.yaml). A session sources it, checks what it
# will run, calls `quote <traffic GB> <seconds it needs> [pods]` (the $0 pre-flight ends there unless RENT=true), then
# `rent` once per pod (or `order` each, then `ready` each, to boot them together), and works on $NAME; the EXIT trap
# ends every rental by its own rental id and checks each is gone. RENT_ARGS: more `machine rent` flags (`--ssh`).
# From the workflow: OUT, SKU, DISK, CAP (USD, all in), RENT, PARAMS (a JSON object), PACKAGES (packages/<name>
# checkouts), REF (release-gate/ref).
# Every failure is handled here (the trap ends the rental), not by bash -e.
set +e -uo pipefail
mkdir -p "$OUT"; R=$OUT/results.txt
say() { echo "$(date -u +%T) $*" | tee -a "$R"; }
RC=0 RID='' NAME='' WD='' BG='' RENT_ARGS=''  # BG: a session's own background loop, ended with it
RIDS=()  # every rental this session ordered; $OUT/rental lists them, one a line
fail() { say "FAIL: $*"; RC=1; exit 1; }
# param <key> [default]: the session's setting from PARAMS (a non-string as JSON).
param() { jq -r --arg k "$1" --arg d "${2-}" 'if has($k) then (.[$k] | if type == "string" then . else tojson end) else $d end' <<<"$PARAMS"; }
state() { cozy machine show "$1" --json 2>/dev/null | jq -r '.state // empty'; }
# live <rental id>: listed and not ended; a list that does not answer counts as live.
live() {
  local j; j=$(cozy machine list --json 2>/dev/null) || return 0
  jq -e --arg r "$1" '.machines[] | select(.rental_id==$r and .state!="ended")' <<<"$j" >/dev/null
}
gone() { local s; s=$(state "$1"); { [ "$s" = ended ] || [ -z "$s" ]; } && ! live "$1"; }
# runno <name>: the run a step's output names last (its own, after a local source's sync run).
runno() { awk '$1=="run" && NF==2{print $2}' "$OUT/$1.txt" | tail -1; }
finish() {
  local status=$?; [ "$RC" != 0 ] || RC=$status
  for p in $WD $BG; do pkill -P "$p"; kill "$p"; done 2>/dev/null
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
    say "GONE $r: ended, and no live machine in cozy machine list holds it"
  done
  say "RESULT $([ "$RC" = 0 ] && echo PASSED || echo FAILED)"
  exit "$RC"
}
trap finish EXIT
trap 'RC=1; exit 1' INT TERM HUP
# step <name> <cozy args…>: one call followed to its end; output that stops growing for 15 min cancels it.
step() {
  local n=$1; shift; say "step $n: cozy $*"
  cozy "$@" > "$OUT/$n.txt" 2>&1 & local p=$! last=-1 still=0 size
  while kill -0 "$p" 2>/dev/null; do
    sleep 30; size=$(stat -c %s "$OUT/$n.txt")
    if [ "$size" = "$last" ]; then still=$((still + 30)); else still=0; last=$size; fi
    [ "$still" -lt 900 ] || { kill "$p"; say "$n: no output for 15 min"; return 124; }
  done
  wait "$p"; local c=$?
  tail -6 "$OUT/$n.txt" | cut -c1-300 | sed 's/^/    /' >> "$R"
  return $c
}

# quote <traffic GB> <seconds> [pods]: the quote and the guard (GPU + disk an hour from the offer for each of `pods`, 1
# by default, and the provider's traffic at its rate); fails unless CAP buys the seconds. Ends the $0 pre-flight unless
# RENT=true.
quote() {
  say "cozy $(cozy --version); $(cozy whoami 2>&1 | grep -E '^account')"
  ! live pr-pod-session-probe || fail "gone-check: a rental that never existed reads live"
  cozy machine rent --json > "$OUT/skus.json" 2>&1 || fail "no catalog"
  local storage bw gpu disk
  read -r storage bw < <(jq -r --arg s "$SKU" 'if type=="array" then .[] else . end | select(.name==$s) | .widths[] | select(.accelerator_count==1) |
    [(.storage_usd_micros_per_hour / .container_disk_gb), (.bandwidth_usd_micros_per_gb // 0)] | @tsv' "$OUT/skus.json")
  [ -n "${bw:-}" ] || fail "the catalog offers no single $SKU"
  cozy machine rent "$SKU" --disk-gb "$DISK" $RENT_ARGS --json < /dev/null > "$OUT/quote.json" 2>&1
  grep -q quote_unagreed "$OUT/quote.json" || fail "a quote without --yes did not refuse: $(tail -c 300 "$OUT/quote.json")"
  gpu=$(jq -r 'select(.quote) | .quote.usd_micros_per_hour' "$OUT/quote.json" 2>/dev/null | head -1)
  [ -n "$gpu" ] || fail "no quote: $(tail -c 300 "$OUT/quote.json")"
  disk=$(awk -v s="$storage" -v d="$DISK" 'BEGIN{printf "%d", s*d}')
  local n=${3:-1}
  CAP_S=$(awk -v c="$CAP" -v g="$gpu" -v d="$disk" -v w="$bw" -v gb="$1" -v n="$n" 'BEGIN{printf "%d", (c*1e6 - w*gb) / (n * (g + d)) * 3600}')
  [ "$CAP_S" -ge "$2" ] || fail "\$$CAP leaves only ${CAP_S}s for $n at GPU $gpu + disk $disk micro-USD/h and $bw/GB x $1 GB"
  say "quote $n x $SKU, $DISK GB: GPU $gpu + disk $disk micro-USD/h each, traffic $bw/GB x $1 GB; guard ends them ${CAP_S}s after the first order (\$$CAP)"
  [ "$RENT" = true ] || { say "pre-flight only: nothing rented"; exit 0; }
}

# order: one rental ordered (its boot followed in the background); `ready` waits for it. rent = order + ready.
# The first order starts the guard, which ends every rental in $OUT/rental once CAP_S has passed.
order() {
  local o first='' i=${#RIDS[@]} RP
  o=$(date +%s%3N)
  cozy machine rent "$SKU" --disk-gb "$DISK" $RENT_ARGS -y > "$OUT/rent$i.out" 2> "$OUT/rent$i.err" & RP=$!
  while [ -z "$first" ]; do
    first=$(grep -m1 -oE '^[a-z0-9][a-z0-9-]*: (booting|ready|offline)' "$OUT/rent$i.err" | cut -d: -f1)
    [ -n "$first" ] || kill -0 "$RP" 2>/dev/null || break
    sleep 2
  done
  local done=''  # the order's exit, when it ended before naming its machine
  [ -n "$first" ] || { wait "$RP"; done=$?; [ "$done" = 0 ] && first=$(awk '$1=="machine"{print $2; exit}' "$OUT/rent$i.out"); }
  [ -n "$first" ] || fail "the rent named no machine: $(tail -3 "$OUT/rent$i.err")"
  cozy machine show "$first" --json > "$OUT/show-boot$i.json" 2>&1
  RID=$(jq -r '.rental_id // empty' "$OUT/show-boot$i.json")
  jq -e --arg s "$SKU" --argjson o "$o" '.rental.sku==$s and .rental.rented_at_ms>=$o-60000' "$OUT/show-boot$i.json" >/dev/null ||
    { RID=''; fail "$first is not this order's machine: $(head -c 300 "$OUT/show-boot$i.json")"; }
  RIDS+=("$RID"); echo "$RID" >> "$OUT/rental"; eval "RP_$i=$RP RRC_$i=$done"
  say "rental $RID ($first)"
  [ -n "$WD" ] || { ( sleep "$CAP_S"; echo "$(date -u +%T) GUARD: \$$CAP reached, ending $(paste -sd' ' "$OUT/rental")" >> "$R"
    while read -r r; do cozy machine end "$r" -y --force >> "$OUT/end.txt" 2>&1; done < "$OUT/rental" ) & WD=$!; }
}

# ready [i]: the i-th order (the last by default) booted: RID and NAME are its, its record in show<i>.json/txt (the
# first's also in show.json/txt).
ready() {
  local i=${1:-$((${#RIDS[@]} - 1))} p c
  RID=${RIDS[$i]}; p=RP_$i c=RRC_$i
  local RRC=${!c}
  [ -n "$RRC" ] || { wait "${!p}"; RRC=$?; }
  [ "$RRC" = 0 ] || fail "boot failed: $(tail -3 "$OUT/rent$i.err")"
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
