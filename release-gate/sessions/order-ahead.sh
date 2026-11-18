# order-ahead (cl-1278; the Predictive Downloads coordinator, 11-18: "among a machine's predicted boxes, the one the user touched
# most recently downloads first"): the 0.27.0 walkthrough's two cold boxes on one rental (pod-session 44338350009, one RTX 4090:
# Klein and Qwen, about 60 GB at about 105 MB/s; the Qwen box, just edited, went behind Klein's whole set, and its Render waited
# about 100 s for an 11 GB enhancer that hadn't started), with this change's Calcifer and TensorD. Proof-branch harness, never
# merged. Every step through this runner's cozy and its Calcifer, every command naming the rental; the evidence is the machine's own
# (its Status every 2 s in samples.ndjson, its log) and Calcifer's lists to it.
#   1. A plain project of the rental alone holds `klein` (flux.3-klein/image), then `qwen` (qwen-image-2/image), both from the
#      packages checkout (a dev home, as the walkthrough's), drafted text-to-image. A page shows it: both are guessed onto the
#      rental, klein first.
#   2. With klein's models coming (`at_gib` of them there, not whole), qwen's draft mentions <pic>.
#   3. What qwen needs is whole before what klein needs: nothing of klein's in flight at the edit is refused or begun again, its
#      item stays listed, and its bytes grow again once qwen's are here. The rental ends by name then: nothing is rendered.
# The order is one: an out-of-stock answer ends the session, never ordered again.
# params:
#   tree       the packages_refs checkout (`-f packages_refs=pk=rc/0.27.0`, tree pk)
#   link_mbit  the rental's download link shaped to this many Mbit/s (default 800, the walkthrough's speed; "": as rented); a host
#              that refuses runs as rented and says so
#   at_gib     klein's bytes there when qwen is edited (default 2)
#   need_s     what the cap must buy (default 1800)
. "$(dirname "$0")/lib.sh"
# A failure keeps the rental's TensorD log (unless this session ended it) and Calcifer's.
LOGS_FROM=$(date -u +%s) ENDED=''
pod_logs() { cozy machine logs "$NAME" --since "$(( ($(date -u +%s) - LOGS_FROM) / 60 + 2 ))m" -n 0 > "$OUT/tensord-$NAME.log" 2>&1 || true; }
fail() {
  [ -z "${NAME:-}" ] || [ -n "$ENDED" ] || pod_logs
  cozy machine logs local calcifer --lines 20000 > "$OUT/calcifer.log" 2>&1
  say "FAIL: $*"; RC=1; exit 1
}
TREE=$(param tree '') LINK=$(param link_mbit 800) NEED=$(param need_s 1800) AT=$(param at_gib 2)
[ -n "$TREE" ] && [ -f "$PACKAGES/$TREE/qwen-image-2/pyproject.toml" ] && [ -f "$PACKAGES/$TREE/flux.3-klein/pyproject.toml" ] ||
  fail "dispatch with -f packages_refs=<name>=<packages ref> and params.tree=<name>"
[[ -z $LINK || $LINK =~ ^[0-9]+$ ]] || fail "link_mbit $LINK"
[[ $AT =~ ^[0-9]+$ ]] || fail "at_gib $AT"
SOURCE=$(cd "$PACKAGES/$TREE" && pwd)
V=$(cozy --version | awk '{print $2}')
[[ $V == *-dev* ]] || fail "cozy $V: the change runs from a dev build (pod-session cozy=<dev-build run id>)"
for tool in jq curl ffmpeg python3; do command -v "$tool" > /dev/null || fail "no $tool"; done
# A dev home, its first-party packages from the checkout; Calcifer started again on it.
cfg=$HOME/.cozy/config.toml
mkdir -p "${cfg%/*}" && touch "$cfg" || fail "no $cfg"
{ echo 'dev = true'; grep -v '^dev *=' "$cfg" || true; } > "$cfg.new" && mv "$cfg.new" "$cfg" || fail "dev = true not written"
awk -v source="$SOURCE" '
  function ours() { print "diagnostics = true"; print "packages = \"" source "\""; put = 1 }
  /^(diagnostics|packages) *=/ { next } { print } /^\[calcifer\]/ { ours() } END { if (!put) { printf "\n[calcifer]\n"; ours() } }' \
  "$cfg" > "$cfg.next" && mv "$cfg.next" "$cfg" || fail "[calcifer] not written"
