#include "varena.h"
#include <stdint.h>

_Static_assert(sizeof(vr_stats_v1) == 16 * sizeof(uint64_t), "stats ABI layout");
_Static_assert(sizeof(vr_cache_info_v1) == 7 * sizeof(uint64_t), "cache ABI layout");

uint64_t mixed_c_identity(void) { return vr_v1_core_identity(); }
int mixed_c_error(void) {
    vr_handle handle = UINT64_MAX;
    int status = vr_v1_gpu_open(-1, &handle);
    return status == VR_INVALID && handle == 0;
}
int mixed_c_alloc(vr_handle gpu, uint64_t bytes, vr_handle *out) {
    return vr_v1_alloc(gpu, bytes, 0, out);
}
int mixed_c_free(vr_handle allocation) { return vr_v1_free(allocation); }

static int drops;
static void source_drop(void *context) { (void)context; ++drops; }
static int source_fill(void *context, uint64_t offset, uint64_t bytes, uint64_t dst, uint64_t stream) {
    (void)context; (void)offset; (void)bytes; (void)dst; (void)stream;
    return VR_SOURCE_ERROR;
}
int mixed_c_contract(void) {
    vr_handle output = UINT64_MAX;
    vr_source_v1 source = {source_fill, source_drop, NULL};
    int before = drops;
    /* A real refused constructor, no fake allocator/driver. Caller still owns Source. */
    if (vr_v1_region_new(0, 64, source, &output) != VR_INVALID || output || drops != before) return 0;
    source.drop(source.context);
    if (drops != before + 1) return 0;
    output = UINT64_MAX;
    if (vr_v1_alloc(0, 64, 0, &output) != VR_INVALID || output) return 0;
    struct { vr_stats_v1 stats; uint64_t sentinel; } result = { {0}, UINT64_MAX };
    if (vr_v1_stats(0, &result.stats, sizeof(result.stats) - 1) != VR_INVALID) return 0;
    return result.sentinel == UINT64_MAX;
}
