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
 * Default-stream sentinels 0/1/2 are refused; pass a live explicit stream.
 * Handles are process-local; fork is unsupported.
 * Calls on live handles are thread-safe. Each handle has one owning caller: serialize
 * its close/ownership-transfer calls; close must not race use of a raw device pointer.
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
/* Finalizers transfer, never silently discard a refused handle. Success invalidates
 * the caller's handle. Native retirement retries Capture/Busy/submitted fences off
 * the allocator stack. Live Cache pages and permanent faults are quarantined, not
 * freed or retried forever. Checked closes retain the same handle on every refusal. */
VR_API vr_status vr_v1_defer_close(vr_handle);
typedef struct {
    uint64_t queued, retired, quarantined, abandoned_cache_bytes;
} vr_retirement_stats_v1;
VR_API vr_status vr_v1_retirement_stats(vr_retirement_stats_v1 *out, size_t out_size);
/* Constructors zero their output first; failed constructors retain caller ownership. */
VR_API vr_status vr_v1_gpu_open(int32_t device, vr_handle *out);
VR_API vr_status vr_v1_gpu_close(vr_handle gpu); /* children retain GPU ownership */

/* Immutable weights. Host construction copies immutable bytes immediately; cold
 * host fills use bounded pinned staging and explicit stream ordering. Neither provider
 * implies host-visible GPU completion. Native Source ownership transfers only on
 * successful region construction. */
typedef void (*vr_drop_fn)(void *);
typedef struct vr_source_job_v1 vr_source_job_v1;
typedef vr_status (*vr_source_prepare_fn)(void *source, int32_t device,
    uint64_t offset, uint64_t bytes, vr_source_job_v1 *out);
typedef vr_status (*vr_source_submit_fn)(void *job, uint64_t dst, uint64_t bytes);
typedef vr_status (*vr_source_retire_fn)(void *context);
/* Prepare gets no destination and submits NO GPU work. Failure transfers nothing:
 * Source retains every failed-prepare owner for checked final retirement. Success
 * returns a complete owned job and transfers its context. Producer handles are
 * actual nonzero primary-context CUstreams, never 0/1/2 sentinels. Their array/code/
 * source/staging owners remain retained; core copies the fixed array before begin.
 * Submit receives dst only after native admission and returns with every host
 * submitter closed. VR_OK certifies every byte initialized/enqueued; a normal error
 * aborts. VR_FAULT means closure is unproven and quarantines actual owners.
 * No callback may unwind, reenter the same Region, or run under an ABI/allocator lock.
 */
struct vr_source_job_v1 {
    const uint64_t *streams;
    size_t stream_count;
    vr_source_submit_fn submit;
    vr_source_retire_fn retire;
    vr_drop_fn drop;
    void *context;
};
typedef struct {
    vr_source_prepare_fn prepare;
    vr_source_retire_fn retire;
    vr_drop_fn drop;
    void *context;
} vr_source_provider_v1;
/* Both retire callbacks run on the native per-device queue under retirement exclusion.
 * VR_BUSY retains pending ownership, VR_OK leaves the wrapper inert (or transfers
 * equivalent custody), every other error quarantines without retry. They never wait
 * for Python/the GIL. drop is host-only/inert after checked retirement; it may not
 * run CUDA or arbitrary Python finalizers. Persistent Source outlives all its jobs.
 * Asynchronous failure/pending cleanup remains visible in retirement_stats.
 * The old by-value Source constructor is removed; there is no compatibility adapter.
 */
VR_API vr_status vr_v1_region_from_source(vr_handle gpu, uint64_t bytes,
    const vr_source_provider_v1 *, size_t source_size, vr_handle *out);
VR_API vr_status vr_v1_region_from_host(vr_handle, const uint8_t *, size_t, vr_handle *out);
VR_API vr_status vr_v1_region_ptr(vr_handle, uint64_t *out); /* VA only, never a pin */
VR_API vr_status vr_v1_region_close(vr_handle);
/* Success orders consumer stream work after Published producers. It is not host GPU
 * completion; each Region publishes only whole contents, but the group is not atomic. */
VR_API vr_status vr_v1_use(vr_handle gpu, const vr_handle *regions, size_t n, uint64_t stream, vr_handle *out);
VR_API vr_status vr_v1_use_ptr(vr_handle use, size_t index, uint64_t *out);
/* Producer ordering covers only the stream supplied to use. Additional readers need
 * another Use on their stream, or a CUDA dependency behind the ordered stream BEFORE
 * reading. All additional consumers must join the retained stream before done.
 * Success consumes the Use. Fencing failure retains the core's conservative context
 * fence. No alias may access a Region after its last Use ends. */
VR_API vr_status vr_v1_done(vr_handle use);

/* Working memory: free consumes the allocation handle on success. Its allocation
 * stream must remain alive through free and any deferred retirement; side streams
 * require record_stream first. Use checked close/retry, or drain retirement before
 * destroying a borrowed stream.
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
 * same-GPU Regions across arenas and one active phase. Repeat/prefetch policy is not in ABI 1.
 * activate/deactivate expose region intent only: no fill, Pin, execution fingerprint
 * or successful H observation. Nested clients restore their prior intent explicitly.
 * enter uses an execution key including shape, dtype and kernel identity. Leave with
 * completed=1 only after checked successful completion; aborted passes never train it.
 * close aborts an open phase. This eager API does not manage CUDA graph lifetimes.
 */
typedef struct { const char *name; const vr_handle *regions; size_t count; } vr_phase_v1;
VR_API vr_status vr_v1_plan_new(vr_handle gpu, const vr_phase_v1 *, size_t n, vr_handle *out);
VR_API vr_status vr_v1_plan_activate(vr_handle plan, const char *name);
VR_API vr_status vr_v1_plan_deactivate(vr_handle plan);
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
