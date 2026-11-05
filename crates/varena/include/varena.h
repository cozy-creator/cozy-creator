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
 * Streams and pointers are the GPU's primary-context handles as uint64_t.
 * Default-stream sentinels 0/1/2 are refused; pass a live explicit stream.
 * Handles belong to one process; fork is unsupported.
 * Calls on live handles are thread-safe. Serialize a handle's close or transfer; a close
 * must not race use of a raw device pointer. Callbacks must not unwind.
 * Layouts never grow: a change needs new symbols.
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
/* For finalizers: hands the handle to the native retirer, and success invalidates it. The
 * retirer retries through capture and busy states. Live Cache pages and permanent faults are
 * quarantined, not freed. A checked close keeps its handle on every refusal. */
VR_API vr_status vr_v1_defer_close(vr_handle);
typedef struct {
    uint64_t queued, retired, quarantined, abandoned_cache_bytes;
} vr_retirement_stats_v1;
VR_API vr_status vr_v1_retirement_stats(vr_retirement_stats_v1 *out, size_t out_size);
/* Constructors zero their output first; failed constructors retain caller ownership. */
VR_API vr_status vr_v1_gpu_open(int32_t device, vr_handle *out);
VR_API vr_status vr_v1_gpu_close(vr_handle gpu); /* children retain GPU ownership */

/* Immutable weights. from_host copies the bytes at once. A Source transfers only when
 * construction succeeds, and may run again after eviction. */
typedef void (*vr_drop_fn)(void *);
typedef struct vr_source_job_v1 vr_source_job_v1;
typedef vr_status (*vr_source_prepare_fn)(void *source, int32_t device,
    uint64_t offset, uint64_t bytes, vr_source_job_v1 *out);
typedef vr_status (*vr_source_submit_fn)(void *job, uint64_t dst, uint64_t bytes);
typedef vr_status (*vr_source_retire_fn)(void *context);
/* Prepare gets no destination and submits no GPU work. A failure transfers nothing: the
 * Source keeps what it made for its own retirement. Success returns a complete job and transfers
 * its context. `streams` are the live CUstreams it writes on, never 0/1/2, and stay retained.
 * Submit gets dst once the core admits it, and returns with nothing left submitting. VR_OK means
 * every byte is written or enqueued; an error aborts; VR_FAULT means that's unknown, and its owners
 * are quarantined. No callback may reenter its own Region. None runs under an allocator lock.
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
/* Both retire callbacks run on the native retirer. VR_BUSY asks again later, VR_OK leaves the
 * wrapper inert, any other status quarantines it. They never wait for Python. drop, after
 * retire, is host-only: no CUDA and no Python finalizers. A Source outlives its jobs.
 */
VR_API vr_status vr_v1_region_from_source(vr_handle gpu, uint64_t bytes,
    const vr_source_provider_v1 *, size_t source_size, vr_handle *out);
VR_API vr_status vr_v1_region_from_host(vr_handle, const uint8_t *, size_t, vr_handle *out);
typedef struct { const uint8_t *data; size_t bytes; } vr_host_part_v1;
/* One Region from parts, in order, at 256-byte-aligned offsets with zero-filled gaps. Copied
 * before returning. A zero-length part may have NULL data, but the whole must hold bytes.
 * offsets has count entries and must not overlap out. On failure both outputs stay
 * unchanged, unlike the other constructors, and no partial Region exists. */
VR_API vr_status vr_v1_region_from_host_parts(vr_handle gpu,
    const vr_host_part_v1 *parts, size_t count, uint64_t *offsets, vr_handle *out);
/* The same layout filled from the caller's ranges without copying them. They stay readable and
 * unchanged until drop(context), which runs once from any thread when the last fill and handle let
 * go, and also if this call fails. */
VR_API vr_status vr_v1_region_borrow_host_parts(vr_handle gpu,
    const vr_host_part_v1 *parts, size_t count, uint64_t *offsets,
    void (*drop)(void *), void *context, vr_handle *out);
VR_API vr_status vr_v1_region_ptr(vr_handle, uint64_t *out); /* VA only, never a pin */
/* An idle HIGH Region goes only once owner Caches and peers were asked to give memory: for
 * weights worth more than rebuilding a Cache. NORMAL, the default, goes before them. */
#define VR_PRIORITY_NORMAL 0
#define VR_PRIORITY_HIGH 1
VR_API vr_status vr_v1_region_set_priority(vr_handle region, int32_t priority);
VR_API vr_status vr_v1_region_close(vr_handle);
/* Orders the stream's next work after each Region's fill. It is not GPU completion. */
VR_API vr_status vr_v1_use(vr_handle gpu, const vr_handle *regions, size_t n, uint64_t stream, vr_handle *out);
VR_API vr_status vr_v1_use_ptr(vr_handle use, size_t index, uint64_t *out);
/* Waits on the host for this Use's fills. Not for consumers; other work may still keep a
 * capture out. Refused during capture or replay. A failure keeps the Use. */