cozy restart > "$OUT/restart.txt" 2>&1 || fail "cozy restart: $(tail -c 300 "$OUT/restart.txt")"
# Each package as its release ships it, kernels too, as a guess may load it once its weights are here.
kernels "$SOURCE/qwen-image-2"
kernels "$SOURCE/flux.3-klein"
nice -n 19 ffmpeg -v error -y -f lavfi -i 'testsrc2=size=1024x1024:rate=1' -frames:v 1 "$OUT/pic.png" || fail "no picture"
WORDS='A red bicycle against a white wall.' PICTURED='The red bicycle of <pic>, against a white wall at night.'
PAGE() { python3 -I "$RUN_PAGE" "$@"; }
# c <name> <dir> <cozy args…>: one command in <dir>, its output in <name>.txt.
c() { local n=$1 d=$2; shift 2; (cd "$d" && timeout 900 cozy "$@") > "$OUT/$n.txt" 2>&1 < /dev/null; }
now() { date +%s%3N; }

P=${RUNNER_TEMP:-/tmp}/order-ahead/two
run_project "$P" > "$OUT/project-two.json" 2>&1 || fail "the project: $(tail -c 300 "$OUT/project-two.json")"
PAGE import "$P" "$OUT/pic.png" --name pic > "$OUT/pic.json" 2>&1 || fail "its picture: $(tail -c 300 "$OUT/pic.json")"
for t in flux.3-klein qwen-image-2; do
  c "describe-$t" "$P" run "$t/image" --describe || fail "$t/image: $(tail -c 300 "$OUT/describe-$t.txt")"
  grep -q 'from source' "$OUT/describe-$t.txt" || fail "$t/image isn't the checkout's: $(head -3 "$OUT/describe-$t.txt")"
done
draft() { jq -nc --arg w "$1" '{instructions: $w, megapixels: 1}'; }
PAGE add "$P" klein --tool flux.3-klein/image --fields "$(draft 'A lighthouse on a cliff at dawn, waves breaking below.')" > "$OUT/add-klein.json" 2>&1 ||
  fail "klein: $(tail -c 300 "$OUT/add-klein.json")"
PAGE add "$P" qwen --tool qwen-image-2/image --fields "$(draft "$WORDS")" > "$OUT/add-qwen.json" 2>&1 || fail "qwen: $(tail -c 300 "$OUT/add-qwen.json")"
ver() { sed -n 's/^version = "\(.*\)"/\1/p' "$SOURCE/$1/pyproject.toml" | head -1; }
say "cozy $V, a dev home; flux.3-klein $(ver flux.3-klein) and qwen-image-2 $(ver qwen-image-2) from $TREE $(git -C "$SOURCE" rev-parse --short=8 HEAD 2> /dev/null)"

