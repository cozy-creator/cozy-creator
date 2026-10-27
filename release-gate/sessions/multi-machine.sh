# multi-machine: every machine is a full cozy (xs-126). Two rentals of SKU with SSH (`cozy machine rent --ssh`), A
# and B, both put on the session's cozy; then, as a user would, from a shell on each (`cozy machine ssh`):
#   a. A's shell: its `cozy` reaches the TensorD the image started as `local` (the rental's own leaf, no second
#      TensorD), and runs sent at once from A's shell and from the runner share A's GPU, twice (cold, then warm with the
#      models swapped): every run completes (they prepare together and execute in turn; the results say how).
#   b. A controls B: `cozy machine add` is refused with the key to authorize, `cozy machine authorize` on B admits it,
#      the add and a run on B from A complete; the runner's own key, never authorized, is refused cleanly.
#   c. Mixed versions: A on the previous release controls B on the session's cozy, then A on the session's cozy
#      controls B on the previous release (completes, or waits saying what admits it); the runner still runs on B.
# params:
#   old         the previous cozy release (default 0.13.3)
#   traffic_gb  GB the provider bills, both pods (default 60)
. "$(dirname "$0")/lib.sh"
OLD=$(param old 0.13.3)
NEW=$(cozy --version | awk '{print $2}')
SEED=$((RANDOM * 32768 + RANDOM))
[ -f "$REF/seed.jpg" ] || fail "no reference image"
[ "$OLD" != "$NEW" ] || fail "old ($OLD) is the session's cozy"
[ -f ~/.ssh/id_ed25519.pub ] || { mkdir -p ~/.ssh && ssh-keygen -q -t ed25519 -N '' -f ~/.ssh/id_ed25519; } || fail "no SSH key"
RENT_ARGS=--ssh
quote "$(param traffic_gb 60)" 3600 2
order; order
ready 0; A=$NAME
ready 1; B=$NAME
for i in 0 1; do jq -e '.rental.ssh // empty' "$OUT/show$i.json" > /dev/null || fail "rental $i has no ssh address"; done

# on <machine> <step> <command>: a step that runs <command> in a shell there.
on() { local m=$1 n=$2; shift 2; step "$n" machine ssh "$m" -- "$*"; }
# sh <machine> <command>: its stdout, for a value.
sh() { local m=$1; shift; cozy machine ssh "$m" -- "$*" < /dev/null 2>> "$OUT/ssh.err"; }
# num <step>: the run that step started (its first line names it, followed to its end or not).
num() { grep -m1 -oE '^run [0-9]+' "$OUT/$1.txt" | awk '{print $2}'; }
# record <machine|-> <step>: that run as its controller records it (-: the runner's); its status.
record() {
  local n; n=$(num "$2")
  [ -n "$n" ] || { echo "no run"; return; }
  case $1 in
    -) cozy run show "$n" --once --json ;;
    *) sh "$1" "cozy run show $n --once --json" ;;
  esac > "$OUT/$2.json" 2>&1
  jq -r '.status // empty' "$OUT/$2.json"
}
completed() {
  local s; s=$(record "$1" "$2")
  say "$2: run $(num "$2") $s on $(jq -r '.machine // "?"' "$OUT/$2.json"), $(jq -r '((.settled_at_ms // 0) - (.created_at_ms // 0)) / 1000' "$OUT/$2.json") s$(jq -r '.reason.message // empty | ": " + .' "$OUT/$2.json" 2>/dev/null)"
  [ "$s" = completed ]
}
version_on() { cozy machine show "$1" --json | jq -r .status.version; }
# update <machine> <cozy> <version>: the machine's cozy (its TensorD, and its shell's) becomes <version>; the
# Calcifer its shell started on the one before ends, so the next command starts it on this one.
update() {
  step "update-$1-$3" machine update "$1" --tensord "$2" -y || fail "update $1 to $3"
  [ "$(version_on "$1")" = "$3" ] || fail "$1 runs TensorD $(version_on "$1"), not $3"
  sh "$1" 'h=/root/.cozy/calcifer/calcifer.lock.holder; p=$(grep -o "\"pid\":[0-9]*" $h 2>/dev/null | cut -d: -f2)
    [ -z "$p" ] || { kill $p; while kill -0 $p 2>/dev/null; do sleep 1; done; }; cozy --version' > "$OUT/version-$1-$3.txt"
  [ "$(cat "$OUT/version-$1-$3.txt")" = "cozy $3" ] || fail "$1's shell runs $(cat "$OUT/version-$1-$3.txt"), not cozy $3"
}

for m in "$A" "$B"; do update "$m" "$(command -v cozy)" "$NEW"; done
say "A=$A B=$B, both on cozy $NEW; shells reached by cozy machine ssh"

# a. A's shell: local is the TensorD the image started; it and the runner share A's GPU.
sh "$A" 'cozy machine show local --json' > "$OUT/a-local.json" || fail "cozy machine show local on $A"
[ "$(jq -r .fingerprint "$OUT/a-local.json")" = "$(jq -r .fingerprint "$OUT/show0.json")" ] ||
  fail "A's local is not the rental's TensorD: $(jq -c '{address, fingerprint}' "$OUT/a-local.json")"
