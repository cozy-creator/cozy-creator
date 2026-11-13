# release-gate/sessions/lib.sh: what every session shares (pod-session.yaml). A session sources it, checks what it
# will run, calls `quote <seconds it needs> [pods]` (the $0 pre-flight ends there unless RENT=true), then
# `rent` once per pod (or `order` each, then `ready` each, to boot them together), and works on $NAME; the EXIT trap
# ends every rental by its own rental id and checks each is gone. RENT_ARGS: more `machine rent` flags (`--ssh`).
# From the workflow: OUT, SKU, DISK, CAP (USD, all in), RENT, PARAMS (a JSON object), PACKAGES (packages/<name>
# checkouts), REF (release-gate/ref), HUB (prod or isolated: e2e/test-hub.sh require_ci_rental_hub).
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
RC=0 RID='' NAME='' WD='' BG='' RENT_ARGS=''  # BG: a session's own background loop, ended with it
RIDS=()  # every rental this session ordered; $OUT/rental lists them, one a line
fail() { say "FAIL: $*"; RC=1; exit 1; }
# param <key> [default]: the session's setting from PARAMS (a non-string as JSON).
param() { jq -r --arg k "$1" --arg d "${2-}" 'if has($k) then (.[$k] | if type == "string" then . else tojson end) else $d end' <<<"$PARAMS"; }
state() { cozy machine show "$1" --json 2>/dev/null | jq -r '.state // empty'; }
. "$(dirname "${BASH_SOURCE[0]}")/../machines.sh"
gone() { local s; s=$(state "$1"); { [ "$s" = ended ] || [ -z "$s" ]; } && ! listed "$1"; }
# made <name>: the checkpoint a step's upload or quantize made, as sha256:<hex>: its `checkpoint <org/name>@sha256:…` line
# (a machine's own `checkpoint sha256:…` line names no model); cozy 0.24's quantize says `commit`.
made() { awk '($1=="checkpoint" || $1=="commit") && $2 ~ /@sha256:/ {c=$2} END{sub(/.*@/, "", c); print c}' "$OUT/$1.txt"; }
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
    say "GONE $r: ended, and the machine listing holds no live machine of it; billed $(billed "$r")"
  done
  say "RESULT $([ "$RC" = 0 ] && echo PASSED || echo FAILED)"
  exit "$RC"
}
trap finish EXIT
trap 'RC=1; exit 1' INT TERM HUP
# step <name> <cozy args…>: one call followed to its end (looked at each second, its output every 30 s); output that
# stops growing for 15 min cancels it.
# A run belongs to this session's working-directory project.
RUN_PROJECT=$OUT/project
# A folder is a project once `cozy new` made it one (0.24: `status` reads one, it makes none).
. "$(dirname "${BASH_SOURCE[0]}")/../../e2e/run.sh"
{ run_project "$RUN_PROJECT" && (cd "$RUN_PROJECT" && "$RUN_CLI" --json status); } > "$OUT/project.json" 2>> "$OUT/project.err" \
  || fail "project initialization: $(cat "$OUT/project.err") $(tail -c 300 "$OUT/project.json")"
