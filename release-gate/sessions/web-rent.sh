# web-rent (cl-774 Part C, decision 2): Rent and End through the page, in a real Chrome, on this runner's Calcifer (sessions/fixtures/web-rent/rent.js).
# At $0, whichever `rent` says: the page serves, the SKU list, a quote through the page with its stated price and no order, and
# the guard's arithmetic. With `rent`: one click on that price buys one pod with SSH; the page shows it boot; it's used: the idle clock and Keep up,
# what's installed, a log, a real run from the terminal live in the page to its end; End through the page, followed until the Hub affirms the
# pod gone, which `machine ls` then agrees with. The guard
# starts before the click. Whatever fails, the trap ends what this runner's Calcifer ordered. It takes no params.
. "$(dirname "$0")/lib.sh"
FIX=$(cd "$(dirname "$0")/fixtures/web-rent" && pwd) || fail "no fixtures"
J() { jq -e "$@" > /dev/null; }
check() { say "FAIL: $*"; echo "::error::$*"; RC=1; }
mark() { say "$*"; echo "::notice::$*"; }
for tool in jq node npm google-chrome curl; do command -v "$tool" > /dev/null || fail "no $tool"; done
[ -f ~/.ssh/id_ed25519.pub ] || { mkdir -p ~/.ssh && ssh-keygen -q -t ed25519 -N '' -f ~/.ssh/id_ed25519; } || fail "no SSH key"
RENT_ARGS=--ssh
lock=$HOME/.cozy/calcifer/calcifer.lock.holder

# What this runner's Calcifer ordered is this session's: it started with none, and another client's rentals, which it lists too, read
# `ordered: false`. So a lost answer's order is ended too, by name until it has a rental id.
own() { machines_json | jq -r '.machines[] | select(.kind == "rental" and .ordered and .state != "ended") | .rental_id // .name'; }
claim() {
  local r
  for r in $(own); do
    [[ " ${RIDS[*]:-} " == *" $r "* ]] || { RIDS+=("$r"); echo "$r" >> "$OUT/rental"; say "this runner's Calcifer also holds $r"; }
  done
}
trap 'code=$?; claim; (exit "$code"); finish' EXIT

PW=$(mktemp -d)
npm install -s --prefix "$PW" playwright-core@1.55.0 > "$OUT/npm.log" 2>&1 || fail "npm install playwright-core: $(tail -c 300 "$OUT/npm.log")"
cozy status > "$OUT/status.txt" 2>&1
ADDR=$(jq -r .address < "$lock") TOKEN=$(jq -r .token < "$lock")
[ -n "$TOKEN" ] || fail "this runner's Calcifer has no record"
page=$(curl -sS --max-time 30 -o /dev/null -w '%{http_code}' "http://$ADDR/")
[ "$page" = 200 ] || fail "this cozy serves no page: GET / answers $page"
[ -z "$(own)" ] || fail "this runner's Calcifer already holds a rental: $(own | paste -sd' ')"

# link: a fresh sign-in link, which works once.
link() {
  cozy open --no-open > "$OUT/web-open.txt" 2>&1 || fail "cozy open --no-open: $(tail -c 300 "$OUT/web-open.txt")"
  grep -oE 'http://[^[:space:]]+#code=[0-9a-f]+[^[:space:]]*' "$OUT/web-open.txt" | head -1
}
browser() {
  local json=$1; shift
  ORDER=$OUT/order.json PROJECT=$RUN_PROJECT SKU=$SKU SSH_KEY=$(cat ~/.ssh/id_ed25519.pub) SHOT=$OUT/page COZY=$(command -v cozy) CHROME=$(command -v google-chrome) \
    PLAYWRIGHT=$PW/node_modules/playwright-core node "$FIX/rent.js" "$@" > "$OUT/$json.json" 2> "$OUT/$json.err"
}
judged() {
  local json=$OUT/$1.json
  J '.failures | length == 0' "$json" || check "$1: $(jq -c .failures "$json" 2> /dev/null): $(jq -c '.seen | with_entries(select(.value == false or (.value | type) == "object" or (.value | type) == "string"))' "$json" 2> /dev/null | head -c 1200) $(tail -c 500 "$OUT/$1.err")"
}

# The $0 pre-flight: the page's quote, which orders nothing, then the guard's arithmetic. A setup failure is the session's end.
browser quote "$(link)" || fail "the page's quote: $(jq -c .failures "$OUT/quote.json" 2> /dev/null) $(tail -c 500 "$OUT/quote.err")"
mark "the page states: $(jq -r .seen.stated "$OUT/quote.json"), and ordered nothing"
[ -z "$(own)" ] || fail "a quote left a rental: $(own | paste -sd' ')"
# A Hub gives a boot 15 minutes, and the use and End take some 10 more: a cap that can't buy that much could end the pod before End is tried.
quote 1500

# The guard, before the click: what's in $OUT/rental, and what this runner's Calcifer ordered, by what the Hub bills for them.
GUARD_ALSO=own guard

# record: the rental id, the moment the order's answer names it.
record() {
  [ -z "$RID" ] && [ -s "$OUT/order.json" ] || return 0
  local name id
  name=$(jq -r .name "$OUT/order.json") id=$(jq -r '.rental_id // empty' "$OUT/order.json")
  [ -n "$id" ] || id=$(cozy machine show "$name" --json 2> /dev/null | jq -r '.rental_id // empty')
  [ -n "$id" ] || return 0
  RID=$id; RIDS+=("$RID"); echo "$RID" >> "$OUT/rental"
  mark "rental $RID ($name)"
}
browser rent "$(link)" --rent & node=$!
BG="$BG $node"
while kill -0 "$node" 2> /dev/null; do record; sleep 2; done
wait "$node"; code=$?
record
say "the browser exited $code: $(jq -c '.seen | {boot, ending, end_says, stated}' "$OUT/rent.json" 2> /dev/null | head -c 1500)"
judged rent
[ "$code" = 0 ] || check "the browser exited $code"
[ -n "$RID" ] || check "the order's answer named no rental"
if [ -n "$RID" ] && gone "$RID"; then mark "gone: $RID is ended, and the machine listing holds no live machine of it"; else [ -z "$RID" ] || check "$RID is still live after End through the page"; fi
exit "$RC"
