#!/usr/bin/env bash
# hang (cl-977 G9): a GPU call that deadlocks inside its steps fails `hung`, and a long GPU step is never touched. One GPU pod with
# SSH, the session's cozy as TensorD, and dev builds of the Runtime (whose step sampler is a `~` watcher thread) and TensorFS
# (whose varena donor is one):
#   stuck  two real steps, then a lock it holds inside the third, its GPU idle and the step sampler (`~host-stalls`) ticking: it
#          fails `hung` within 60 s + `slack_s`; the next call runs in a fresh executor
#   long   a step that is a single GPU kernel for `long_s`, with no word: it completes
# Then the machine is ready, nothing stuck, not broken, and idle.
# params: runtime (a wheel URL or a cozy-runtime dev-build run id), tensorfs (a wheel URL or a cozy-creator dev-build run id),
#         long_s (default 90), slack_s (default 60)
. "$(dirname "$0")/lib.sh"
FIX=$(cd "$(dirname "$0")/fixtures" && pwd)
[ -f "$FIX/hang/hang/__init__.py" ] || fail "no fixture checkout"
[ -f ~/.ssh/id_ed25519.pub ] || ssh-keygen -q -t ed25519 -N '' -f ~/.ssh/id_ed25519 || fail "ssh-keygen"
RENT_ARGS=--ssh
LONG=$(param long_s 90) SLACK=$(param slack_s 60)
# wheel <kind> <source>: the manylinux wheel at a URL, or in a dev-build run's artifact; fetched before renting.
wheel() {
  local d=$OUT/wheels/$1; mkdir -p "$d"
  case $2 in
    https://*) curl -fsSL --retry 5 -o "$d/${2##*/}" "$2" || return 1 ;;
    *) case $1 in
         runtime) gh run download "$2" -R cozy-creator/cozy-runtime -p 'cozy-runtime-*' -D "$d" ;;
         *) gh run download "$2" -R cozy-creator/cozy-creator -p 'tensorfs-wheel-*' -D "$d" ;;
       esac > /dev/null 2>&1 || return 1 ;;
  esac
  find "$d" -name '*manylinux*x86_64*.whl' | head -1
}
UPDATE=()
for kind in runtime tensorfs; do
  src=$(param "$kind"); [ -n "$src" ] || fail "no $kind: params.$kind"
  f=$(wheel "$kind" "$src") && [ -n "$f" ] || fail "no $kind wheel from $src"
  say "$kind: ${f##*/}"; UPDATE+=("--$kind" "$f")
done
quote 1800
rent

sh() { cozy machine ssh "$NAME" -- "$*" < /dev/null 2>> "$OUT/ssh.err"; }
show() { cozy machine show "$NAME" --json 2> /dev/null < /dev/null; }
step update machine update "$NAME" --tensord "$(command -v cozy)" "${UPDATE[@]}" -y || fail "update: $(tail -c 300 "$OUT/update.txt")"
show > "$OUT/update.show.json"
say "pod: TensorD $(jq -r .status.version "$OUT/update.show.json"), Runtime $(jq -r .status.runtime "$OUT/update.show.json"), TensorFS $(jq -r .status.tensorfs "$OUT/update.show.json")"
cd "$FIX" || fail "no fixtures"

# stuck, followed in the background: its executor's threads are read over SSH while it hangs.
(step stuck run ./hang/stuck --on "$NAME" seed:=$RANDOM) & SP=$!
T0='' S=''
for _ in $(seq 600); do
  S=$(runno stuck)
  [ -n "$S" ] && [ "$(cozy run watch "$S" --once --json < /dev/null 2> /dev/null | jq -r '.progress.completed // 0')" -ge 2 ] && { T0=$(date +%s); break; }
  kill -0 "$SP" 2> /dev/null || break
  sleep 2
done
[ -n "$T0" ] || T0=$(date +%s)
sleep 10
threads=$(sh 'cat /proc/sys/kernel/yama/ptrace_scope; grep CapEff /proc/$(pgrep -xo tensord)/status; for p in $(pgrep -f cozy_runtime.internal.executor); do for t in /proc/$p/task/*; do echo "$p $(cat $t/comm) $(cat $t/wchan) $(head -c 24 $t/syscall 2>&1)"; done; done')
echo "$threads" > "$OUT/stuck.threads"
say "stuck: its executor's threads while it hangs: $(awk '{print $2"("$3")"}' "$OUT/stuck.threads" | sort | uniq -c | tr -s ' \n' ' ')"
grep -q ' ~host-stalls ' "$OUT/stuck.threads" || fail "stuck: no ~host-stalls thread ran while it hung: the Runtime has no named step sampler"
cozy machine logs "$NAME" -n 0 > "$OUT/stuck.log" 2>&1 < /dev/null
grep -q "its vigil's line is up" "$OUT/stuck.log" || fail "stuck: no executor's vigil line came up (TensorD's log)"
wait "$SP"
took=$(($(date +%s) - T0))
S=$(runno stuck)
cozy run watch "$S" --once --json --full > "$OUT/stuck.json" 2>&1 < /dev/null
say "stuck: run $S $(jq -c '{status, reason: .reason.code, message: .reason.message}' "$OUT/stuck.json") about ${took}s after its second step"
jq -e '.status == "failed" and .reason.code == "hung"' "$OUT/stuck.json" > /dev/null || fail "stuck: run $S isn't failed hung"
[ "$took" -le $((60 + SLACK)) ] || fail "stuck: it took ${took}s to fail (60 s + ${SLACK}s slack)"

step again run ./hang/long --on "$NAME" seed:=1 seconds:=1 || fail "again: a call after the hung one: $(tail -c 300 "$OUT/again.txt")"
say "again: run $(runno again) completed in a fresh executor"

T0=$(date +%s)
step long run ./hang/long --on "$NAME" seed:=2 "seconds:=$LONG" || fail "long: $(tail -c 300 "$OUT/long.txt")"
L=$(runno long)
cozy run watch "$L" --once --json --full > "$OUT/long.json" 2>&1 < /dev/null
jq -e '.status == "completed"' "$OUT/long.json" > /dev/null || fail "long: run $L is $(jq -c '{status, reason}' "$OUT/long.json")"
say "long: run $L completed, its GPU step $(jq -r '.result.value | floor' "$OUT/long.json") s, $(($(date +%s) - T0)) s in all"

for _ in $(seq 60); do show > "$OUT/after.json"; jq -e '.status.idle_deadline_ms > 0' "$OUT/after.json" > /dev/null && break; sleep 2; done
jq -e '.state == "ready" and (.status.stuck // []) == [] and .broken == null and .status.idle_deadline_ms > 0' "$OUT/after.json" > /dev/null \
  || fail "after: $(jq -c '{state, stuck: .status.stuck, broken, idle: .status.idle_deadline_ms}' "$OUT/after.json")"
say "after: ready, nothing stuck, not broken, idle"
