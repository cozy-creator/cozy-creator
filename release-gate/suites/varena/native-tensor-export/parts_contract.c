/* Actual public C ABI client. Host mode loads no CUDA; GPU mode is coordinator-only.
   No framework, CUDA SDK, mock, or private host-parts implementation is used. */
#define _POSIX_C_SOURCE 200809L
#include "varena.h"
#include <errno.h>
#include <inttypes.h>
#include <limits.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

static const uint64_t handle_sentinel = UINT64_C(0x7319572354197319);
static unsigned refusals;

static int check(vr_status status, const char *operation) {
    if (status == VR_OK) return 1;
    fprintf(stderr, "%s: status=%d detail=%s\n", operation, status, vr_v1_last_error());
    return 0;
}

#define REQUIRE(condition) do { \
    if (!(condition)) { fprintf(stderr, "requirement failed: %s\n", #condition); return 1; } \
} while (0)
#define OK(operation) REQUIRE(check((operation), #operation))

static int rejected(const char *name, vr_handle gpu, const vr_host_part_v1 *parts,
                    size_t count, int null_offsets, int null_handle) {
    uint64_t offsets[] = {123, 456, 789, 1011};
    const uint64_t expected[] = {123, 456, 789, 1011};
    vr_handle handle = handle_sentinel;
    vr_status status = vr_v1_region_from_host_parts(
        gpu, parts, count, null_offsets ? NULL : offsets, null_handle ? NULL : &handle);
    if (status != VR_INVALID || handle != handle_sentinel ||
        memcmp(offsets, expected, sizeof(offsets)) != 0) {
        fprintf(stderr, "%s: status=%d handle=%" PRIu64 " detail=%s\n",
                name, status, handle, vr_v1_last_error());
        return 0;
    }
    ++refusals;
    return 1;
}

static int no_cuda_loaded(void) {
    FILE *maps = fopen("/proc/self/maps", "r");
    if (!maps) return 0;
    char line[4096];
    int clean = 1;
    while (fgets(line, sizeof(line), maps)) {
        if (strstr(line, "libcuda.so") || strstr(line, "nvcuda.dll")) clean = 0;
    }
    if (fclose(maps) != 0) clean = 0;
    return clean;
}

static int host(void) {
    const uint8_t value = 17;
    const vr_host_part_v1 part = {&value, 1};
    vr_retirement_stats_v1 before = {0}, after = {0};
    REQUIRE(no_cuda_loaded());
    OK(vr_v1_retirement_stats(&before, sizeof(before)));
    REQUIRE(rejected("null handle output", 0, &part, 1, 0, 1));
    REQUIRE(rejected("null offsets output", 0, &part, 1, 1, 0));
    REQUIRE(rejected("both outputs null", 0, &part, 1, 1, 1));
    REQUIRE(rejected("null nonempty descriptor array", 0, NULL, 1, 0, 0));
    /* Array count overflow is checked before constructing/dereferencing a slice.
       There are deliberately no readable descriptors at this address. */
    REQUIRE(rejected("descriptor count overflow", 0,
                     (const vr_host_part_v1 *)(uintptr_t)1, SIZE_MAX, 0, 0));
    REQUIRE(rejected("invalid zero GPU handle", 0, &part, 1, 0, 0));
    REQUIRE(rejected("invalid unknown GPU handle", UINT64_MAX, &part, 1, 0, 0));
    REQUIRE(rejected("invalid GPU with no descriptors", 0, NULL, 0, 0, 0));
    OK(vr_v1_retirement_stats(&after, sizeof(after)));
    REQUIRE(after.queued == before.queued && after.retired == before.retired);
    REQUIRE(after.quarantined == before.quarantined);
    REQUIRE(after.abandoned_cache_bytes == before.abandoned_cache_bytes);
    REQUIRE(no_cuda_loaded());
    printf("{\"passed\":true,\"mode\":\"host\",\"actual_public_abi\":true,"
           "\"refusals\":%u,\"outputs_unchanged\":true,\"cuda_loaded\":false,"
           "\"native_retirement_unchanged\":true,\"gpu_qualified\":false}\n", refusals);
    return 0;
}

static int same_live_resources(const vr_stats_v1 *a, const vr_stats_v1 *b) {
    return a->weights_resident == b->weights_resident &&
        a->working_live == b->working_live && a->working_capacity == b->working_capacity &&
        a->working_mapped == b->working_mapped && a->working_pending == b->working_pending &&
        a->working_free == b->working_free && a->pools == b->pools && a->caches == b->caches &&
        a->cache_live == b->cache_live && a->cache_pending == b->cache_pending &&
        a->cache_ready == b->cache_ready;
}

static int gpu(int32_t device) {
    vr_retirement_stats_v1 retirement_before = {0}, retirement_after = {0};
    vr_stats_v1 before = {0}, after = {0};
    vr_handle gpu_handle = 0;
    OK(vr_v1_retirement_stats(&retirement_before, sizeof(retirement_before)));
    OK(vr_v1_gpu_open(device, &gpu_handle));
    OK(vr_v1_stats(gpu_handle, &before, sizeof(before)));
    const vr_host_part_v1 null_payload = {NULL, 1};
    const vr_host_part_v1 empty[] = {{NULL, 0}, {NULL, 0}};
    /* Unreadable payload pointers appear ONLY in layouts rejected before any
       snapshot allocation/copy: oversized member length and aggregate arithmetic. */
    const vr_host_part_v1 oversized = {(const uint8_t *)(uintptr_t)1, SIZE_MAX};
    const vr_host_part_v1 overflow[] = {
        {(const uint8_t *)(uintptr_t)1, (size_t)PTRDIFF_MAX},
        {(const uint8_t *)(uintptr_t)1, 1},
    };
    REQUIRE(rejected("null nonempty payload", gpu_handle, &null_payload, 1, 0, 0));
    REQUIRE(rejected("no parts", gpu_handle, NULL, 0, 0, 0));
    REQUIRE(rejected("all-empty aggregate", gpu_handle, empty, 2, 0, 0));
    REQUIRE(rejected("oversized member", gpu_handle, &oversized, 1, 0, 0));
    REQUIRE(rejected("aggregate arithmetic overflow", gpu_handle, overflow, 2, 0, 0));
    OK(vr_v1_stats(gpu_handle, &after, sizeof(after)));
    REQUIRE(same_live_resources(&before, &after));

    /* The actual GPU handle and constructor remain usable after every refusal.
       A trailing empty member receives its aligned logical-end offset. */
    const uint8_t first[] = {17, 19, 23};
    const uint8_t second[] = {39, 43};
    const vr_host_part_v1 valid[] = {{first, sizeof(first)}, {second, sizeof(second)}, {NULL, 0}};
    uint64_t offsets[] = {123, 456, 789};
    vr_handle region = handle_sentinel;
    OK(vr_v1_region_from_host_parts(gpu_handle, valid, 3, offsets, &region));
    REQUIRE(region != 0 && region != handle_sentinel);
    REQUIRE(offsets[0] == 0 && offsets[1] == 256 && offsets[2] == 512);
    OK(vr_v1_region_close(region));
    OK(vr_v1_stats(gpu_handle, &after, sizeof(after)));
    REQUIRE(same_live_resources(&before, &after));
    OK(vr_v1_gpu_close(gpu_handle));
    /* This constructor-only client proves the live resource baseline above.
       Close transfers native custody; its accepted request is not proof of final
       service teardown. The maintained typed/Rust clients check retirement. */
    OK(vr_v1_retirement_stats(&retirement_after, sizeof(retirement_after)));
    REQUIRE(retirement_after.quarantined == retirement_before.quarantined);
    REQUIRE(retirement_after.abandoned_cache_bytes == retirement_before.abandoned_cache_bytes);
    printf("{\"passed\":true,\"mode\":\"gpu\",\"device\":%" PRId32 ","
           "\"actual_public_abi\":true,\"refusals\":%u,\"outputs_unchanged\":true,"
           "\"valid_constructor_after_refusal\":true,\"native_resource_baseline\":true,"
           "\"close_accepted_not_completion\":true,\"scope\":\"constructor metadata and live resource baseline; no readback or final service teardown claim\"}\n",
           device, refusals);
    return 0;
}

int main(int argc, char **argv) {
    REQUIRE(vr_v1_abi_version() == VR_ABI_VERSION && vr_v1_core_identity() != 0);
    if (argc == 2 && strcmp(argv[1], "host") == 0) return host();
    if (argc == 3 && strcmp(argv[1], "gpu") == 0) {
        char *end = NULL;
        errno = 0;
        long device = strtol(argv[2], &end, 10);
        if (!errno && end != argv[2] && *end == '\0' && device >= 0 && device <= INT32_MAX)
            return gpu((int32_t)device);
    }
    fprintf(stderr, "usage: parts-contract host | gpu DEVICE\n");
    return 2;
}
