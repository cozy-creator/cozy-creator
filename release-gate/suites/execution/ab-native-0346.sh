# Same-pod A/B (release lane, proof-only): SDXL bf16 twice on one executor (the second must capture natively) on
# 0.34.5, 0.34.6 and 0.34.6 without #1857's allocator setting (5878df918), each arm after a Runtime update (a restart).
# The 0.34.6 staged-image sanity failed the second request on a 12 GB card: capture OOM (40528301770).
V346='https://github.com/cozy-creator/cozy-runtime/releases/download/v0.34.6/cozy_runtime-0.34.6-cp312-abi3-manylinux_2_28_x86_64.whl#sha256=926a76152067380febbdc389078c57ae2fb7cac97926f320f19f1c2994f05c49'
NOALLOC='https://github.com/cozy-creator/cozy-runtime/releases/download/noalloc1-0346-94c476b16/cozy_runtime-0.34.6%2Bnoalloc1-cp312-abi3-manylinux_2_28_x86_64.whl#sha256=b10f9dd9af6525a94b1fab026283bcdcdcd04cbb3d781feb7d3fb06e7cd1ff27'
pair() {  # pair <arm> <seed offset>
  cell "$1-1" "sdxl/generate" prompt="$SCENE, photo, take $1 one" seed:=$((R + $2)) --lane bf16
  cell "$1-2" "sdxl/generate" prompt="$OBJECT on a shelf, photo, take $1 two" seed:=$((R + $2 + 1)) --lane bf16
  local nat; nat=$(jq -r '[.. | objects | select(has("stages")) | .stages | select(has("native_pipeline")) | .native_pipeline.count] | add // 0' "$1-2.json" 2>/dev/null)
  expect "$1-native" "$([ "$(run_get "$OUT" "$1-2" .status)" = completed ] && [ "${nat:-0}" -gt 0 ] && echo PASS || echo FAIL)" \
    "$(run_get "$OUT" "$1-2" .status), native ${nat:-0}; $(grep -m1 -oE 'memory allocation failed[^"]{0,140}' "$1-2.ndjson" 2>/dev/null)"
}
to() { local f; f=$(fetch "$1") && "$UP" --json machine update "$M" --runtime "$f" -y > "ab-$2.json" 2>&1; log "arm $2: $(jq -c 'select(.machine and .runtime) | .runtime' "ab-$2.json" 2>/dev/null)"; }
pair x 501
to "$V346" y && pair y 511
to "$NOALLOC" z && pair z 521
