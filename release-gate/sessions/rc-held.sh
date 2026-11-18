# rc-held (cl-1236, cl-1267): a run held for its rental's update keeps that rental past its idle deadline, and runs there.
# Calcifer holds a run in its own journal while its machine updates, so TensorD sees no job; since cl-1203 an update restarts
# no idle clock. One that outlasted the deadline released the rental on the restarted service's first pass, under the run
# waiting for it, and a new rental was bought (red on a runner's stood-in rental: e2e 44296656584). The fix: the held run's
# driver sends the machine's keepalive as it is held and as the update is taken, and an update a keepalive came during
# restarts the clock at its end. Its green was by reading (#3935, #3955): a runner's update is too quick to hold a render.
# One cheap rental of $SKU with SSH, this session's cozy its TensorD (the fix is TensorD's and Calcifer's; the code sent asks
# for the rental's Runtime minor, so nothing but the updates by hand moves it):
#   1. its idle window becomes `idle_s` for its service's next start (the config.toml of the running service's own home,
#      over SSH: the running service keeps its 15 minutes), a keepalive starts its clock, and it sits idle past `idle_s`:
#      the restarted service opens a clock already past its deadline, as one an update outlasted;
#   2. an update by hand moves its Runtime to `runtime_to` (a download and an install: long enough to hold a run; a restart
#      alone settles in under a second, 44396105554), a second is asked behind it, back to the Runtime it had (cl-1267's
#      case), and while the first goes, local code is sent `--on` the rental;
#   3. the run is held ("waiting for <rental> to finish updating"), completes on that rental, and its window after the
#      restart is `idle_s` (the control: else no deadline was outlasted).
# A run the update didn't hold proves nothing, and fails the session saying so.
# params: runtime_to (a Runtime release of the rental's minor other than its own, required), idle_s (120).
. "$(dirname "$0")/lib.sh"
check() { say "FAIL: $*"; echo "::error::$*"; RC=1; }
IDLE=$(param idle_s 120)
[[ $IDLE =~ ^[0-9]+$ && $IDLE -ge 60 && $IDLE -lt 900 ]] || fail "idle_s is seconds, 60 to 899: $IDLE"
TO=$(param runtime_to)
[[ $TO =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]] || fail "runtime_to is a Runtime release (0.47.0): ${TO:-none}"
CODE=$OUT/held-code
cp -r "$(dirname "$0")/fixtures/rc-floor" "$CODE" || fail "no fixtures/rc-floor"
[ -f ~/.ssh/id_ed25519.pub ] || { mkdir -p ~/.ssh && ssh-keygen -q -t ed25519 -N '' -f ~/.ssh/id_ed25519; } || fail "no SSH key"
RENT_ARGS=--ssh
quote $((IDLE + 900))
rent
step tensord machine update "$NAME" --tensord "$(command -v cozy)" -y || fail "this cozy as $NAME's TensorD: $(tail -3 "$OUT/tensord.txt")"
for _ in $(seq 60); do [ "$(state "$NAME")" = ready ] && break; sleep 5; done
cozy machine show "$NAME" --json > "$OUT/show-tensord.json" 2> /dev/null
want=$(cozy --version | awk '{print $2}')
[ "$(jq -r '.status.version // empty' "$OUT/show-tensord.json")" = "$want" ] ||
  fail "$NAME runs TensorD $(jq -r '.status.version // "?"' "$OUT/show-tensord.json"), not this session's $want"
# The code asks for the minor the rental runs: no floor moves the rental for it.
at=$(jq -r '.status.runtime // empty' "$OUT/show-tensord.json")
[[ $at =~ ^[0-9]+\.[0-9]+ ]] || fail "$NAME reports no Runtime: ${at:-none}"
minor=${BASH_REMATCH[0]}
[ "$TO" != "$at" ] && [[ $TO == "$minor".* ]] || fail "runtime_to $TO must be a release of $NAME's minor $minor other than its own $at"
sed -i "s/\"cozy-runtime[^\"]*\"/\"cozy-runtime>=$minor\"/" "$CODE/pyproject.toml" && grep -qF "\"cozy-runtime>=$minor\"" "$CODE/pyproject.toml" ||
  fail "the code's floor wasn't set to $NAME's Runtime minor $minor"