step() {
  local n=$1 directory=$PWD; shift
  local words=("$@")
  if [ "$1" = run ]; then
    [[ ${words[1]:-} != .* ]] || words[1]="$PWD/${words[1]}"
    directory=$RUN_PROJECT
  fi
  say "step $n: cozy ${words[*]}"
  (cd "$directory" && cozy "${words[@]}") > "$OUT/$n.txt" 2>&1 & local p=$! last=-1 still=0 size t=0
  while kill -0 "$p" 2>/dev/null; do
    sleep 1; t=$((t + 1)); [ $((t % 30)) = 0 ] || continue
    size=$(stat -c %s "$OUT/$n.txt")
    if [ "$size" = "$last" ]; then still=$((still + 30)); else still=0; last=$size; fi
    [ "$still" -lt 900 ] || { kill "$p"; say "$n: no output for 15 min"; return 124; }
  done
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

# quote <seconds> [SKU…]: the quote and the guard for one rental of each SKU named ($SKU by default): the Hub's one price
# an hour, the machine with its disk and traffic, which its order carries as its cap; fails unless CAP buys the seconds.
# Ends the $0 pre-flight unless RENT=true.
quote() {
  local need=$1; shift
  local skus=("${@:-$SKU}") s price hourly=0 said=''
  say "cozy $(cozy --version); $(cozy whoami 2>&1 | grep -E '^account')"
  ! listed pr-pod-session-probe || fail "gone-check: a rental that never existed reads live (no machine listing answers?)"
  for s in "${skus[@]}"; do
    cozy machine rent "$s" --disk-gb "$DISK" $RENT_ARGS --json < /dev/null > "$OUT/quote-$s.json" 2>&1
    grep -q quote_unagreed "$OUT/quote-$s.json" || fail "a quote without --yes did not refuse: $(tail -c 300 "$OUT/quote-$s.json")"
    price=$(jq -r 'select(.quote) | .quote.usd_micros_per_hour' "$OUT/quote-$s.json" 2>/dev/null | head -1)
    [ -n "$price" ] || fail "no quote for $s: $(tail -c 300 "$OUT/quote-$s.json")"
    hourly=$((hourly + price)); said="$said $s $price"
  done
  cp "$OUT/quote-${skus[0]}.json" "$OUT/quote.json"
  CAP_S=$(awk -v c="$CAP" -v h="$hourly" 'BEGIN{printf "%d", c*1e6 / h * 3600}')
  [ "$CAP_S" -ge "$need" ] || fail "\$$CAP leaves only ${CAP_S}s for$said micro-USD/h"
  say "quote$said micro-USD/h, $DISK GB each: \$$CAP buys about ${CAP_S}s; the guard ends them by what the Hub bills"
  [ "$RENT" = true ] || { say "pre-flight only: nothing rented"; exit 0; }
}

# order [SKU]: one rental ($SKU by default) ordered, its boot followed in the background; `ready` waits for it. rent =
# order + ready. The first order starts the guard (`guard`).
order() {
  local o first i=${#RIDS[@]} RP sku=${1:-$SKU} try=1 pause=15
  local done  # the order's exit, when it ended before naming its machine
  # A SKU no provider has is often offered again within minutes: the order is placed again, six times over about 6 min.
  while :; do
    o=$(date +%s%3N) first='' done=''
    # Nothing long-lived holds a session's rent lock (fd 9): only the order does, while it names its machine.
    cozy machine rent "$sku" --disk-gb "$DISK" $RENT_ARGS -y > "$OUT/rent$i.out" 2> "$OUT/rent$i.err" 9>&- & RP=$!
    while [ -z "$first" ]; do
      first=$(grep -m1 -oE '^[a-z0-9][a-z0-9-]*: (booting|ready|offline)' "$OUT/rent$i.err" | cut -d: -f1)
      [ -n "$first" ] || kill -0 "$RP" 2>/dev/null || break
      sleep 2
    done
    [ -n "$first" ] || { wait "$RP"; done=$?; [ "$done" = 0 ] && first=$(awk '$1=="machine"{print $2; exit}' "$OUT/rent$i.out"); }
    [ -z "$first" ] && [ "$try" -lt 6 ] && grep -q sku_out_of_stock "$OUT/rent$i.err" || break
    say "no provider has $sku now (attempt $try/6): ordering again in ${pause}s"
    sleep "$pause"; try=$((try + 1)); pause=$((pause * 2 > 120 ? 120 : pause * 2))
  done
  [ -n "$first" ] || fail "the rent named no machine: $(tail -3 "$OUT/rent$i.err")"
  cozy machine show "$first" --json > "$OUT/show-boot$i.json" 2>&1
  RID=$(jq -r '.rental_id // empty' "$OUT/show-boot$i.json")
  jq -e --arg s "$sku" --argjson o "$o" '.rental.sku==$s and .rental.rented_at_ms>=$o-60000' "$OUT/show-boot$i.json" >/dev/null ||
    { RID=''; fail "$first is not this order's machine: $(head -c 300 "$OUT/show-boot$i.json")"; }
  RIDS+=("$RID"); echo "$RID" >> "$OUT/rental"; eval "RP_$i=$RP RRC_$i=$done"
  say "rental $RID ($first)"
  guard
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
