# release-gate/sessions/lib.sh: what every session shares (pod-session.yaml). A session sources it, checks what it
# will run, calls `quote <traffic GB> <seconds it needs>` (the $0 pre-flight ends there unless RENT=true), then `rent`,
# and works on $NAME; the EXIT trap ends the rental by its own rental id and checks it is gone.
# From the workflow: OUT, SKU, DISK, CAP (USD, all in), RENT, PARAMS (a JSON object), PACKAGES (packages/<name>
# checkouts), REF (release-gate/ref).
# Every failure is handled here (the trap ends the rental), not by bash -e.
set +e -uo pipefail
mkdir -p "$OUT"; R=$OUT/results.txt
say() { echo "$(date -u +%T) $*" | tee -a "$R"; }
RC=0 RID='' NAME='' WD='' BG=''  # BG: a session's own background loop, ended with it
fail() { say "FAIL: $*"; RC=1; exit 1; }
# param <key> [default]: the session's setting from PARAMS (a non-string as JSON).
param() { jq -r --arg k "$1" --arg d "${2-}" 'if has($k) then (.[$k] | if type == "string" then . else tojson end) else $d end' <<<"$PARAMS"; }
GPUS=$(param gpus 1)  # GPUs in the rental (params.gpus)
[[ $GPUS =~ ^[1-8]$ ]] || { echo "gpus must be 1-8"; exit 1; }
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
  if [ -n "$RID" ]; then
    local tries=0
    until gone "$RID"; do
      if [ $((tries % 12)) = 0 ]; then
        cozy machine end "$RID" -y >> "$OUT/end.txt" 2>&1 || cozy machine end "$RID" -y --force >> "$OUT/end.txt" 2>&1
        say "END requested $RID"
      fi
      tries=$((tries + 1)); sleep 10
    done
    say "GONE $RID ($NAME): ended, and no live machine in cozy machine list holds it"
  fi
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

# quote <traffic GB> <seconds>: the quote and the guard (GPU + disk an hour from the offer, the provider's traffic at
# its rate); fails unless CAP buys the seconds. Ends the $0 pre-flight unless RENT=true.
quote() {
  say "cozy $(cozy --version); $(cozy whoami 2>&1 | grep -E '^account')"
  ! live pr-pod-session-probe || fail "gone-check: a rental that never existed reads live"
  cozy machine rent --json > "$OUT/skus.json" 2>&1 || fail "no catalog"
  local storage bw gpu disk
  read -r storage bw < <(jq -r --arg s "$SKU" --arg g "$GPUS" 'if type=="array" then .[] else . end | select(.name==$s) | .widths[] | select(.accelerator_count==($g|tonumber)) |
    [(.storage_usd_micros_per_hour / .container_disk_gb), (.bandwidth_usd_micros_per_gb // 0)] | @tsv' "$OUT/skus.json")
  [ -n "${bw:-}" ] || fail "the catalog offers no $GPUS x $SKU"
  cozy machine rent "$SKU" --gpus "$GPUS" --disk-gb "$DISK" --json < /dev/null > "$OUT/quote.json" 2>&1
  grep -q quote_unagreed "$OUT/quote.json" || fail "a quote without --yes did not refuse: $(tail -c 300 "$OUT/quote.json")"
  gpu=$(jq -r 'select(.quote) | .quote.usd_micros_per_hour' "$OUT/quote.json" 2>/dev/null | head -1)
  [ -n "$gpu" ] || fail "no quote: $(tail -c 300 "$OUT/quote.json")"
  disk=$(awk -v s="$storage" -v d="$DISK" 'BEGIN{printf "%d", s*d}')
  CAP_S=$(awk -v c="$CAP" -v g="$gpu" -v d="$disk" -v w="$bw" -v gb="$1" 'BEGIN{printf "%d", (c*1e6 - w*gb) / (g + d) * 3600}')
  [ "$CAP_S" -ge "$2" ] || fail "\$$CAP leaves only ${CAP_S}s at GPU $gpu + disk $disk micro-USD/h and $bw/GB x $1 GB"
  say "quote $GPUS x $SKU, $DISK GB: GPU $gpu + disk $disk micro-USD/h, traffic $bw/GB x $1 GB; guard ends it ${CAP_S}s after the order (\$$CAP)"
  [ "$RENT" = true ] || { say "pre-flight only: nothing rented"; exit 0; }
}

# rent: the rental this order made, by its own rental id ($RID, its machine $NAME), and the guard.
rent() {
  local o first='' RRC='' RP
  o=$(date +%s%3N)
  cozy machine rent "$SKU" --gpus "$GPUS" --disk-gb "$DISK" -y > "$OUT/rent.out" 2> "$OUT/rent.err" & RP=$!
  while [ -z "$first" ]; do
    first=$(grep -m1 -oE '^[a-z0-9][a-z0-9-]*: (booting|ready|offline)' "$OUT/rent.err" | cut -d: -f1)
    [ -n "$first" ] || kill -0 "$RP" 2>/dev/null || break
    sleep 2
  done
  [ -n "$first" ] || { wait "$RP"; RRC=$?; [ "$RRC" = 0 ] && first=$(awk '$1=="machine"{print $2; exit}' "$OUT/rent.out"); }
  [ -n "$first" ] || fail "the rent named no machine: $(tail -3 "$OUT/rent.err")"
  cozy machine show "$first" --json > "$OUT/show-boot.json" 2>&1
  RID=$(jq -r '.rental_id // empty' "$OUT/show-boot.json")
  jq -e --arg s "$SKU" --argjson o "$o" --argjson g "$GPUS" '.rental.sku==$s and .rental.gpus==$g and .rental.rented_at_ms>=$o-60000' "$OUT/show-boot.json" >/dev/null ||
    { RID=''; fail "$first is not this order's machine: $(head -c 300 "$OUT/show-boot.json")"; }
  echo "$RID" > "$OUT/rental"
  say "rental $RID ($first)"
  ( sleep "$CAP_S"; echo "$(date -u +%T) GUARD: \$$CAP reached, ending $RID" >> "$R"; cozy machine end "$RID" -y --force >> "$OUT/end.txt" 2>&1 ) & WD=$!
  [ -n "$RRC" ] || { wait "$RP"; RRC=$?; }
  [ "$RRC" = 0 ] || fail "boot failed: $(tail -3 "$OUT/rent.err")"
  [ "$(state "$RID")" = ready ] || fail "$RID is $(state "$RID"), not ready"
  cozy machine show "$RID" --json > "$OUT/show.json" 2>&1; cozy machine show "$RID" > "$OUT/show.txt" 2>&1
  NAME=$(jq -r .name "$OUT/show.json")
  [ "$(jq -r .rental_id "$OUT/show.json")" = "$RID" ] && [ -n "$NAME" ] || fail "show $RID: $(head -c 300 "$OUT/show.json")"
  say "READY $RID as $NAME, image $(jq -r .rental.base_worker_image_digest "$OUT/show.json")"
  grep -E '^(version|runtime|tensorfs|GPU 0|disk free) ' "$OUT/show.txt" | tee -a "$R"
}