[ -z "$LINK" ] || { [ -f ~/.ssh/id_ed25519.pub ] || ssh-keygen -q -t ed25519 -N '' -f ~/.ssh/id_ed25519 || fail "ssh-keygen"; RENT_ARGS=--ssh; }
# One quote and one order: an out-of-stock answer ends the session (no second order).
say "cozy $(cozy --version); $(cozy whoami 2>&1 | grep -E '^account')"
! listed pr-pod-session-probe || fail "gone-check: a rental that never existed reads live"
f=$OUT/quote-$SKU.json
cozy machine rent "$SKU" --disk-gb "$DISK" $RENT_ARGS --json < /dev/null > "$f" 2> "${f%.json}.err" || true
! grep -qs sku_out_of_stock "$f" "${f%.json}.err" || fail "OUT OF STOCK at the quote, not asked again: $(tail -c 300 "${f%.json}.err")"
grep -qs quote_unagreed "$f" "${f%.json}.err" || fail "a quote without --yes did not refuse: $(tail -c 300 "$f") $(tail -c 300 "${f%.json}.err")"
price=$(jq -R -r 'fromjson? | select(.quote) | .quote.usd_micros_per_hour' "$f" | head -1)
[ -n "$price" ] || fail "no quote for $SKU: $(tail -c 300 "$f") $(tail -c 300 "${f%.json}.err")"
cp "$f" "$OUT/quote.json"
CAP_S=$(awk -v c="$CAP" -v h="$price" 'BEGIN{printf "%d", c*1e6 / h * 3600}')
[ "$CAP_S" -ge "$NEED" ] || fail "\$$CAP leaves only ${CAP_S}s at $price micro-USD/h"
say "quote $SKU $price micro-USD/h, $DISK GB asked: \$$CAP buys about ${CAP_S}s; the guard ends it by what the Hub bills"
[ "$RENT" = true ] || { say "pre-flight only: nothing rented"; exit 0; }
cozy --json machine rent "$SKU" --disk-gb "$DISK" $RENT_ARGS -y > "$OUT/rent0.out" 2> "$OUT/rent0.err" 9>&- & RP_0=$!
until RID=$(accepted "$OUT/rent0.out"); [ -n "$RID" ] || ! kill -0 "$RP_0" 2> /dev/null; do sleep 1; done
[ -n "$RID" ] || RID=$(accepted "$OUT/rent0.out")
if [ -z "$RID" ]; then
  wait "$RP_0"
  ! grep -qs sku_out_of_stock "$OUT/rent0.out" "$OUT/rent0.err" || fail "OUT OF STOCK: the order for $SKU was refused and not placed again: $(tail -c 300 "$OUT/rent0.err")"
  fail "the order was not accepted: $(tail -c 300 "$OUT/rent0.out") $(tail -3 "$OUT/rent0.err")"
fi
RIDS+=("$RID"); echo "$RID" >> "$OUT/rental"; SKU_0=$SKU
say "rental $RID ordered"
guard
ready
c use "$P" --json machine use "$NAME" || fail "machine use $NAME: $(tail -c 300 "$OUT/use.txt")"

# The pod's Status every 2 s: its list items' readiness, and each model's bytes (its own, all, whole).
( while :; do
    printf '{"t":%s,"s":%s}\n' "$(now)" "$(cozy --json machine show "$NAME" 2> /dev/null | jq -c '{v: .status.version,
      ahead: [.status.ahead[]? | {id, state, stage: .readiness.stage, done: .readiness.done, total: .readiness.total, reason: .readiness.reason}],
      models: [.status.models[]? | {r: .repository, all: .total_bytes, own: .unique_bytes, whole: ([.checkpoints[]?.complete] | any)}]}' 2> /dev/null || echo null)"
    sleep 2
  done >> "$OUT/samples.ndjson" ) & BG="$BG $!"
# ours: the open RC's builds on $NAME (its Runtime among them), then the session's TensorD, as needs-ahead.sh does.
version() { cozy --json machine show "$NAME" 2> /dev/null | jq -r '.status.version // empty'; }
step rc machine update "$NAME" -y || fail "$NAME didn't move to the open RC's builds"
tries=0
while runs=$(version); [ "$runs" != "$V" ]; do
  [ "$tries" -lt 3 ] || fail "$NAME doesn't keep the session's TensorD $V: it runs ${runs:-nothing it says}"
  step tensord machine update "$NAME" --tensord "$(command -v cozy)" -y || fail "$NAME refused the session's TensorD"
  sleep 30; tries=$((tries + 1))
