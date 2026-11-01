#ifndef VARENA_V1_H
#define VARENA_V1_H
#include <stddef.h>
#include <stdint.h>
#if defined(_WIN32)
#define VR_API __declspec(dllimport)
#else
#define VR_API
#endif
#ifdef __cplusplus
extern "C" {
#endif

/* ABI 1: fixed-width statuses and non-reused opaque handles. No CUDA SDK needed.
 * Streams/pointers are CUDA primary-context handles represented as uint64_t.
 * Zero is the legacy default stream. Handles are process-local; fork is unsupported.
 * Functions are thread-safe; close must not race use of a raw device pointer.
 * Caller buffers/UTF-8 strings must be valid for the call. Callbacks must not unwind.
 * ABI 1 layouts never grow: a different layout/symbol requires a new ABI version.
 */
#define VR_ABI_VERSION 1u
typedef uint64_t vr_handle;
typedef int32_t vr_status;
#define VR_OK 0
#define VR_EXHAUSTED 1
#define VR_CAPTURING 2
#define VR_SOURCE_ERROR 3
#define VR_FAULT 4
#define VR_INVALID 5
#define VR_UNSUPPORTED 6
#define VR_BUSY 7
VR_API uint32_t vr_v1_abi_version(void);
VR_API uint64_t vr_v1_core_identity(void);
/* Last failure on this thread, unchanged by success or these getters. Copy before
 * the next failing call. Requested bytes are meaningful for VR_EXHAUSTED only. */
VR_API const char *vr_v1_last_error(void);
VR_API uint64_t vr_v1_error_requested_bytes(void);
/* Constructors zero their output first; failed constructors retain caller ownership. */
VR_API vr_status vr_v1_gpu_open(int32_t device, vr_handle *out);
VR_API vr_status vr_v1_gpu_close(vr_handle gpu); /* children retain GPU ownership */

/* Immutable weights. Host constructor copies bytes, retained for future refills.
 * Source ownership transfers only on success. Source callbacks may run on a caller
 * thread, without allocator locks. They must be thread-safe and must not reenter the
 * same Region's use. fill initializes all bytes and retains staging until completion,
 * including failure. ABI 1 conservatively joins the context before publishing a fill.
 * drop runs after all Regions, Uses and Plans referencing the Source have retired.
 */
typedef vr_status (*vr_fill_fn)(void *, uint64_t offset, uint64_t bytes, uint64_t dst, uint64_t stream);
typedef void (*vr_drop_fn)(void *);
typedef struct { vr_fill_fn fill; vr_drop_fn drop; void *context; } vr_source_v1;
VR_API vr_status vr_v1_region_new(vr_handle, uint64_t bytes, vr_source_v1, vr_handle *out);
VR_API vr_status vr_v1_region_from_host(vr_handle, const uint8_t *, size_t, vr_handle *out);
VR_API vr_status vr_v1_region_ptr(vr_handle, uint64_t *out); /* VA only, never a pin */
VR_API vr_status vr_v1_region_close(vr_handle);
VR_API vr_status vr_v1_use(vr_handle gpu, const vr_handle *regions, size_t n, uint64_t stream, vr_handle *out);
VR_API vr_status vr_v1_use_ptr(vr_handle use, size_t index, uint64_t *out);
/* All additional consumer streams must join the stream retained by use before done.
 * Success consumes the Use. Fencing failure retains the core's conservative context
 * fence. No alias may access a Region after its last Use ends. */
VR_API vr_status vr_v1_done(vr_handle use);

/* Working memory: free consumes the allocation handle on success. Its allocation
 * stream must remain alive through free; side streams require record_stream first.
 * Free is stream ordered, not proof of physical reclamation. */
VR_API vr_status vr_v1_alloc(vr_handle gpu, uint64_t bytes, uint64_t stream, vr_handle *out);
VR_API vr_status vr_v1_allocation_ptr(vr_handle, uint64_t *out);
VR_API vr_status vr_v1_record_stream(vr_handle allocation, uint64_t stream);
VR_API vr_status vr_v1_free(vr_handle allocation);

/* Rebuildable Cache. Shrink runs on the native callback worker without allocator
 * locks. VR_BUSY declines; VR_OK ends the attempt, it does not promise freed bytes.
 * Only owner-selected ranges released behind actual completion events count.
 * Callback gets this Cache's handle and may call release. It must not close itself.
 * Callback/context ownership transfers on success. close drains callbacks outside
 * locks, then refuses live/pending pages (handle remains retryable on failure).
 * Python must release the GIL while closing. NULL shrink/drop installs no callback.
 */
typedef vr_status (*vr_shrink_fn)(void *, vr_handle cache, uint64_t bytes);
typedef struct { vr_shrink_fn shrink; vr_drop_fn drop; void *context; } vr_shrinker_v1;
typedef struct {
    uint64_t base, granularity, reserved, mapped, live, pending, ready;
} vr_cache_info_v1;
VR_API vr_status vr_v1_cache_new(vr_handle gpu, uint64_t reserve, vr_shrinker_v1, vr_handle *out);
VR_API vr_status vr_v1_cache_info(vr_handle, vr_cache_info_v1 *out, size_t out_size);
VR_API vr_status vr_v1_cache_commit(vr_handle, uint64_t offset, uint64_t bytes);
VR_API vr_status vr_v1_cache_release(vr_handle, uint64_t offset, uint64_t bytes, uint64_t stream);
VR_API vr_status vr_v1_cache_close(vr_handle);

/* Plan copies descriptors and retains Regions AND Sources. Current core supports
 * one arena on a GPU and one active phase. Repeat/prefetch policy is not in ABI 1.
 * enter uses an execution key including shape, dtype and kernel identity. Leave with
 * completed=1 only after successful submitted work; aborted passes never train it.
 * close aborts an open phase. This eager API does not manage CUDA graph lifetimes.
 */
typedef struct { const char *name; const vr_handle *regions; size_t count; } vr_phase_v1;
VR_API vr_status vr_v1_plan_new(vr_handle gpu, const vr_phase_v1 *, size_t n, vr_handle *out);
VR_API vr_status vr_v1_phase_enter(vr_handle plan, const char *name, const char *execution_key);
VR_API vr_status vr_v1_phase_leave(vr_handle plan, int32_t completed);
VR_API vr_status vr_v1_plan_close(vr_handle);

/* Snapshots are diagnostic, not allocation promises. working_mapped includes pools
 * but excludes caches; working_capacity includes rounding. Weight totals include
 * other TensorFS arenas in this shared core. */
typedef struct {
    uint64_t weights_resident, working_live, working_capacity;
    uint64_t working_mapped, working_pending, working_free, working_peak;
    uint64_t pools, caches, cache_live, cache_pending, cache_ready;
    uint64_t allocations, frees, driver_calls, driver_ooms;
} vr_stats_v1;
VR_API vr_status vr_v1_stats(vr_handle gpu, vr_stats_v1 *out, size_t out_size);
VR_API vr_status vr_v1_trim(vr_handle gpu, uint64_t *released);
#ifdef __cplusplus
}
#endif
#endif
