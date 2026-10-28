# Runtime 0.35.1 + TensorFS 0.17.6 pre-roll sanity, one GPU (release lane, proof-only). Sourced by gate.sh --cells after
# the runtime suite's selected cells (H3 on its fixed request): the loop cells, SDXL bf16 twice, Krea 2 Raw and Turbo
# with a caller LoRA, the H3 receipt; then krea-ab: Krea 2 Turbo at 2048^2 (cold, two warm) in fresh executors on the
# candidate, and the same on Runtime 0.35.0 / TensorFS 0.17.5 after `machine update` on this pod.
. "$HERE/suites/execution/h3-route.sh"
. "$HERE/suites/execution/core-cells.sh"
S=$(pkg sdxl generate)
cell sdxl-bf16-1 "$S" prompt="$SCENE, photo, first light" seed:=$((R + 371)) --lane bf16
cell sdxl-bf16-2 "$S" prompt="$OBJECT on a windowsill, photo, rain outside" seed:=$((R + 372)) --lane bf16
if [ -f sdxl-bf16-2.json ]; then
  pid1=$(jq -r '.measurements.execution.gpus[0].pid // empty' sdxl-bf16-1.json 2>/dev/null)
  pid2=$(jq -r '.measurements.execution.gpus[0].pid // empty' sdxl-bf16-2.json 2>/dev/null)
  st=$(run_get "$OUT" sdxl-bf16-2 .status)
  expect sdxl-bf16-second "$([ "$st" = completed ] && [ -n "$pid1" ] && [ "$pid1" = "$pid2" ] && echo PASS || echo FAIL)" \
    "request 2 $st, executor $pid1 -> $pid2; loop $(run_get "$OUT" sdxl-bf16-2 '.measurements.execution.loops[0] | "\(.mode) native \(.native_steps) reused \(.reused)"')"
fi
K=$(pkg krea-2 generate) KT=$(pkg krea-2 generate_turbo)
cell krea "$K" prompt="$SCENE, photo" seed:=$((R + 381))
cell krea-turbo-lora "$KT" prompt="$OBJECT, ink wash painting" seed:=$((R + 383)) \
  --lora "model:transformer=fidika/krea-2-darkbrush@sha256:95afffbed63209deecd40744849fa42e8ed61aaada1e52b09c033d447d428ed0,1.0"
if [ -f h3.json ]; then
  st=$(run_get "$OUT" h3 .status)
  expect h3-route "$([ "$st" = completed ] && echo PASS || echo FAIL)" \
    "$st; LoRA $(lora h3); served $(served h3); exact $(exact_sites h3); vae $(vae h3); decode $(decode_s h3) s"
fi
# krea-ab: the candidate first, each side from a restart so both start cold.
PREV_RT=https://files.pythonhosted.org/packages/7d/56/63ba4a74abe5b2546d71dc2ab5edf5bb6bc4a580493f7260e8add565110d/cozy_runtime-0.35.0-cp312-abi3-manylinux_2_28_x86_64.whl
PREV_RT_SHA=72ff113bd5a8f4ad4e90db2d005bc4f093ae5b9ed8e0400f9acc943ccb331107
PREV_TFS=https://github.com/cozy-creator/cozy-creator/releases/download/tensorfs-v0.17.5/tensorfs-0.17.5-cp311-abi3-manylinux_2_17_x86_64.manylinux2014_x86_64.whl
PREV_TFS_SHA=7dc8316e8638ea40de6ce8dfe6b29c7085af38b6089f235e3c86f453aa7dcf30
warm() { jq -r '.execution_ms // 0 | . / 1000 | . * 10 | round / 10' "$1.json" 2>/dev/null; }
if [ -z "$ONLY" ] || [[ $ONLY == *",krea-ab,"* ]]; then
  "$CLI" machine update "$M" --gpu-memory-limit off -y > krea-ab-restart.log 2>&1
  for i in 1 2 3; do cell "krea-new-$i" "$KT" prompt="$OBJECT, Krea call $i, studio photo" seed:=$((R + 400 + i)); done
  mkdir -p prev && curl -sfLo "prev/${PREV_RT##*/}" "$PREV_RT" \
    && curl -sfLo "prev/${PREV_TFS##*/}" "$PREV_TFS" \
    && echo "$PREV_RT_SHA  prev/${PREV_RT##*/}" | sha256sum -c --quiet && echo "$PREV_TFS_SHA  prev/${PREV_TFS##*/}" | sha256sum -c --quiet \
    && "$CLI" machine update "$M" --runtime "prev/${PREV_RT##*/}" --tensorfs "prev/${PREV_TFS##*/}" -y > krea-ab-prev.log 2>&1
  prev=$("$CLI" --json machine show "$M" 2>/dev/null | jq -r '"\(.status.runtime)/\(.status.tensorfs)"')
  for i in 1 2 3; do cell "krea-prev-$i" "$KT" prompt="$OBJECT, Krea call $i on the last release, studio photo" seed:=$((R + 410 + i)); done
  ok=PASS; for c in krea-new-1 krea-new-2 krea-new-3 krea-prev-1 krea-prev-2 krea-prev-3; do [ "$(run_get "$OUT" $c .status)" = completed ] || ok=FAIL; done
  [ "$prev" = 0.35.0/0.17.5 ] || ok=FAIL
  expect krea-ab $ok "candidate cold $(warm krea-new-1) warm $(warm krea-new-2)/$(warm krea-new-3) s; $prev cold $(warm krea-prev-1) warm $(warm krea-prev-2)/$(warm krea-prev-3) s"
fi
