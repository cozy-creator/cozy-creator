# multi-machine: every machine is a full cozy (xs-126). Two rentals of SKU with SSH (`cozy machine rent --ssh`), A
# and B, both put on the session's cozy; then, as a user would, from a shell on each (`cozy machine ssh`):
#   a. A's shell: its `cozy` reaches the TensorD the image started as `local` (the rental's own leaf, no second
#      TensorD), and runs sent at once from A's shell and from the runner share A's GPU, twice (cold, then warm with the
#      models swapped): every run completes (they prepare together and execute in turn; the results say how).
#   b. A controls B: `cozy machine add` is refused with the key to authorize (the one A's `cozy machine show local`
#      prints), `cozy machine authorize` on B admits it, the add and a run on B from A complete; the runner's own key,
#      never authorized, is refused cleanly; A cannot add itself under another name. B's authorize prints the
#      address the runner rents it at. `authorize --remove` on B refuses A's next run at once; authorized again, it runs.
#   p. Two signers on A's TensorD: the runner reads a private model of the account there under its own capability;
#      A's shell, not signed in, is refused naming `cozy login`: the runner's capability never served it.
#   c. Mixed versions: A on the previous release controls B on the session's cozy. A goes back to the session's cozy
#      while its shell's Calcifer still runs the previous release with a run of its own going on B: the next command
#      restarts Calcifer on the new binary, no kill, and the run completes. Then A on the session's cozy controls B on
#      the previous release (completes, or fails at once naming the key to authorize); the runner still runs on B. A
#      removes b. The runner ends B; another controller of the account (a second user here) no longer shows it ready.
# The throwaway private model is deleted after. params:
#   old         the previous cozy release (default 0.13.3)
#   traffic_gb  GB the provider bills, both pods (default 40)
. "$(dirname "$0")/lib.sh"
OLD=$(param old 0.13.3)
NEW=$(cozy --version | awk '{print $2}')
SEED=$((RANDOM * 32768 + RANDOM))
[ -f "$REF/seed.jpg" ] || fail "no reference image"
[ "$OLD" != "$NEW" ] || fail "old ($OLD) is the session's cozy"
[ -f ~/.ssh/id_ed25519.pub ] || { mkdir -p ~/.ssh && ssh-keygen -q -t ed25519 -N '' -f ~/.ssh/id_ed25519; } || fail "no SSH key"
# Another controller of this account: a second user on the runner, signed in with the runner's device key.
sudo useradd -m cozywatch && sudo install -d -o cozywatch -m 700 /home/cozywatch/.cozy /home/cozywatch/.cozy/keys &&
  sudo install -o cozywatch -m 600 ~/.cozy/keys/device-*.json /home/cozywatch/.cozy/keys/ &&
  sudo install -D -m 755 "$(command -v cozy)" /opt/cozywatch/cozy || fail "no second controller"
watcher() { sudo -u cozywatch -H /opt/cozywatch/cozy "$@" < /dev/null 2>> "$OUT/watcher.err"; }
[ "$(watcher --json whoami | jq -r .account)" = "$(cozy --json whoami | jq -r .account)" ] || fail "the second controller is not signed in: $(tail -3 "$OUT/watcher.err")"
TS=$(date -u +%m%d%H%M) ACCOUNT=$(cozy --json whoami | jq -r .account) M=''
cleanup() { [ -z "$M" ] || cozy model delete "$M" -y >> "$OUT/delete.txt" 2>&1; }
trap 'cleanup; finish' EXIT
RENT_ARGS=--ssh
quote "$(param traffic_gb 40)" 2400 2
order; order
ready 0; A=$NAME
ready 1; B=$NAME
for i in 0 1; do jq -e '.rental.ssh // empty' "$OUT/show$i.json" > /dev/null || fail "rental $i has no ssh address"; done
watcher machine list --json > "$OUT/watcher-list.json"
jq -e --arg r "$(jq -r .rental_id "$OUT/show1.json")" '.machines[] | select(.rental_id == $r and .recorded == false and .state == "ready")' \
  "$OUT/watcher-list.json" > /dev/null || fail "the second controller does not list B: $(head -c 300 "$OUT/watcher-list.json")"

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
# update <machine> <cozy> <version>: the machine's cozy (its TensorD, and its shell's) becomes <version>.
update() {
  step "update-$1-$3" machine update "$1" --tensord "$2" -y || fail "update $1 to $3"
  [ "$(version_on "$1")" = "$3" ] || fail "$1 runs TensorD $(version_on "$1"), not $3"
  sh "$1" 'cozy --version' > "$OUT/version-$1-$3.txt"
  [ "$(cat "$OUT/version-$1-$3.txt")" = "cozy $3" ] || fail "$1's shell runs $(cat "$OUT/version-$1-$3.txt"), not cozy $3"
}
# calcifer_on <machine>: the cozy its shell's Calcifer runs.
calcifer_on() { sh "$1" 'grep -o "\"version\":\"[^\"]*\"" /root/.cozy/calcifer/calcifer.lock.holder' | cut -d'"' -f4; }
# A release older than the session's cozy cannot restart a newer Calcifer: the session ends that one by its pid.
end_calcifer() {
  sh "$1" 'p=$(grep -o "\"pid\":[0-9]*" /root/.cozy/calcifer/calcifer.lock.holder 2>/dev/null | cut -d: -f2)
    [ -z "$p" ] || { kill $p; while kill -0 $p 2>/dev/null; do sleep 1; done; }'
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
at=$(awk '$1=="add" && $2=="it" && $3=="at"{print $4}' "$OUT/b-authorize.txt")
case $at in
  "$(jq -r .address "$OUT/show1.json")") say "b: B's authorize names the address the runner rents it at: $at" ;;
  '') say "b: B's authorize names no address (its TensorD knows no provider mapping)" ;;
  *) fail "b: B's authorize names $at, not the address the runner rents it at ($(jq -r .address "$OUT/show1.json"))" ;;