# Its next service start has the short window; the one running keeps 15 minutes. The service reads the config.toml of its own
# home, which need not be the SSH login's: its home is read from the running service's environment (else its user's entry).
script=$(cat << EOS
p=; for d in /proc/[0-9]*; do [ "\$(cat \$d/comm 2> /dev/null)" = tensord ] && { p=\${d#/proc/}; break; }; done
[ -n "\$p" ] || { echo "no tensord running"; exit 1; }
h=\$(tr '\\0' '\\n' < /proc/\$p/environ | sed -n 's/^HOME=//p')
[ -n "\$h" ] || h=\$(getent passwd "\$(stat -c %u /proc/\$p)" | cut -d: -f6)
[ -n "\$h" ] || { echo "tensord \$p has no home"; exit 1; }
echo "tensord \$p reads \$h/.cozy/config.toml; the SSH login is \$(id -un), home \$HOME"
c=\$h/.cozy/config.toml
mkdir -p "\${c%/*}" && touch "\$c" || exit 1
awk '/^idle_seconds *=/ { next } { print } /^\\[machine\\]/ { print "idle_seconds = $IDLE"; put = 1 }
  END { if (!put) printf "\\n[machine]\\nidle_seconds = $IDLE\\n" }' "\$c" > "\$c.new" && mv "\$c.new" "\$c" || exit 1
grep -x 'idle_seconds = $IDLE' "\$c"
EOS
)
said=
for _ in 1 2 3 4 5 6; do
  said=$(timeout 120 cozy machine ssh "$NAME" "echo $(base64 -w0 <<< "$script") | base64 -d | sh" < /dev/null 2>&1)
  grep -qx "idle_seconds = $IDLE" <<< "$said" && break
  sleep 10
done
grep -qx "idle_seconds = $IDLE" <<< "$said" || fail "$NAME's config.toml doesn't say idle_seconds = $IDLE: $(tail -c 300 <<< "$said")"
say "$(grep -m1 '^tensord ' <<< "$said")"
cozy machine keepalive "$NAME" > "$OUT/keepalive.txt" 2>&1 || fail "keepalive: $(tail -c 300 "$OUT/keepalive.txt")"
d0=0
for _ in 1 2 3 4 5 6; do
  d0=$(cozy machine show "$NAME" --json 2> /dev/null | jq -r '.status.idle_deadline_ms // 0'); (( ${d0:-0} >= $(date +%s%3N) + 840000 )) && break
  sleep 5
done
(( ${d0:-0} > 0 )) || fail "$NAME reports no idle deadline after a keepalive"
say "$NAME (Runtime $at) started its clock $(date -u -d "@$(((d0 - 900000) / 1000))" +%T), deadline $(date -u -d "@$((d0 / 1000))" +%T) on a 15 min window; idle now for $((IDLE + 20)) s"
sleep $((IDLE + 20))

# The update by hand (a Runtime move), a second behind it (back), and the run once the rental's own record shows an update
# unsettled.
t0=$(date +%s%3N)
(cozy --json machine update "$NAME" --runtime "$TO" -y > "$OUT/held-update.json" 2> "$OUT/held-update.err") 9>&- & UP=$!; BG="$BG $UP"
for _ in $(seq 240); do
  cozy machine show "$NAME" --json > "$OUT/show-update.json" 2> /dev/null
  jq -e '[.jobs[]? | select(.kind == "update" and (.status == "queued" or .status == "in_progress"))] | length > 0' "$OUT/show-update.json" > /dev/null 2>&1 && break
  kill -0 "$UP" 2> /dev/null || break
  sleep 0.5
done
(cozy --json machine update "$NAME" --runtime "$at" -y > "$OUT/queued-update.json" 2> "$OUT/queued-update.err") 9>&- & UQ=$!; BG="$BG $UQ"
say "the update went $(( $(date +%s%3N) - t0 )) ms ago, $NAME's clock $(( (t0 - d0 + 900000) / 1000 )) s old against a window of $IDLE s to come; the run is asked now"
step held run "$CODE/runtime" --on "$NAME" text=held; held=$?
wait "$UP"; up=$?; wait "$UQ"; uq=$?
say "the updates by hand ended $up and $uq$([ "$uq" = 0 ] || echo ": $(tail -c 200 "$OUT/queued-update.err" | tr '\n' ' ')")"
n=$(runno held)
[ -z "$n" ] || (cd "$RUN_PROJECT" && cozy --json run watch "$n" --once --full) > "$OUT/held.json" 2> "$OUT/held.err"
cozy machine show "$RID" --json > "$OUT/show-after.json" 2> /dev/null
now=$(date +%s%3N)
jq -r '.jobs[]? | select(.kind == "update") | "  update \(.number) \(.status) made \(.created_at_ms - '"$t0"') ms, accepted \((.accepted_at_ms // 0) - '"$t0"'), settled \((.settled_at_ms // 0) - '"$t0"') after the first was asked"' "$OUT/show-after.json" 2> /dev/null | tee -a "$R"
cozy machine logs "$NAME" --since 20m -n 400 2> /dev/null | grep -aE 'keepalive|idle|releas|restarting on|update run' | tail -30 | cut -c1-300 > "$OUT/held-machine.log" || true
waited=$(grep -oE "(updating $NAME first|waiting for $NAME to finish updating)" "$OUT/held.txt" | head -1)
status=$(jq -r '.status // empty' "$OUT/held.json" 2> /dev/null) on=$(jq -r '.machine // empty' "$OUT/held.json" 2> /dev/null)
d1=$(jq -r '.status.idle_deadline_ms // 0' "$OUT/show-after.json" 2> /dev/null)
left=$(( (${d1:-0} - now) / 1000 ))
say "run ${n:-none} ${status:-unrecorded} on ${on:-nothing}; $NAME is $(jq -r '.state // "gone"' "$OUT/show-after.json" 2> /dev/null), its deadline in $left s"
if [ -z "$waited" ]; then
  check "the run wasn't held behind $NAME's update, so this proves nothing: $(grep -E '^ +waiting' "$OUT/held.txt" | sort -u | head -3 | cut -c1-200 | paste -sd'|')"
elif [ "$held" != 0 ] || [ "$status" != completed ] || [ "$on" != "$NAME" ]; then
  check "the run held for $NAME's update didn't complete there: ${status:-unrecorded} on ${on:-nothing}: $(jq -r '[.reason.code, .reason.message] | map(select(. != null)) | join(": ")' "$OUT/held.json" 2> /dev/null | cut -c1-400)"
elif (( ${d1:-0} == 0 || left > IDLE + 5 || left <= 0 )); then
  check "control: after its restart $NAME's deadline is $left s away, not within its window of $IDLE s: the short window never took, so no deadline was outlasted"
else
  say "PASS: run $n waited ($waited), then completed on $NAME, whose clock was $(( (t0 - d0 + 900000) / 1000 )) s old against the $IDLE s window its restart opened; $NAME was not released"
fi
exit "$RC"