sh "$A" 'ps -eo args' > "$OUT/a-ps.txt"
grep 'cozy machine serve' "$OUT/a-ps.txt" | grep -qv -- '--rental' && fail "a second TensorD runs on $A: $(grep 'machine serve' "$OUT/a-ps.txt" | paste -sd';')"
say "a: A's local is the image's TensorD ($(jq -r .address "$OUT/a-local.json")), and the only one"
cozy machine ssh "$A" -- 'cat > /root/seed.jpg' < "$REF/seed.jpg" 2>> "$OUT/ssh.err" || fail "copy to $A"
# turns <step> <step>: how the two runs shared the GPU: accepted together, then each one's execution, in order.
turns() {
  jq -rs 'sort_by(.started_at_ms) as [$x, $y] |
    "accepted \(((map(.accepted_at_ms) | max) - (map(.accepted_at_ms) | min)) / 1000) s apart; run \($x.number) executed \($x.started_at_ms) to \($x.settled_at_ms), run \($y.number) from \($y.started_at_ms) (\(if $y.started_at_ms < $x.settled_at_ms then "overlapping" else "\($y.started_at_ms - $x.settled_at_ms) ms after" end))"' "$OUT/$1.json" "$OUT/$2.json"
}
# Round 1, cold: SDXL from the runner and BiRefNet from A's shell, sent together.
step a1-remote run sdxl/generate --on "$A" prompt="a brass telescope on a windowsill at dawn, photo" seed:=$SEED & P=$!
on "$A" a1-local "cozy run birefnet/remove_background --asset image=/root/seed.jpg" & Q=$!
wait $P; wait $Q
completed - a1-remote || fail "a1: the runner's run on A did not complete"
completed "$A" a1-local || fail "a1: A's own run did not complete"
say "a1: $(turns a1-remote a1-local)"
# Round 2, warm, the models swapped between the two controllers.
step a2-remote run birefnet/remove_background --on "$A" --asset image="$REF/seed.jpg" & P=$!
on "$A" a2-local "cozy run sdxl/generate prompt='a paper boat on a puddle after rain, macro photo' seed:=$((SEED + 1))" & Q=$!
wait $P; wait $Q
completed - a2-remote || fail "a2: the runner's run on A did not complete"
completed "$A" a2-local || fail "a2: A's own run did not complete"
say "a2: $(turns a2-remote a2-local)"
say "a: PASS"

# b. A controls B: refused with the key to authorize, authorized on B, added, run.
ADDR=$(jq -r .address "$OUT/show1.json") FP=$(jq -r .fingerprint "$OUT/show1.json")
on "$A" b-refused "cozy machine add b $ADDR --fingerprint $FP" && fail "b: B admitted A before any authorize"
KEY=$(grep -oE 'cozy machine authorize [A-Za-z0-9_-]{43}' "$OUT/b-refused.txt" | awk '{print $4}' | head -1)
grep -q machine_unauthorized "$OUT/b-refused.txt" && [ -n "$KEY" ] || fail "b: the refusal names no key to authorize: $(tail -3 "$OUT/b-refused.txt")"
on "$B" b-authorize "cozy machine authorize $KEY" || fail "b: authorize on B"
on "$A" b-add "cozy machine add b $ADDR --fingerprint $FP" || fail "b: add after authorize"
on "$A" b-run "cozy run sdxl/generate --on b prompt='a sleeping cat on a coil of ship rope, oil painting' seed:=$((SEED + 2))"
completed "$A" b-run || fail "b: A's run on B did not complete"
s=$(date +%s)
step b-intruder machine add intruder "$ADDR" --fingerprint "$FP" && fail "b: B admitted the runner's key, never authorized"
grep -q machine_unauthorized "$OUT/b-intruder.txt" && grep -q 'cozy machine authorize' "$OUT/b-intruder.txt" ||
  fail "b: the unauthorized add was not refused cleanly: $(tail -3 "$OUT/b-intruder.txt")"
say "b: PASS (the runner's key refused in $(($(date +%s) - s)) s)"

# c. Mixed versions, both ways.
update "$A" "$OLD" "$OLD"
on "$A" c1-version 'cozy --version; cozy machine list' || fail "c1: cozy $OLD on A"
on "$A" c1-run "cozy run sdxl/generate --on b prompt='a tin robot watering a bonsai, studio photo' seed:=$((SEED + 3))"
completed "$A" c1-run || fail "c1: A on $OLD did not run on B on $NEW"
update "$A" "$(command -v cozy)" "$NEW"
update "$B" "$OLD" "$OLD"
on "$A" c2-run "timeout 300 cozy run sdxl/generate --on b prompt='an origami crane on a stack of old maps, photo' seed:=$((SEED + 4))"
c2=$(record "$A" c2-run)
case $c2 in
  completed) say "c2: A on $NEW ran on B on $OLD" ;;
  *) w=$(jq -r '[.. | strings | select(test("cozy machine authorize"))][0] // empty' "$OUT/c2-run.json" 2>/dev/null)
     [ -n "$w" ] || fail "c2: A on $NEW -> B on $OLD is $c2 with no word of what admits it: $(tail -c 400 "$OUT/c2-run.json")"
     say "c2: A on $NEW -> B on $OLD is $c2: $w"
     sh "$A" "cozy run cancel $(num c2-run)" >> "$OUT/c2-cancel.txt" ;;
esac
step c2-renter run sdxl/generate --on "$B" prompt="a wind-up music box on velvet, candlelight" seed:=$((SEED + 5))
completed - c2-renter || fail "c2: the runner on $NEW did not run on B on $OLD"
say "c: PASS"
say "PASS: a shell on a rented worker uses its TensorD as local beside remote work; A controls B by authorize/add; cozy $OLD and $NEW control each other"
