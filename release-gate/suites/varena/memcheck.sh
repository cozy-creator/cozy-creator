# Sourced by cells.sh: compute-sanitizer's memcheck on SDXL, stock Diffusers against the same through vt.offload's graphs,
# in the fresh child process torch-offload starts. TensorD's launch seal keeps the sanitizer off an executor, which it would
# have to start as its child, so this never sees Runtime's loops, TensorFS's kernels, Qwen or H3: the differentials carry those.
memcheck_run() {
  local S=/root/memcheck A=./memcheck mode errors=0 stock= graphs=
  local tool=$S/lib/python3.12/site-packages/nvidia/cu13/bin/compute-sanitizer
  mpod() { timeout 300 "$CLI" machine ssh "$M" -- "$@"; }
  kinds() {  # each error once, its addresses out
    mpod "grep -E '^=+ (Invalid|Host API|Uninitialized)' $S/$1.log | sed -E 's/0x[0-9a-f]+/0x/g' | sort -u" < /dev/null
  }
  mpod "uv venv -q --allow-existing $S && uv pip install -q --python $S/bin/python 'nvidia-cuda-sanitizer-api>=13'" < /dev/null ||
    { expect memcheck FAIL "no compute-sanitizer on $M"; return; }
  mpod "cat > $S/wrap.sh; chmod +x $S/wrap.sh" <<POD || { expect memcheck FAIL "could not copy the wrapper to $M"; return; }
#!/bin/bash
exec $tool --tool memcheck --require-cuda-init no --target-processes all --leak-check no --report-api-errors no --print-limit 200 --log-file $S/\$(cat $S/mode).log "\$@"
POD
  rm -rf "$A"; mkdir -p "$A"; cp -r "$V/torch-offload" "$A/"
  sed -i "s#subprocess.Popen(\[sys.executable,#subprocess.Popen([\"$S/wrap.sh\", sys.executable,#" "$A/torch-offload/ci_torch_offload/__init__.py"
  for mode in stock graphs; do
    cap off  # stock Torch yields nothing: the card must be free
    mpod "echo $mode > $S/mode; rm -f $S/$mode.log" < /dev/null
    run_cell "$OUT" "memcheck-$mode" "$A/torch-offload/sdxl" --on "$M" mode="$mode" seed:=$((R + 81)) steps:=6 size:=512 --gpus=1 \
      'prompts:=["a tin robot on a shelf, photo", "a glass bird on a sill, photo"]'
    log "memcheck-$mode run $(cat "memcheck-$mode.number" 2>/dev/null) $(run_get "$OUT" "memcheck-$mode" .status)"
    errors=$(mpod "sed -n 's/^=* *ERROR SUMMARY: \([0-9]*\).*/\1/p' $S/$mode.log" < /dev/null | tail -n 1)
    [ -n "$errors" ] || { expect "memcheck-$mode" FAIL "no summary: the sanitizer did not run"; return; }
    log "memcheck-$mode: $errors errors"
  done
  stock=$(kinds stock); graphs=$(kinds graphs)
  graphs=$(comm -13 <(sort <<< "$stock") <(sort <<< "$graphs"))
  if [ -z "$graphs" ]; then expect memcheck PASS "nothing stock doesn't have"; else expect memcheck FAIL "${graphs:0:300}"; fi
}
[ -n "$ONLY" ] && [[ $ONLY != *,memcheck,* ]] || memcheck_run
