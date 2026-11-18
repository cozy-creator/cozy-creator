#!/usr/bin/env bash
# construction-card (cr-335, D3): a model's construction never reads the real card. A construction is a function of its config
# and the card's variant, so its factory sees one card named by that variant (`sm<NN>`), with its capability; the request after
# it, and the Runtime's own code inside the construction, read the real card. Paul, 11-17, on hack audit varena #6 (that view
# was a process-wide patch of torch.cuda, and the Runtime's escape from it broke every multi-GPU group, cr-326): "those are
# pretty bad ones that all need to be patched". fixtures/construction-card.py's `card` returns both readings; the cell fails if
# the construction read the real card or the request read the variant. One GPU of any card; under `bundle`, its pod. A change to
# derive's construction substrate runs it as its PR proof, with `without` for its red. params:
#   runtime    a Runtime in place of the pod's: a cozy-runtime dev-build or rc-dev run id, or a wheel's https URL
#   tensorfs   a TensorFS with it, as runtime (a cozy-creator dev-build's `artifact:<run id>/<name>`)
#   without    a Runtime with the view removed, run first: its construction must read the real card (the cell's red)
. "$(dirname "$0")/lib.sh"
FIX=$(cd "$(dirname "$0")/fixtures" && pwd)
[ -f "$FIX/construction-card.py" ] || fail "no fixtures/construction-card.py"
command -v jq > /dev/null || fail "no jq"
declare -A W=()
for k in runtime tensorfs without; do
  src=$(param "$k"); [ -n "$src" ] || continue
  W[$k]=$(wheel_from "$OUT/wheels/$k" "$src") && [ -f "${W[$k]}" ] || fail "no $k wheel from $src"
  say "$k: ${W[$k]##*/}"
done
[ -z "${W[without]:-}" ] || [ -n "${W[runtime]:-}" ] || fail "without needs runtime: the green run moves the machine back to it"
quote 1800
rent
# A dev home on an RC build moves its rental to the RC's builds once it's ready (ab.sh): nothing runs before that ends.
cozy machine show "$NAME" --json > "$OUT/show-rc.json" 2> /dev/null
if [ -n "$(jq -r '.rc.state // empty' "$OUT/show-rc.json")" ]; then
  for _ in $(seq 180); do
    [ "$(jq -r '.rc.state // empty' "$OUT/show-rc.json")" = current ] && [ "$(jq -r '.state // empty' "$OUT/show-rc.json")" = ready ] && break
    sleep 5; cozy machine show "$NAME" --json > "$OUT/show-rc.json" 2> /dev/null
  done
  [ "$(jq -r '.rc.state // empty' "$OUT/show-rc.json")" = current ] || fail "$NAME isn't on the RC's builds in 15 min: $(jq -c .rc "$OUT/show-rc.json")"
fi

read -r -d '' JUDGE << 'JQ'
.result as $r | ($r.construction // {}) as $c | ($r.request // {}) as $q
| if $r == null then "no result: \(.status // "unrecorded") \(.reason.message // "")"
  elif $c.count != 1 then "the construction read \($c.count) cards"
  elif $c.name != "sm\($c.capability[0])\($c.capability[1])" then "the construction read the card's name, \($c.name)"
  elif $c.capability != $q.capability then "the construction read capability \($c.capability), the card's is \($q.capability)"
  elif ($c.available | not) then "the construction read no CUDA"
  elif $q.name == $c.name then "the request read the variant, \($q.name), not its card"
  else "ok" end
JQ
# card <label> [update flags…]: the machine updated to those builds (none: as it is), then `card`; its record in <label>.json and
# the verdict in VERDICT. The run must be on the machine's own Runtime: a fixture floor it missed would bring its own.
card() {
  local c=$1 n; shift
  if [ $# -gt 0 ]; then
    step "update-$c" machine update "$NAME" --tensord "$(command -v cozy)" "$@" -y || fail "update before $c: $(tail -c 300 "$OUT/update-$c.txt")"
    for _ in $(seq 120); do [ "$(state "$NAME")" = ready ] && break; sleep 5; done
  fi
  step "$c" run "$FIX/construction-card.py/card" "note=$c $RANDOM" --on "$NAME" || fail "$c: $(tail -3 "$OUT/$c.txt" | cut -c1-300)"
  n=$(runno "$c")
  (cd "$RUN_PROJECT" && cozy --json --full run watch "$n" --once) > "$OUT/$c.json" 2> "$OUT/$c.err"
  local held ran
  held=$(cozy machine show "$NAME" --json 2> /dev/null | jq -r '.status.runtime // empty') ran=$(jq -r '.result.runtime // empty' "$OUT/$c.json")
  [ -n "$ran" ] && [ "$ran" = "$held" ] || fail "$c ran on Runtime ${ran:-none}, not the machine's $held"
  VERDICT=$(jq -r "$JUDGE" "$OUT/$c.json")
  say "$c: run $n on Runtime $ran: construction $(jq -c .result.construction "$OUT/$c.json"), request $(jq -c .result.request "$OUT/$c.json")"
}
if [ -n "${W[without]:-}" ]; then
  card red --runtime "${W[without]}" ${W[tensorfs]:+--tensorfs "${W[tensorfs]}"}
  if [ "$VERDICT" != ok ]; then say "RED (expected) without the view: $VERDICT"; else say "FAIL red: without the view the construction still read one card"; RC=1; fi
fi
flags=()
[ -z "${W[runtime]:-}" ] || flags+=(--runtime "${W[runtime]}")
[ -z "${W[tensorfs]:-}" ] || flags+=(--tensorfs "${W[tensorfs]}")
card green "${flags[@]}"
if [ "$VERDICT" = ok ]; then say "PASS: the construction read one card, its variant; the request read its card"; else say "FAIL: $VERDICT"; RC=1; fi
{ echo "## construction-card on $NAME ($SKU)"; grep -E '^[0-9:]+ (PASS|FAIL|RED)' "$R"; } >> "${GITHUB_STEP_SUMMARY:-/dev/null}"
exit "$RC"
