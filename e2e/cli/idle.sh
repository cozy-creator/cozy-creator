# xs-189: what an idle cozy costs. Calcifer and TensorD with nothing to do, again with an idle executor after one
# weightless run, and again after a model's download: each cozy process's CPU time and wakeups over 60 s, after 30 s to
# settle, and its threads by name (idle.py; idle.json, idle-warm.json, idle-downloaded.json). By dispatch: a PR that
# changes a loop puts its before and after in its Proof.
idle_cost() {
[ "$RUNNER_OS" = Linux ] || return 0
step "an idle Calcifer and TensorD: CPU time and wakeups over 60 s (xs-189)"
python3 "$here/idle.py" 30 60 "$OUT/idle.json" 2> "$OUT/idle.err" || check "idle: $(tail -c 300 "$OUT/idle.err")"
step "the same with an idle executor (xs-189)"
cell idle-warm "$here/weightless/reverse" "text=idle$RANDOM" || return 0
python3 "$here/idle.py" 30 60 "$OUT/idle-warm.json" 2> "$OUT/idle-warm.err" || check "idle-warm: $(tail -c 300 "$OUT/idle-warm.err")"
step "the same after a model's download (xs-189)"
cozy --json model install lilylilith/fontbox --on local > "$OUT/idle-download.ndjson" 2>&1 \
  || { check "idle-downloaded: model install failed: $(tail -c 300 "$OUT/idle-download.ndjson")"; return 0; }
python3 "$here/idle.py" 30 60 "$OUT/idle-downloaded.json" 2> "$OUT/idle-downloaded.err" || check "idle-downloaded: $(tail -c 300 "$OUT/idle-downloaded.err")"
}
