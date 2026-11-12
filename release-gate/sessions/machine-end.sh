#!/usr/bin/env bash
# machine-end (cl-946): `cozy machine end` says one truth. `pods` cheap rentals are ordered, booted, then each ended with
# `cozy machine end <name>` as a person types it; what it printed (its exit, "shut down", or an error naming the rental) must
# match its Hub's record of that rental (`machine ls --since 24h`: `released`/`failed` with an end time only once its provider
# has no pod, which the Hub proves before it says so) and Calcifer's (`machine show`, `machine ls`). An end said done that the
# Hub has not ended, or an end said failed that the Hub ended, fails the session. Params: {"pods": 3}.
. "$(dirname "$0")/lib.sh"
PODS=$(param pods 3)
[ "$PODS" -ge 1 ] || fail "pods: $PODS"
quote $((PODS * 900)) $(for _ in $(seq "$PODS"); do echo "$SKU"; done)
for _ in $(seq "$PODS"); do order; done
for i in $(seq 0 $((PODS - 1))); do ready "$i"; done

# hub <rental id>: the Hub's record of it, `<state> <ended_at_ms or none>`.
hub() {
  timeout 60 cozy machine ls --since 24h --json < /dev/null 2> /dev/null |
    jq -r --arg r "$1" '.rentals[] | select(.rental_id == $r) | "\(.state) \(.ended_at_ms // "none")"' | head -1
}
for i in $(seq 0 $((PODS - 1))); do
  rid=${RIDS[$i]} name=$(jq -r .name "$OUT/show$i.json") t0=$(date +%s)
  cozy machine end "$name" -y > "$OUT/end$i.out" 2> "$OUT/end$i.err"; code=$?
  took=$(($(date +%s) - t0))
  said=$(cat "$OUT/end$i.out" "$OUT/end$i.err" | tr -s '\n' ' ' | cut -c1-400)
  # The Hub's record, as the end left it: read until it is terminal, or for a minute.
  word=$(hub "$rid") tries=0
  while [[ $word != released\ [0-9]* && $word != failed\ [0-9]* ]] && [ "$tries" -lt 6 ]; do sleep 10; word=$(hub "$rid"); tries=$((tries + 1)); done
  here=$(state "$rid")
  say "END $rid ($name): exit $code after ${took}s, the Hub: ${word:-no record}, Calcifer: ${here:-none}; it said: $said"
  hub_ended=false; [[ $word == released\ [0-9]* || $word == failed\ [0-9]* ]] && hub_ended=true
  if [ "$code" = 0 ]; then
    grep -q 'shut down' "$OUT/end$i.out" || fail "$name: end exited 0 without saying it shut down: $said"
    $hub_ended || fail "$name: end said it shut down, and its Hub says $word"
    [ "$here" = ended ] && ! listed "$rid" || fail "$name: end said it shut down, and Calcifer says ${here:-nothing} (listed: $(listed "$rid" && echo yes || echo no))"
  else
    ! $hub_ended || fail "$name: end said it failed ($said), and its Hub ended it: $word"
    grep -q "$rid" "$OUT/end$i.err" || fail "$name: a failed end that does not name rental $rid: $said"
  fi
done
say "every end said what its Hub says"
