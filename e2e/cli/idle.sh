# xs-189: what an idle cozy costs. Calcifer and TensorD with nothing to do, then again with an idle executor after one
# weightless run: each cozy process's CPU time and wakeups over 60 s, after 30 s to settle, and its threads by name
# (idle.py; idle.json and idle-warm.json). By dispatch: a PR that changes a loop puts its before and after in its Proof.
idle_cost() {
[ "$RUNNER_OS" = Linux ] || return 0
step "an idle Calcifer and TensorD: CPU time and wakeups over 60 s (xs-189)"
python3 "$here/idle.py" 30 60 "$OUT/idle.json" 2> "$OUT/idle.err" || check "idle: $(tail -c 300 "$OUT/idle.err")"
step "the same with an idle executor (xs-189)"
cell idle-warm "$here/weightless/reverse" "text=idle$RANDOM" || return 0
python3 "$here/idle.py" 30 60 "$OUT/idle-warm.json" 2> "$OUT/idle-warm.err" || check "idle-warm: $(tail -c 300 "$OUT/idle-warm.err")"
}
