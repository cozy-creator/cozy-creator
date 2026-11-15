#!/usr/bin/env bash
# restart (cl-977): a rental's TensorD restarts, updates and stops within a bound, and the rental ends when told. One GPU pod with
# SSH, the session's cozy as TensorD. Each step must end within `bound_s`, leaving the machine ready on the session's cozy with its
# GPU listed (a child process opened it), nothing stuck and not broken:
#   update   `machine update --tensord <the session's cozy>`: the image's TensorD restarts onto it
#   restart  `machine update --gpu-memory-limit off`: the new TensorD's own update restart
#   term     its service sent SIGTERM over SSH, as a stop reaches it: its supervisor starts it again
#   kill     its service sent SIGKILL: the same
#   end      `machine end`: its Hub ends it and it is gone
# A plain restart kills nothing and finds nothing stuck (its log). The driver bug that made muromisan's TensorD unable to exit can't
# be forced: e2e's `stuck` shard holds a TensorD's exit in the kernel instead.
# params: bound_s (default 180)
. "$(dirname "$0")/lib.sh"
[ -f ~/.ssh/id_ed25519.pub ] || ssh-keygen -q -t ed25519 -N '' -f ~/.ssh/id_ed25519 || fail "ssh-keygen"
RENT_ARGS=--ssh
BOUND=$(param bound_s 180)
WANT=$(cozy --version | awk '{print $2}')
quote 1500
rent

sh() { cozy machine ssh "$NAME" -- "$*" < /dev/null 2>> "$OUT/ssh.err"; }
# The service: a tensord whose parent is the tensord supervising it.
service() { sh 'for p in $(pgrep -x tensord); do q=$(ps -o ppid= -p "$p" | tr -d " "); [ "$(cat /proc/$q/comm 2>/dev/null)" = tensord ] && echo "$p"; done' | tail -1; }
# settled <step>: within BOUND of T0, ready on the session's cozy, its GPU listed, nothing stuck, not broken.
settled() {
  local s=$1 took f=$OUT/$1.show.json
  for _ in $(seq "$BOUND"); do
    cozy machine show "$NAME" --json > "$f" 2> /dev/null < /dev/null || true
    jq -e --arg v "$WANT" '.state == "ready" and (.unreachable // "") == "" and .status.version == $v' "$f" > /dev/null && break
    sleep 1
  done
  took=$(($(date +%s) - T0))
  jq -e --arg v "$WANT" '.state == "ready" and .status.version == $v' "$f" > /dev/null \
    || fail "$s: not ready on cozy $WANT ${took}s later: $(jq -c '{state, unreachable, version: .status.version}' "$f")"
  [ "$took" -le "$BOUND" ] || fail "$s: took ${took}s (bound ${BOUND}s)"
  jq -e '(.status.gpus // []) | length >= 1' "$f" > /dev/null || fail "$s: no GPU listed"
  jq -e '(.status.stuck // []) | length == 0' "$f" > /dev/null || fail "$s: stuck: $(jq -c .status.stuck "$f")"
  jq -e '.broken == null' "$f" > /dev/null || fail "$s: broken: $(jq -c .broken "$f")"
  say "$s: ready on cozy $WANT in ${took}s, $(jq -r '.status.gpus[0].name' "$f"), nothing stuck, not broken"
}

T0=$(date +%s); step update machine update "$NAME" --tensord "$(command -v cozy)" -y || fail "update: $(tail -c 300 "$OUT/update.txt")"
settled update
T0=$(date +%s); step restart machine update "$NAME" --gpu-memory-limit off -y || fail "restart: $(tail -c 300 "$OUT/restart.txt")"
settled restart
for sig in TERM KILL; do
  pid=$(service)
  [ -n "$pid" ] || fail "no TensorD service on the pod: $(sh 'ps -eo pid,ppid,comm' | head -20)"
  T0=$(date +%s)
  sh "kill -$sig $pid" || fail "kill -$sig $pid"
  sleep 3
  settled "${sig,,}"
  now=$(service)
  [ -n "$now" ] && [ "$now" != "$pid" ] || fail "$sig: the service is ${now:-gone}, was $pid"
done
cozy machine logs "$NAME" tensord -n 1000 > "$OUT/tensord.log" 2>&1 < /dev/null || true
! grep -E "did not exit within|can't exit" "$OUT/tensord.log" || fail "a plain restart needed a kill, or found a TensorD stuck"
T0=$(date +%s)
step end machine end "$NAME" -y || fail "end: $(tail -c 300 "$OUT/end.txt")"
until gone "$RID"; do [ $(($(date +%s) - T0)) -le 300 ] || fail "end: still listed 300 s after machine end"; sleep 5; done
say "end: gone $(($(date +%s) - T0))s after machine end"
