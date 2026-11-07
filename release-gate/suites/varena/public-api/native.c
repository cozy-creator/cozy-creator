#include "varena.h"
#include <stdint.h>

_Static_assert(sizeof(vr_stats_v1) == 16 * sizeof(uint64_t), "stats ABI layout");
_Static_assert(sizeof(vr_cache_info_v1) == 7 * sizeof(uint64_t), "cache ABI layout");
_Static_assert(sizeof(vr_room_v1) == 5 * sizeof(uint64_t), "room ABI layout");
_Static_assert(sizeof(vr_split_choice_v1) == 2 * sizeof(uint64_t), "split choice ABI layout");
_Static_assert(sizeof(vr_split_selection_v1) == 5 * sizeof(uint64_t), "split selection ABI layout");
_Static_assert(sizeof(vr_split_end_v1) == 2 * sizeof(uint64_t), "split end ABI layout");

uint64_t mixed_c_identity(void) { return vr_v1_core_identity(); }
int mixed_c_error(void) {
    vr_handle handle = UINT64_MAX;
    int status = vr_v1_gpu_open(-1, &handle);
    return status == VR_INVALID && handle == 0;
}
int mixed_c_alloc(vr_handle gpu, uint64_t bytes, uint64_t stream, vr_handle *out) {
    return vr_v1_alloc(gpu, bytes, stream, out);
}
int mixed_c_free(vr_handle allocation) { return vr_v1_free(allocation); }
int mixed_c_room(vr_handle gpu, vr_room_v1 *out) { return vr_v1_room(gpu, out, sizeof(*out)); }

static int drops;
static void source_drop(void *context) { (void)context; ++drops; }
static int source_prepare(void *context, int32_t device, uint64_t offset, uint64_t bytes, vr_source_job_v1 *out) {
    (void)context; (void)device; (void)offset; (void)bytes; (void)out;
    return VR_SOURCE_ERROR;
}
static int source_retire(void *context) { (void)context; return VR_OK; }
int mixed_c_contract(void) {
    /* Intent-only entry points validate actual handles without touching CUDA. */
    if (vr_v1_plan_activate(0, "absent") != VR_INVALID) return 0;
    if (vr_v1_plan_deactivate(0) != VR_INVALID) return 0;
    vr_handle output = UINT64_MAX;
    vr_source_provider_v1 source = {source_prepare, source_retire, source_drop, NULL};
    int before = drops;
    /* A real refused constructor, no fake allocator/driver. Caller still owns Source. */
    if (vr_v1_region_from_source(0, 64, &source, sizeof(source), &output) != VR_INVALID || output || drops != before) return 0;
    source.drop(source.context);
    if (drops != before + 1) return 0;
    output = UINT64_MAX;
    if (vr_v1_alloc(0, 64, 0, &output) != VR_INVALID || output) return 0;
    vr_room_v1 room;
    vr_split_choice_v1 choice = {"whole", 64};
    vr_split_selection_v1 selection;
    vr_split_end_v1 end;
    if (vr_v1_room(0, &room, sizeof(room)) != VR_INVALID) return 0;
    if (vr_v1_room(0, &room, sizeof(room) - 1) != VR_INVALID) return 0;
    if (vr_v1_split_select(0, "key", &choice, 1, 3, &selection, sizeof(selection)) != VR_INVALID) return 0;
    if (vr_v1_split_begin(0, "key", "whole", 3, 0, &output) != VR_INVALID || output) return 0;
    if (vr_v1_split_end(0, 1, 1, &end, sizeof(end)) != VR_INVALID) return 0;
    if (vr_v1_region_set_priority(0, VR_PRIORITY_HIGH) != VR_INVALID) return 0;
    struct { vr_stats_v1 stats; uint64_t sentinel; } result = { {0}, UINT64_MAX };
    if (vr_v1_stats(0, &result.stats, sizeof(result.stats) - 1) != VR_INVALID) return 0;
    return result.sentinel == UINT64_MAX;
}