esac
on "$A" b-add "cozy machine add b $ADDR --fingerprint $FP" || fail "b: add after authorize"
[ "$(sh "$A" 'cozy machine show local --json' | jq -r .controller_key)" = "$KEY" ] || fail "b: A's show local does not print the key B authorized"
on "$A" b-self "cozy machine add self $(jq -r .address "$OUT/show0.json") --fingerprint $(jq -r .fingerprint "$OUT/show0.json")" &&
  fail "b: A added its own machine under another name"
grep -q machine_is_local "$OUT/b-self.txt" || fail "b: A's add of itself was not refused as local: $(tail -2 "$OUT/b-self.txt")"
on "$A" b-run "cozy run sdxl/generate --on b prompt='a sleeping cat on a coil of ship rope, oil painting' seed:=$((SEED + 2))"
completed "$A" b-run || fail "b: A's run on B did not complete"
on "$B" b-remove "cozy machine authorize $KEY --remove" || fail "b: authorize --remove on B"
t=$(date +%s)
on "$A" b-removed "timeout 120 cozy run sdxl/generate --on b prompt='a fox asleep under a fern, ink drawing' seed:=$((SEED + 7))"
[ "$(record "$A" b-removed)" = failed ] && jq -e '.reason.code == "machine_unauthorized"' "$OUT/b-removed.json" > /dev/null ||
  fail "b: a run from A after B removed its key was not refused: $(jq -c '{status, reason}' "$OUT/b-removed.json")"
say "b: after --remove, A's next run on B was refused in $(($(date +%s) - t)) s"
on "$B" b-reauthorize "cozy machine authorize $KEY" || fail "b: authorize again on B"
s=$(date +%s)
step b-intruder machine add intruder "$ADDR" --fingerprint "$FP" && fail "b: B admitted the runner's key, never authorized"
grep -q machine_unauthorized "$OUT/b-intruder.txt" && grep -q 'cozy machine authorize' "$OUT/b-intruder.txt" ||
  fail "b: the unauthorized add was not refused cleanly: $(tail -3 "$OUT/b-intruder.txt")"
say "b: PASS (the runner's key refused in $(($(date +%s) - s)) s)"

# p. Two signers on A: a private model of the account, read by the renter under its capability, refused to A's
# signed-out shell with the remedy.
python3 -I -c 'import json,struct,sys,os; h=json.dumps({"w":{"dtype":"F32","shape":[4,4],"data_offsets":[0,64]}}).encode(); h+=b" "*(-len(h)%8); open(sys.argv[1],"wb").write(struct.pack("<Q",len(h))+h+os.urandom(64))' "$OUT/tiny.safetensors"
M=$ACCOUNT/mm-private-$TS
step p-upload model upload "$OUT/tiny.safetensors" "$M" --lane f32 || fail "p: upload of a private model"
MC=$M@$(awk '$1=="commit"{c=$2} END{sub(/.*@/, "", c); print c}' "$OUT/p-upload.txt")
step p-renter model download "$MC" --on "$A" || fail "p: the renter did not read its private model on A: $(tail -3 "$OUT/p-renter.txt")"
on "$A" p-shell "cozy model download $MC" && fail "p: A's signed-out shell read the account's private model"
grep -q 'cozy login' "$OUT/p-shell.txt" || fail "p: the shell's refusal does not name cozy login: $(tail -4 "$OUT/p-shell.txt")"
cozy machine logs "$A" -n 0 > "$OUT/a-tensord.log" 2>&1
say "p: renter read $MC on A; A's shell refused: $(grep -m1 -o 'this run carried no sign-in[^)]*' "$OUT/p-shell.txt"); capability lines on A: $(grep -c 'redeemed the run capability' "$OUT/a-tensord.log")"

