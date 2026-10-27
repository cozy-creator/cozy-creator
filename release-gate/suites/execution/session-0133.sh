# One RTX 4090 session (release lane): the cozy 0.13.3 pre-roll sanity on the staged image (SDXL then Qwen 4MP,
# conditional-body from the runtime suite, SDXL bf16 twice with the second native), then on the same rental cozy-runtime
# #1869's proof: its wheel (0.34.5+choosepad1) goes on, one SDXL int8 request, and the kernel-regret measurement.
# kernel-regret reads FAIL by design (its own kernel builds count as request-path blocking): judge it by its numbers.
. "$HERE/suites/execution/sdxl-native-twice.sh"
CHOOSE=https://github.com/cozy-creator/cozy-runtime/releases/download/choosepad1-81ada295d/cozy_runtime-0.34.5%2Bchoosepad1-cp312-abi3-manylinux_2_28_x86_64.whl#sha256=cd5df446a3ef0e96a078a8cad34a2c9fc42e1d75c0683ae1b1791903b3df0d2b
if [ -z "$ONLY" ] || [[ $ONLY == *",kernel-regret,"* || $ONLY == *",sdxl-int8-choosepad,"* ]]; then
  if f=$(fetch "$CHOOSE"); then
    "$UP" --json machine update "$M" --runtime "$f" -y > choosepad-update.json 2> choosepad-update.log
    log "choosepad1 on $M: exit $? $(jq -c 'select(.machine and .runtime)' choosepad-update.json 2>/dev/null)"
    cell sdxl-int8-choosepad "$(pkg sdxl generate)" prompt="$OBJECT on a marble counter, photo" seed:=$((R + 391)) --lane int8
    . "$HERE/suites/execution/kernel-regret/cells.sh"
  else
    expect choosepad1 FAIL "cannot fetch the #1869 wheel"
  fi
fi