done
say "$NAME runs the session's TensorD $V"
if [ -n "$LINK" ]; then
  for _ in $(seq 20); do timeout 120 cozy machine ssh "$NAME" true < /dev/null > /dev/null 2>&1 && break; sleep 15; done
  timeout 300 cozy machine ssh "$NAME" "command -v tc > /dev/null || { apt-get update -qq && DEBIAN_FRONTEND=noninteractive apt-get install -y -qq iproute2; }" \
    < /dev/null > "$OUT/iproute2.txt" 2>&1 || say "iproute2 not installed on $NAME: $(tail -c 200 "$OUT/iproute2.txt")"
  dev=$(timeout 120 cozy machine ssh "$NAME" "ip route show default | awk '{print \$5; exit}'" < /dev/null 2> /dev/null | tr -d '\r')
  if timeout 120 cozy machine ssh "$NAME" "tc qdisc add dev $dev handle ffff: ingress && tc filter add dev $dev parent ffff: protocol ip u32 match u32 0 0 police rate ${LINK}mbit burst 4m drop flowid :1" \
      < /dev/null > "$OUT/link.txt" 2>&1; then
    say "$NAME's link ($dev) is shaped to $LINK Mbit/s"
  else
    say "$NAME refused the shaped link: $(tail -c 200 "$OUT/link.txt"); it runs as rented, and the edit lands where the host's own speed puts it"
  fi
fi
[ "$(version)" = "$V" ] || fail "$NAME left the session's TensorD before the project was shown: it runs $(version)"

# row <box>: its guess's row on the board, the newest that names it. rows: both, in a line.
row() { PAGE board "$P" 2> /dev/null | jq -c --arg n "$1" '[.ahead[]? | select((.names // []) | index($n))] | max_by(.run) // {}' 2> /dev/null; }
rows() { echo "klein $(row klein | jq -c '{machine, ready, stage, done, total, waiting}' 2> /dev/null); qwen $(row qwen | jq -c '{machine, ready, stage, done, total, waiting}' 2> /dev/null)"; }
is_ready() { jq -e --arg m "$NAME" '.ready == true and .machine == $m' <<< "${1:-null}" > /dev/null 2>&1; }
models() { cozy --json machine show "$NAME" 2> /dev/null | jq -c '[.status.models[]? | {r: .repository, own: .unique_bytes, whole: ([.checkpoints[]?.complete] | any)}]' 2> /dev/null; }

say "step 1: a page shows the project; klein, then qwen, both drafted text-to-image, are guessed on $NAME"
( exec python3 -I "$RUN_PAGE" show "$P" > "$OUT/show.json" 2>&1 ) & SHOW=$!; BG="$BG $SHOW"
SHOWN_MS=$(now)
# Until klein has $AT GiB there: qwen's edit then lands while klein's models come. 15 min at most.
began=$SECONDS K=''
while [ $((SECONDS - began)) -lt 900 ]; do
  K=$(row klein)
  ! is_ready "$K" || fail "klein was ready on $NAME before qwen could be edited: nothing of it was left to come: $(rows)"
  [ "$(jq -r '.done // 0' <<< "${K:-null}" 2> /dev/null || echo 0)" -ge $((AT << 30)) ] && break
  sleep 1
done
[ "$(jq -r '.done // 0' <<< "${K:-null}" 2> /dev/null || echo 0)" -ge $((AT << 30)) ] || fail "klein's guess brought under $AT GiB to $NAME in 15 min: $(rows)"
say "before the edit (+$(( $(now) - SHOWN_MS )) ms): $(rows); $NAME holds $(models)"
say "step 2: qwen's draft mentions <pic>"
EDIT_MS=$(now)
PAGE set "$P" qwen --fields "$(jq -nc --arg w "$PICTURED" '{instructions: $w}')" > "$OUT/set-qwen.json" 2>&1 || fail "qwen's edit: $(tail -c 300 "$OUT/set-qwen.json")"
say "qwen's draft changed at $EDIT_MS (Unix ms, the runner's clock), $(( EDIT_MS - SHOWN_MS )) ms after the page showed the project"
# Until qwen is ready on $NAME or klein is: which one's needs were whole first. 25 min at most.
began=$SECONDS Q='' FIRST=''
while [ $((SECONDS - began)) -lt 1500 ]; do
  Q=$(row qwen); K=$(row klein)
  if is_ready "$Q"; then FIRST=qwen; break; fi
  if is_ready "$K"; then FIRST=klein; break; fi
  sleep 1