# c. Mixed versions, both ways.
update "$A" "$OLD" "$OLD"
end_calcifer "$A"
on "$A" c1-version 'cozy --version; cozy machine list' || fail "c1: cozy $OLD on A"
[ "$(calcifer_on "$A")" = "$OLD" ] || fail "c1: A's Calcifer runs $(calcifer_on "$A"), not $OLD"
on "$A" c1-run "cozy run sdxl/generate --on b prompt='a tin robot watering a bonsai, studio photo' seed:=$((SEED + 3))"
completed "$A" c1-run || fail "c1: A on $OLD did not run on B on $NEW"
# A back on the session's cozy; its shell's Calcifer still runs $OLD and starts a run on B with the binary it runs.
update "$A" "$(command -v cozy)" "$NEW"
[ "$(calcifer_on "$A")" = "$OLD" ] || fail "c: A's Calcifer is not the one before the update"
[ "$(sh "$A" '/usr/local/bin/.cozy-previous --version')" = "cozy $OLD" ] || fail "c: no previous cozy beside A's"
sh "$A" "nohup /usr/local/bin/.cozy-previous run sdxl/generate --on b prompt='a lantern-lit greenhouse at night, photo' \
  seed:=$((SEED + 6)) steps:=50 < /dev/null > /root/inflight.txt 2>&1 &"
N=''; for _ in $(seq 1 90); do
  N=$(sh "$A" 'cat /root/inflight.txt' | grep -m1 -oE '^run [0-9]+' | awk '{print $2}')
  [ -n "$N" ] && sh "$A" "/usr/local/bin/.cozy-previous run show $N --once --json" | jq -e '.machine_state == "running"' > /dev/null && break
  sleep 2
done
[ -n "$N" ] || fail "c: the run from A's $OLD Calcifer never started: $(sh "$A" 'tail -3 /root/inflight.txt')"
say "c: run $N from A's Calcifer on $OLD is running on B; the next command on A"
on "$A" c-restart 'cozy machine list' || fail "c: the first command after the update"
grep -q "restarting it on $NEW" "$OUT/c-restart.txt" || fail "c: the command did not restart Calcifer: $(head -3 "$OUT/c-restart.txt")"
[ "$(calcifer_on "$A")" = "$NEW" ] || fail "c: A's Calcifer runs $(calcifer_on "$A") after the restart, not $NEW"
on "$A" c-inflight "cozy run show $N"
s=$(sh "$A" "cozy run show $N --once --json" | jq -r .status)
[ "$s" = completed ] || fail "c: run $N, going while Calcifer restarted, is $s"
say "c: A's Calcifer restarted itself on $NEW at the next command; run $N, going on B through it, completed"
update "$B" "$OLD" "$OLD"
t=$(date +%s)
on "$A" c2-run "timeout 300 cozy run sdxl/generate --on b prompt='an origami crane on a stack of old maps, photo' seed:=$((SEED + 4))"
t=$(($(date +%s) - t)); c2=$(record "$A" c2-run)
case $c2 in
  completed) say "c2: A on $NEW ran on B on $OLD" ;;
  failed) jq -e '.reason.code == "machine_unauthorized" and (.reason.message | test("cozy machine authorize"))' "$OUT/c2-run.json" > /dev/null ||
       fail "c2: A on $NEW -> B on $OLD failed without naming what admits it: $(jq -c .reason "$OUT/c2-run.json")"
     [ "$t" -le 90 ] || fail "c2: the refusal took ${t}s"
     say "c2: A on $NEW -> B on $OLD failed in ${t}s: $(jq -r .reason.message "$OUT/c2-run.json")" ;;
  *) fail "c2: A on $NEW -> B on $OLD is $c2 after ${t}s: $(tail -c 300 "$OUT/c2-run.json")" ;;
esac
step c2-renter run sdxl/generate --on "$B" prompt="a wind-up music box on velvet, candlelight" seed:=$((SEED + 5))
completed - c2-renter || fail "c2: the runner on $NEW did not run on B on $OLD"
on "$A" c-end-b 'cozy machine end b' || fail "c: A did not remove b"
# The runner ends B; the second controller, which listed it ready, now reads its Hub.
RB=$(jq -r .rental_id "$OUT/show1.json")
cozy machine end "$RB" -y >> "$OUT/end.txt" 2>&1; until gone "$RB"; do sleep 10; done
watcher machine show "$RB" --json > "$OUT/watcher-ended.json"
w=$(jq -r '.state // .error.code' "$OUT/watcher-ended.json")
case $w in ready|booting) fail "c: the second controller shows ended B as $w" ;; esac
say "c: B ended; the second controller's show says $w"
say "c: PASS"
say "PASS: a shell on a rented worker uses its TensorD as local beside remote work; A controls B by authorize/add; cozy $OLD and $NEW control each other; Calcifer follows its binary"
