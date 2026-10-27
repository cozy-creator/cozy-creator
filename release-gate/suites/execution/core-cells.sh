# Selected native, kernel readiness and bounded-pressure cells for the Runtime 0.34 / TensorFS 0.17 cohort.
# Sourced by gate.sh --cells after the selected model suite; --only still applies to these names.
# No H3 request, independent rental or SDK update is introduced here.
CORE_NATIVE_TARGET=$HERE/suites/execution/native-load-execute/gpu
CORE_MEMORY_TARGET=$HERE/suites/execution/bounded-memory-policy/gpu
CORE_READY_TARGET=$HERE/suites/execution/kernel-ready-publication/cpu

cell native-loaded "$CORE_NATIVE_TARGET" mode=loaded ordering=events seed:=$((R + 301)) --gpus=1
cell native-input-error "$CORE_NATIVE_TARGET" mode=input-error ordering=events seed:=$((R + 302)) --gpus=1
cell native-cancel-active "$CORE_NATIVE_TARGET" mode=cancel-active ordering=events seed:=$((R + 303)) --gpus=1
cell native-phases "$CORE_NATIVE_TARGET" mode=phase-loaded ordering=events seed:=$((R + 304)) --gpus=1
cell kernel-ready-late-success "$CORE_READY_TARGET" mode=late-success seed:=$((R + 311))
cell kernel-ready-late-error "$CORE_READY_TARGET" mode=late-error seed:=$((R + 312))
cell kernel-ready-dropped-slot "$CORE_READY_TARGET" mode=dropped-slot seed:=$((R + 313))
cell memory-bounded "$CORE_MEMORY_TARGET" mode=bounded seed:=$((R + 305)) --gpus=1
cell memory-retry "$CORE_MEMORY_TARGET" mode=retry seed:=$((R + 306)) --gpus=1
cell memory-capture "$CORE_MEMORY_TARGET" mode=capture seed:=$((R + 307)) --gpus=1
cell memory-physical-retry "$CORE_MEMORY_TARGET" mode=physical-retry seed:=$((R + 308)) --gpus=1
