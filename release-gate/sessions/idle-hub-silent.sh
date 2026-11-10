# idle-hub-silent (cl-778): a rental stops billing when the Hub goes silent during its idle window. Read from the code on
# 2026-11-09: an idle pod ended itself only when the Hub had been silent since before the window began, so a Hub that died
# during it left the pod asking for its release every minute for as long as the Hub was down. One pod with SSH (`cpu` is
# enough: it runs nothing), the session's cozy as TensorD. `silent_s` into its idle window a root shell names the Hub's host
# in /etc/hosts at an address nothing answers, drops the Hub's addresses at the pod's firewall where the container allows
# it, and TensorD restarts, so no open connection outlives the silence; the provider stays reachable, as in a Hub outage.
# The pod must then, by its own log (followed: its last lines go with it):
#   not have its release accepted (if the Hub accepts it, the silence never took)
#   say it is ending itself, and be gone within one idle window of its deadline (plus `slack_s`), which the Hub records as
#     `failed` (the provider says it is gone and the Hub did not end it), never `released` (the Hub's own end)
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
# Its addresses dropped at the pod's firewall too, so an open connection dies with the silence. A container without
# NET_ADMIN refuses that; the restart below silences it then: TensorD keeps its connection to the Hub open (keepalive),
# which a hosts line never touches (42748695490: the Hub accepted the release), and a restart opens new ones at the address
# nothing answers. A rental's restart keeps its idle clock.
IPS=$(getent ahosts "$HUB" | awk '{print $1}' | sort -u | paste -sd' ')
FW=$(sh "ok=; for ip in $IPS; do case \$ip in *:*) t=ip6tables;; *) t=iptables;; esac; \$t -I OUTPUT -d \$ip -j DROP 2>&1 && ok=1; done; [ -n \"\$ok\" ] && echo dropped || echo refused") || true
say "firewall on the pod for $HUB ($IPS): ${FW:-no answer}"
step restart machine update "$NAME" --gpu-memory-limit off -y || fail "the restart, with the Hub silent: $(tail -c 300 "$OUT/restart.txt")"
# The pod's own log, followed: its last lines go with it, which a poll can miss (42750948253 read nothing).
cozy machine logs "$NAME" tensord -f -n 20 > "$OUT/tensord-follow.log" 2>&1 < /dev/null & BG=$!
D=$(deadline); [ -n "$D" ] || fail "the pod names no idle deadline"
T=$(date +%s); D=$((D / 1000))
say "t=0 $HUB is silent to the pod; its idle deadline is in $((D - T))s"
[ $((D - T)) -gt 60 ] && [ $((D - T)) -le "$WINDOW" ] || fail "the Hub did not go silent inside the idle window"
until gone "$RID"; do
  now=$(date +%s)
  [ "$now" -lt $((D + WINDOW + SLACK)) ] || fail "still billing $((now - D))s past its deadline: $(grep -E 'idle release|ending|provider' "$OUT/tensord-follow.log" | tail -1)"
  sleep 10
done
E=$(date +%s)
sleep 5; grep -E 'idle release|accepted the release|ending (it|this)|provider' "$OUT/tensord-follow.log" > "$OUT/lifecycle.log"
# The Hub's record: `released` when it ended the rental itself, `failed` when the provider says the pod is gone and the
# Hub did not end it (readiness.provider_lost): the pod's own end.
HUBSTATE=$(cozy --json machine ls --since 2h 2> /dev/null | jq -r --arg r "$RID" '.rentals[] | select(.rental_id == $r) | .state')
say "gone t=$((E - T)), $((E - D))s past its deadline; the Hub records it ${HUBSTATE:-?}; the pod said: $(tail -3 "$OUT/lifecycle.log" | tr '\n' ' ' | cut -c1-400)"
! grep -q 'accepted the release' "$OUT/lifecycle.log" || fail "the Hub accepted its release: it was never silent to the pod"
grep -q 'ending it itself' "$OUT/lifecycle.log" || fail "the pod never said it ended itself (the Hub records it ${HUBSTATE:-?})"
[ "$HUBSTATE" != released ] || fail "the Hub ended it (released), not the pod"
[ "$E" -ge "$D" ] || fail "gone $((D - E))s before its deadline"
say "PASS: silent from $((D - T))s before its deadline, the pod ended itself through its provider and was gone $((E - D))s past it (one window is ${WINDOW}s); the Hub records it ${HUBSTATE:-?}"
