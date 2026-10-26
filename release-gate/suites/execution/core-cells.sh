# Selected existing native and bounded-pressure cells for the Runtime 0.34 / TensorFS 0.17 cohort.
# Sourced by gate.sh --cells after the selected model suite; --only still applies to these names.
# No H3 request, new fixture body, independent rental or SDK update is introduced here.
CORE_NATIVE_TARGET=$HERE/suites/execution/native-load-execute/gpu
CORE_MEMORY_TARGET=$HERE/suites/execution/bounded-memory-policy/gpu

cell native-loaded "$CORE_NATIVE_TARGET" mode=loaded ordering=events seed:=$((R + 301)) --gpus=1
cell native-input-error "$CORE_NATIVE_TARGET" mode=input-error ordering=events seed:=$((R + 302)) --gpus=1
cell native-cancel-active "$CORE_NATIVE_TARGET" mode=cancel-active ordering=events seed:=$((R + 303)) --gpus=1
cell native-phases "$CORE_NATIVE_TARGET" mode=phase-both ordering=events seed:=$((R + 304)) --gpus=1
cell memory-bounded "$CORE_MEMORY_TARGET" mode=bounded seed:=$((R + 305)) --gpus=1
cell memory-retry "$CORE_MEMORY_TARGET" mode=retry seed:=$((R + 306)) --gpus=1
cell memory-capture "$CORE_MEMORY_TARGET" mode=capture seed:=$((R + 307)) --gpus=1