done
READY_MS=$(now)
say "step 3: ${FIRST:-neither} is ready first, +$(( READY_MS - EDIT_MS )) ms after the edit: $(rows); $NAME holds $(models)"
[ -n "$FIRST" ] || fail "neither box was ready on $NAME 25 min after the edit: $(rows)"
if [ "$FIRST" = qwen ]; then
  # klein's bytes grow again now that qwen's are here: nothing of it was cancelled. 60 s at most.
  had=$(jq -r '.done // 0' <<< "${K:-null}" 2> /dev/null || echo 0) began=$SECONDS grew=''
  while [ $((SECONDS - began)) -lt 60 ]; do
    K=$(row klein)
    { is_ready "$K" || [ "$(jq -r '.done // 0' <<< "${K:-null}" 2> /dev/null || echo 0)" -gt "$had" ]; } && { grew=1; break; }
    sleep 1
  done
  say "klein, $(( $(now) - READY_MS )) ms after qwen was ready: $([ -n "$grew" ] && echo "its bytes grow again" || echo "its bytes haven't grown in 60 s") ($had then): $(row klein | jq -c '{ready, stage, done, total}' 2> /dev/null)"
fi
VERSION_END=$(version)
pod_logs
cozy machine logs local calcifer --lines 20000 > "$OUT/calcifer.log" 2>&1 || true
say "the order is read: $NAME ends now, by name"
cozy machine end "$NAME" -y > "$OUT/end-by-name.txt" 2>&1 || cozy machine end "$NAME" -y --force >> "$OUT/end-by-name.txt" 2>&1
ENDED=1
say "END $NAME: $(tail -c 200 "$OUT/end-by-name.txt" | tr '\n' ' ')"
[ "$VERSION_END" = "$V" ] || fail "$NAME ran $VERSION_END at the end, not the session's TensorD $V: what it did isn't this change's"

# The timeline, the machine's own (its TensorD's log, its clock): which item is whose from Calcifer's lists (the key listed before
# the edit and after it is klein's, the key new after it qwen's); the edit as the pod took it (qwen's new item first named); when
# what each needs was here; and klein's objects in flight then.
python3 -I - "$OUT/calcifer.log" "$OUT/tensord-$NAME.log" "$NAME" "$EDIT_MS" > "$OUT/timeline.txt" 2> "$OUT/timeline.err" <<'PY'
import json, re, sys
from datetime import datetime
calcifer, tensord, name, edited = sys.argv[1], sys.argv[2], sys.argv[3], int(sys.argv[4])
def at(line):
    try:
        return int(datetime.fromisoformat(line.split(" ", 1)[0].replace("Z", "+00:00")).timestamp() * 1000)
    except ValueError:
        return 0
lists = []
for line in open(calcifer, errors="replace"):
    found = re.search(r"lists to " + re.escape(name) + r": .*prepare \[(.*?)\]( \(unchanged\))?$", line.rstrip())
    if found and not found[2] and at(line):
        lists.append((at(line), re.findall(r'"([^"]+)"', found[1])))
before = next((keys for when, keys in reversed(lists) if when <= edited), [])
after = next((keys for when, keys in lists if when > edited and set(keys) - set(before)), [])
kept, new = [key for key in before if key in after], [key for key in after if key not in before]
print(f"Calcifer's lists to {name}: before the edit {before}, after it {after}")
if len(kept) != 1 or len(new) != 1:
    print("FAIL its lists don't say which item is whose")
    print("judged")
    sys.exit(0)
klein, qwen = kept[0], new[0]
print(f"klein's item {klein} is listed {'behind' if after.index(klein) > after.index(qwen) else 'ahead of'} qwen's new item {qwen}")
MIB, objects, here, dropped, took, moved = 1 << 20, {}, {}, [], 0, []
for line in open(tensord, errors="replace"):
    when = at(line)
    if not when:
        continue
    if not took and f"cozy:prepare:{qwen}" in line:
        took = when
    found = re.search(r": (fetching|landed|fetch refused) .*object=([0-9a-f]{64})", line)
    if found:
        size, order = re.search(r"\bbytes=(\d+)", line), re.search(r"\bpriority=(\d+)", line)
        seen = objects.setdefault(found[2], {"fetching": [], "landed": [], "refused": [], "bytes": 0, "order": None})
        seen["fetching" if found[1] == "fetching" else "landed" if found[1] == "landed" else "refused"].append(when)
        if size:
            seen["bytes"] = int(size[1])
        if order and seen["order"] is None:
            seen["order"] = int(order[1])
    found = re.search(r"cozy:prepare:(\S+): what it needs is here", line)
    if found:
        here.setdefault(found[1], when)
    found = re.search(r"list item cozy:prepare:(\S+) dropped: its run", line)
    if found:
        dropped.append(found[1])
    if "moved ahead in the download line" in line:
        moved.append(line.strip()[-120:])
