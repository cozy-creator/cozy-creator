# ssh-detach: does `cozy machine ssh` return when its command leaves a detached process, before and after the GPU-freeing
# restart comfyui.sh makes (`machine update --gpu-memory-limit off`)? Two LTX gates' ComfyUI passes (44411754512,
# 44411764104) never saw their runner start: one call hung for its whole timeout after that restart. Each call is timed.
. "$(dirname "$0")/lib.sh"
RENT_ARGS=--ssh
[ -f ~/.ssh/id_ed25519.pub ] || ssh-keygen -q -t ed25519 -N '' -f ~/.ssh/id_ed25519 || fail "ssh-keygen"
quote 900
rent
pod() { timeout "${T:-120}" cozy machine ssh "$NAME" "$@" < /dev/null; }
timed() { local what=$1 t=$SECONDS; shift; "$@" > "$OUT/$what.out" 2>&1; local rc=$?; say "$what: exit $rc after $((SECONDS - t)) s: $(tail -c 200 "$OUT/$what.out" | tr '\n' ' ')"; return $rc; }
for i in $(seq 1 20); do pod true > /dev/null 2>&1 && break; [ "$i" = 20 ] && fail "no SSH into $NAME"; sleep 15; done
probe() {  # $1: a label
  T=120 timed "$1-plain" pod "true"
  T=120 timed "$1-braces" pod "cd /var/tmp && { setsid nohup sleep 900 > /var/tmp/$1-a.log 2>&1 < /dev/null & echo \$! > /var/tmp/$1-a.pid; }"
  T=120 timed "$1-started" pod "test -s /var/tmp/$1-b.pid || { cd /var/tmp && setsid nohup sleep 900 > $1-b.log 2>&1 < /dev/null & echo \$! > /var/tmp/$1-b.pid; }; test -s /var/tmp/$1-b.pid"
  T=120 timed "$1-again" pod "test -s /var/tmp/$1-b.pid && ps -o pid,sess,args -p \$(cat /var/tmp/$1-a.pid) -p \$(cat /var/tmp/$1-b.pid)"
}
probe before
timed restart cozy machine update "$NAME" --gpu-memory-limit off -y
for i in $(seq 1 15); do T=60 timed "after-restart-$i" pod "true" && break; sleep 20; done
probe after
say "done"
