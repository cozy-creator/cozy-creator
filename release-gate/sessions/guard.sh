# guard (cl-774): a session's guard ends its rentals by what the Hub bills, never by a quote (42018791462: a 20 GB quote at $0.12/h,
# an order of the default disk billed at $0.49/h). One cpu pod, left idle, until the guard ends it: the GUARD line comes, the pod is gone,
# and what the Hub billed for it, or its provider if more, is under the cap. Pass a small cap: $0.10 is a few minutes of a cpu pod. No params.
. "$(dirname "$0")/lib.sh"
quote 0 60
rent
t=0
until grep -q 'GUARD: ' "$R"; do
  sleep 10; t=$((t + 10))
  # An idle pod ends itself after 15 minutes, which isn't the guard.
  [ "$t" -lt 840 ] || fail "no GUARD line in ${t}s: $(tail -3 "$OUT/guard.log" 2> /dev/null)"
done
say "the guard ended it after ${t}s: $(grep 'GUARD: ' "$R" | tail -1)"
until gone "$RID"; do sleep 10; done
spent=$(timeout 60 cozy machine ls --since 24h --json < /dev/null 2> /dev/null | jq -r --arg r "$RID" \
  '.rentals[] | select(.rental_id == $r) | [.spend_usd_micros // 0, .provider_cost_usd_micros // 0] | max')
[ -n "$spent" ] || fail "no bill for $RID"
awk -v s="$spent" -v c="$CAP" 'BEGIN { exit !(s <= c * 1e6) }' || fail "$RID was billed ${spent} micro-USD, above the \$$CAP cap"
say "billed ${spent} micro-USD, under the \$$CAP cap"