said = lambda when: "not yet when the rental ended" if when is None else f"{(when - took) / 1000:+.1f} s"
# A caller's guess's objects (class 3 of the download line), by the place they began at.
guesses = {sha: seen for sha, seen in objects.items() if seen["order"] is not None and seen["order"] >= 2 and (seen["order"] - 2) >> 62 == 3}
place = lambda seen: ((seen["order"] - 2) >> 32) & ((1 << 30) - 1)
flying = [seen for seen in guesses.values() if seen["fetching"] and min(seen["fetching"]) <= took and not (seen["landed"] and min(seen["landed"]) <= took)]
went = sum(seen["bytes"] for seen in guesses.values() if seen["landed"] and min(seen["landed"]) <= took)
after_it = [seen for seen in guesses.values() if seen["fetching"] and min(seen["fetching"]) > took]
end = max((when for seen in guesses.values() for when in seen["fetching"] + seen["landed"]), default=took)
print(f"the pod took the edit {took - edited:+d} ms from the runner's stamp of it (two clocks); from there, on the pod's clock:")
print(f"  at the edit: {went / MIB:,.0f} MiB of guesses' objects landed, {len(flying)} in flight ({sum(seen['bytes'] for seen in flying) / MIB:,.0f} MiB)")
print(f"  what qwen needs was here {said(here.get(qwen))}; what klein needs {said(here.get(klein))}")
by_place = {}
for seen in after_it:
    by_place.setdefault(place(seen), []).append(seen)
for at_place, group in sorted(by_place.items()):
    print(f"  begun after the edit at place {at_place}: {len(group)} objects, {sum(seen['bytes'] for seen in group) / MIB:,.0f} MiB, from {said(min(min(seen['fetching']) for seen in group))} to {said(max((min(seen['landed']) for seen in group if seen['landed']), default=None))}")
landed_late = [seen for seen in flying if seen["landed"]]
print(f"  of klein's {len(flying)} objects in flight at the edit: {len(landed_late)} landed by the log's end ({said(end)}), {sum(1 for seen in flying if len(seen['fetching']) > 1)} begun again, {sum(1 for seen in flying if seen['refused'])} refused")
for line in moved[:4]:
    print(f"  TensorD: {line}")
first, second = here.get(klein), here.get(qwen)
if second is None:
    print("FAIL what qwen needs was never here")
elif first is not None and first < second:
    print(f"FAIL what klein needs was whole {(second - first) / 1000:.1f} s before what qwen, edited last, needs")
if any(len(seen["fetching"]) > 1 or seen["refused"] for seen in flying):
    print("FAIL something of klein's in flight at the edit was begun again or refused")
if klein in dropped:
    print("FAIL klein's item was dropped, though only qwen was edited")
print("judged")
PY
while read -r line; do say "$line"; done < "$OUT/timeline.txt"
grep -qx judged "$OUT/timeline.txt" || fail "the timeline wasn't read: $(tail -c 400 "$OUT/timeline.err")"
! grep -q '^FAIL ' "$OUT/timeline.txt" || fail "the order: $(grep '^FAIL ' "$OUT/timeline.txt" | sed 's/^FAIL //' | paste -sd';')"
[ "$FIRST" = qwen ] || fail "klein was ready on $NAME before qwen, the box edited last"
say "PASS: on $NAME, what the box edited last needs was whole first, and nothing of the other box's was cancelled"