VR_API vr_status vr_v1_use_wait_ready(vr_handle use);
/* Only use's stream is ordered: another reader takes its own Use, or a CUDA dependency on that
 * stream before reading, and joins it before done. Success consumes the Use.
 * No alias may touch a Region after its last Use ends. */
VR_API vr_status vr_v1_done(vr_handle use);

/* Working memory. Free is stream-ordered and consumes the handle; the pages go later. Keep the
 * allocation's stream alive through it and any deferred retirement. Other streams need
 * record_stream first. */
VR_API vr_status vr_v1_alloc(vr_handle gpu, uint64_t bytes, uint64_t stream, vr_handle *out);
VR_API vr_status vr_v1_allocation_ptr(vr_handle, uint64_t *out);
VR_API vr_status vr_v1_record_stream(vr_handle allocation, uint64_t stream);
VR_API vr_status vr_v1_free(vr_handle allocation);

/* Rebuildable Cache. Shrink runs on a native worker with no allocator lock held, gets this
 * Cache's handle and may call release. VR_BUSY declines; VR_OK ends the attempt. Only ranges
 * released and completed count. It must not close its Cache. The callback transfers on success.
 * close drains callbacks, then refuses live or pending pages, keeping the handle. A Python
 * caller releases the GIL while closing. NULL shrink and drop install no callback.
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

/* Plan copies its descriptors and retains its Regions and their Sources. One GPU, one active
 * plan, one phase at a time. activate/deactivate state intent only: no fill, pin or measure.
 * enter's key covers shape, dtype and kernel. Leave with completed=1 only after the pass has
 * checked its own completion: that publishes its measure, and an aborted pass doesn't.
 * close aborts an open phase. No graph lifetimes here.
 */
typedef struct { const char *name; const vr_handle *regions; size_t count; } vr_phase_v1;
VR_API vr_status vr_v1_plan_new(vr_handle gpu, const vr_phase_v1 *, size_t n, vr_handle *out);
VR_API vr_status vr_v1_plan_activate(vr_handle plan, const char *name);
VR_API vr_status vr_v1_plan_deactivate(vr_handle plan);
VR_API vr_status vr_v1_phase_enter(vr_handle plan, const char *name, const char *execution_key);
VR_API vr_status vr_v1_phase_leave(vr_handle plan, int32_t completed);
VR_API vr_status vr_v1_plan_close(vr_handle);

/* Snapshots, not promises. working_mapped includes pools and excludes caches;
 * working_capacity includes rounding. Totals cover the whole process, TensorFS included. */
typedef struct {
    uint64_t weights_resident, working_live, working_capacity;
    uint64_t working_mapped, working_pending, working_free, working_peak;
    uint64_t pools, caches, cache_live, cache_pending, cache_ready;
    uint64_t allocations, frees, driver_calls, driver_ooms;
} vr_stats_v1;
VR_API vr_status vr_v1_stats(vr_handle gpu, vr_stats_v1 *out, size_t out_size);

/* What this GPU has for the caller right now. A report, not a reservation: nothing is held, and
 * anyone may take it next. capacity is the card or its software cap, less what this process doesn't
 * own or control. free is what can be mapped with no reclaim, and capacity - working_live - pinned
 * is the most reclaim could reach. */
typedef struct {
    uint64_t capacity, working_live, pinned, weights_resident, free;
} vr_room_v1;
VR_API vr_status vr_v1_room(vr_handle gpu, vr_room_v1 *out, size_t out_size);

/* One operation's legal levels, largest first, each an id and its estimated full working bytes.
 * key names the operation's geometry, and its menu may not change. select takes what room it can
 * and picks the largest level under the core's budget. bytes is what that level was chosen at: its
 * measure (measured=1), or the estimate scaled as its nearest measured sibling came out. Nothing is
 * reserved. The same stream and generation go to begin. */
typedef struct { const char *id; uint64_t bytes; } vr_split_choice_v1;
typedef struct {
    uint64_t index, bytes, measured, budget, generation;
} vr_split_selection_v1;
VR_API vr_status vr_v1_split_select(vr_handle gpu, const char *key,
    const vr_split_choice_v1 *choices, size_t n, uint64_t stream,
    vr_split_selection_v1 *out, size_t out_size);
/* Inside a phase of plan, measures the level's working allocations until end. All of them go on
 * this stream. One open split per GPU; leaving the phase closes one left open. submitted=0 keeps
 * no measure. counted=1 means leaving the phase with completed=1 publishes this peak. */
typedef struct { uint64_t peak, counted; } vr_split_end_v1;
VR_API vr_status vr_v1_split_begin(vr_handle plan, const char *key, const char *choice,
    uint64_t stream, uint64_t generation, uint64_t *out);
VR_API vr_status vr_v1_split_end(vr_handle plan, uint64_t split, int32_t submitted,
    vr_split_end_v1 *out, size_t out_size);
#ifdef __cplusplus
}
#endif
#endif
