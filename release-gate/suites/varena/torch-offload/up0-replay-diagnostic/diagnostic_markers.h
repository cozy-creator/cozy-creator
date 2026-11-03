#ifndef XS138_DIAGNOSTIC_MARKERS_H
#define XS138_DIAGNOSTIC_MARKERS_H
#include <stdint.h>

/* Primitive host markers. Snapshot markers1..8 are a separate namespace.
 * RETURN means host return only. CACHE_RETIRE_RETURN is logical retirement,
 * never physical release or GPU completion. */
enum xs138_trace_marker {
    XS138_DISPATCH_ENTER = 100,
    XS138_DISPATCH_RETURN = 101,
    XS138_DISPATCH_ERROR = 102,
    XS138_USE_PREPARE_ENTER = 110,
    XS138_USE_PREPARE_RETURN = 111,
    XS138_USE_PREPARE_ERROR = 112,
    XS138_WAIT_READY_ENTER = 120,
    XS138_WAIT_READY_RETURN = 121,
    XS138_WAIT_READY_ERROR = 122,
    XS138_INPUT_COPY_ENTER = 130,
    XS138_INPUT_COPY_RETURN = 131,
    XS138_INPUT_COPY_ERROR = 132,
    XS138_REPLAY_ENTER = 140,
    XS138_REPLAY_RETURN = 141,
    XS138_REPLAY_ERROR = 142,
    XS138_OUTPUT_CLONE_ENTER = 150,
    XS138_OUTPUT_CLONE_RETURN = 151,
    XS138_OUTPUT_CLONE_ERROR = 152,
    XS138_EPOCH_RELEASE_ENTER = 160,
    XS138_EPOCH_RELEASE_RETURN = 161,
    XS138_EPOCH_RELEASE_ERROR = 162,
    XS138_FOLLOWING_BOUNDARY_ENTER = 170,
    XS138_FOLLOWING_BOUNDARY_RETURN = 171,
    XS138_FOLLOWING_BOUNDARY_ERROR = 172,
    XS138_SELECT_INTENT_ENTER = 180,
    XS138_SELECT_INTENT_RETURN = 181,
    XS138_SELECT_INTENT_ERROR = 182,
    XS138_RESTORE_INTENT_ENTER = 190,
    XS138_RESTORE_INTENT_RETURN = 191,
    XS138_RESTORE_INTENT_ERROR = 192,
    XS138_CACHE_RETIRE_ENTER = 200,
    XS138_CACHE_RETIRE_RETURN = 201,
    XS138_CACHE_RETIRE_ERROR = 202,
    XS138_CACHE_RETIRE_DECLINED = 203
};

#ifdef __cplusplus
#define XS138_TRACE_NOEXCEPT noexcept
extern "C" {
#else
#define XS138_TRACE_NOEXCEPT
#endif

/* Returns a common committed ring ordinal, or0 when inactive/full/unavailable.
 * Each row contains monotonic_ns sampled at commit and a native thread-id hash.
 * graph_identity is a diagnostic/Python control identity, NOT a CUDA graph handle.
 * plan_handle is an ABI handle, NOT a native Plan ID. Neither retains an owner.
 * Identity0 is unavailable; unknown device is -1. Marker pool{0,0} is unavailable.
 * The wrapper records the unavailability reason; it must never guess an identity. */
uint64_t allocation_trace_marker(
    uint32_t marker, uint32_t invocation, int32_t device,
    uint64_t stream, uint64_t graph_identity, uint64_t plan_handle,
    uint64_t pool_a, uint64_t pool_b) XS138_TRACE_NOEXCEPT;

/* Native window and ATen selection are independent. Keep window=1 across the
 * inter-invocation gap with stage=0 (ATen disabled). Close outside exclusions. */
int allocation_trace_window(int enabled) XS138_TRACE_NOEXCEPT;
void allocation_trace_stage(int stage) XS138_TRACE_NOEXCEPT;
int allocation_trace_install(void) XS138_TRACE_NOEXCEPT;
const char* allocation_trace_error(void) XS138_TRACE_NOEXCEPT;
/* Actual first-call-latched Torch state:0/1, or-1 with explicit error on failure.
 * Host-only; never initializes CUDA, installs an allocator, or changes environment. */
int allocation_trace_workspace_cache_enabled(void) XS138_TRACE_NOEXCEPT;
int allocation_trace_checkpoint(int marker, int device, uint64_t pool_a,
                                uint64_t pool_b) XS138_TRACE_NOEXCEPT;
/* Call only outside capture/replay/native exclusions. Serialization is here only. */
const char* allocation_trace_dump(void) XS138_TRACE_NOEXCEPT;

#ifdef __cplusplus
}
#endif
#undef XS138_TRACE_NOEXCEPT
#endif
