# idle-hub-silent (cl-778): a rental stops billing when the Hub goes silent during its idle window. Read from the code on
# 2026-11-09: an idle pod ended itself only when the Hub had been silent since before the window began, so a Hub that died
# during it left the pod asking for its release every minute for as long as the Hub was down. One pod with SSH (`cpu` is
# enough: it runs nothing), the session's cozy as TensorD. `silent_s` into its idle window a root shell names the Hub's host
# in /etc/hosts at an address nothing answers; the provider stays reachable, as in a Hub outage. The pod must then:
#   say its release is not accepted once its deadline passes (the Hub is silent to it; if the Hub accepts it, the silence
#     never took: a connection TensorD held outlived the hosts line, and the session fails saying so)
#   end itself at its provider, and be gone within one idle window of its deadline (plus `slack_s` for the Hub to see it);
#     its own last line goes with it, so the log read every 20 s may not hold it
# Before the fix it is still there then, and the session's trap ends it.
# params:
#   silent_s   seconds into the idle window when the Hub goes silent (default 300)
#   slack_s    seconds past deadline + window allowed for the Hub to record the pod gone (default 300)
. "$(dirname "$0")/lib.sh"
[ -f ~/.ssh/id_ed25519.pub ] || ssh-keygen -q -t ed25519 -N '' -f ~/.ssh/id_ed25519 || fail "ssh-keygen"
RENT_ARGS=--ssh
SILENT=$(param silent_s 300) SLACK=$(param slack_s 300) WINDOW=900
quote $((1200 + 2 * WINDOW + SLACK))
rent

sh() { cozy machine ssh "$NAME" -- "$*" < /dev/null 2>> "$OUT/ssh.err"; }
mend() { sh 'sed -i "/ cl778$/d" /etc/hosts'; }
trap 'mend; finish' EXIT
step update machine update "$NAME" --tensord "$(command -v cozy)" -y || fail "update"
HUB=$(sh 'for p in /proc/[0-9]*; do tr "\0" "\n" < $p/environ 2>/dev/null | sed -n "s|^TENSORHUB_ORIGIN=https://||p"; done | sort -u | head -1' | cut -d: -f1)
[ -n "$HUB" ] || fail "no process on the pod names its Hub (TENSORHUB_ORIGIN)"
deadline() { cozy machine show "$NAME" --json 2>/dev/null | jq -r '.status.idle_deadline_ms // empty'; }
D=$(deadline); [ -n "$D" ] || fail "the pod names no idle deadline"
sleep "$SILENT"
sh "printf '127.0.0.9 %s cl778\n' '$HUB' >> /etc/hosts && getent hosts '$HUB' | grep -q 127.0.0.9" || fail "the Hub's host did not move"
D=$(deadline); [ -n "$D" ] || fail "the pod names no idle deadline"
T=$(date +%s); D=$((D / 1000))
say "t=0 $HUB is silent to the pod; its idle deadline is in $((D - T))s"
[ $((D - T)) -gt 60 ] && [ $((D - T)) -lt "$WINDOW" ] || fail "the Hub did not go silent inside the idle window"
until gone "$RID"; do
  now=$(date +%s)
  cozy machine logs "$NAME" tensord -n 0 2>/dev/null | grep -E 'idle release|accepted the release|ending (it|this)|provider' > "$OUT/lifecycle.log.new" \
    && [ -s "$OUT/lifecycle.log.new" ] && mv "$OUT/lifecycle.log.new" "$OUT/lifecycle.log"
  [ "$now" -lt $((D + WINDOW + SLACK)) ] || fail "still billing $((now - D))s past its deadline: $(tail -1 "$OUT/lifecycle.log" 2>/dev/null)"
  sleep 20
done
E=$(date +%s)
say "gone t=$((E - T)), $((E - D))s past its deadline: $(tail -2 "$OUT/lifecycle.log" 2>/dev/null | tr '\n' ' ' | cut -c1-300)"
! grep -q 'accepted the release' "$OUT/lifecycle.log" 2>/dev/null || fail "the Hub accepted its release: it was never silent to the pod"
grep -q 'idle release not accepted' "$OUT/lifecycle.log" 2>/dev/null || fail "the pod never said its release went unaccepted"
[ "$E" -ge "$D" ] || fail "gone $((D - E))s before its deadline"
say "PASS: silent from $((D - T))s before its deadline, the pod ended itself and was gone $((E - D))s past it (one window is ${WINDOW}s)"
