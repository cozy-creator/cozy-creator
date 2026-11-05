/* A real engine sizing its work under a cap: it reads `room`, sizes a job from it and lets the core
 * pick the level. A level cuts the job in 2^level pieces, each written by a kernel and checked, so the
 * answer is the same at every level. cells.py sets the cap, as the C API has no limit call. */
#include "varena.h"
#include <dlfcn.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

#define VR(call) do { int status_ = (call); if (status_) { snprintf(report, size, "%s: %s (%d)", #call, vr_v1_last_error(), status_); return 1; } } while (0)
#define CU(call) do { int status_ = (call); if (status_) { snprintf(report, size, "%s: CUDA %d", #call, status_); return 1; } } while (0)
#define LOAD(name, args) int (*name) args; do { void *symbol = dlsym(cuda, #name); if (!symbol) { snprintf(report, size, "%s", dlerror()); return 1; } memcpy(&name, &symbol, sizeof(name)); } while (0)
#define LEVELS 6
#define WEIGHTS (4u << 20) /* floats, a power of two */
#define BLOCK 1024u
#define CHECK (1u << 16)   /* floats read back at each end of a piece */

static const char ptx[] =
    ".version 7.0\n.target sm_70\n.address_size 64\n"
    ".visible .entry twice(.param .u64 src, .param .u64 dst, .param .u64 first, .param .u32 mask) {\n"
    ".reg .u64 %a, %b, %f, %n, %o; .reg .u32 %c, %t, %m, %i; .reg .f32 %x;\n"
    "ld.param.u64 %a, [src]; ld.param.u64 %b, [dst]; ld.param.u64 %f, [first]; ld.param.u32 %m, [mask];\n"
    "mov.u32 %c, %ctaid.x; mov.u32 %t, %tid.x;\n"
    "mul.wide.u32 %n, %c, 1024; cvt.u64.u32 %o, %t; add.u64 %n, %n, %o;\n"
    "shl.b64 %o, %n, 2; add.u64 %b, %b, %o;\n"
    "add.u64 %n, %n, %f; cvt.u32.u64 %i, %n; and.b32 %i, %i, %m;\n"
    "mul.wide.u32 %o, %i, 4; add.u64 %a, %a, %o;\n"
    "ld.global.f32 %x, [%a]; add.f32 %x, %x, %x; st.global.f32 [%b], %x; ret; }\n";

static float value(uint64_t index, uint64_t seed) { return (float)((index * 2654435761u + seed) % 8191u); }

/* cells.py checks the rows: this only does the work and reports what the core said. */
int sized_engine(uint64_t seed, char *report, size_t size) {
    void *cuda = dlopen("libcuda.so.1", RTLD_NOW | RTLD_LOCAL);
    if (!cuda) { snprintf(report, size, "%s", dlerror()); return 1; }
    LOAD(cuInit, (unsigned));
    LOAD(cuDevicePrimaryCtxRetain, (void **, int));
    LOAD(cuCtxPushCurrent_v2, (void *));
    LOAD(cuCtxPopCurrent_v2, (void **));
    LOAD(cuModuleLoadData, (void **, const void *));
    LOAD(cuModuleGetFunction, (void **, void *, const char *));
    LOAD(cuLaunchKernel, (void *, unsigned, unsigned, unsigned, unsigned, unsigned, unsigned, unsigned, void *, void **, void **));
    LOAD(cuMemcpyDtoH_v2, (void *, uint64_t, size_t));
    LOAD(cuStreamCreate, (void **, unsigned));
    LOAD(cuStreamDestroy_v2, (void *));
    LOAD(cuEventCreate, (void **, unsigned));
    LOAD(cuEventRecord, (void *, void *));
    LOAD(cuEventSynchronize, (void *));
    LOAD(cuEventDestroy_v2, (void *));
    LOAD(cuModuleUnload, (void *));
    LOAD(cuDevicePrimaryCtxRelease_v2, (int));
    CU(cuInit(0));
    void *context, *module, *kernel, *stream_, *done;
    CU(cuDevicePrimaryCtxRetain(&context, 0));
    CU(cuCtxPushCurrent_v2(context));
    CU(cuStreamCreate(&stream_, 1)); /* CU_STREAM_NON_BLOCKING */
    CU(cuEventCreate(&done, 2)); /* CU_EVENT_DISABLE_TIMING */
    CU(cuModuleLoadData(&module, ptx));
    CU(cuModuleGetFunction(&kernel, module, "twice"));
    uint64_t stream = (uint64_t)(uintptr_t)stream_;

    float *weights = malloc(WEIGHTS * sizeof(float)), *check = malloc(CHECK * sizeof(float));
    if (!weights || !check) { snprintf(report, size, "host allocation"); return 1; }
    for (uint32_t i = 0; i < WEIGHTS; ++i) weights[i] = value(i, seed);
    vr_handle gpu, region, plan;
    VR(vr_v1_gpu_open(0, &gpu));
    VR(vr_v1_region_from_host(gpu, (const uint8_t *)weights, WEIGHTS * sizeof(float), &region));
    vr_phase_v1 phase = {"work", &region, 1};
    VR(vr_v1_plan_new(gpu, &phase, 1, &plan));
    vr_room_v1 room;
    VR(vr_v1_room(gpu, &room, sizeof(room)));
    /* The job takes the whole cap at level 0: too big beside the weights, as a real one would be. */
    uint64_t job = room.capacity & ~(((uint64_t)2 << 20 << LEVELS) - 1);
    size_t used = (size_t)snprintf(report, size, "{\"capacity\":%llu,\"free\":%llu,\"job\":%llu,\"rows\":[",
        (unsigned long long)room.capacity, (unsigned long long)room.free, (unsigned long long)job);

    /* honest: the estimate is what a level uses. over: twice that, as a cautious engine would say. */
    const struct { const char *key; unsigned scale, passes; } menus[] = {{"honest", 1, 2}, {"over", 2, 3}};
    const char *names[LEVELS] = {"1", "2", "4", "8", "16", "32"};
    vr_split_choice_v1 choices[LEVELS];
    int first = 1;
    for (unsigned menu = 0; menu < 2; ++menu) {
        for (int level = 0; level < LEVELS; ++level) {
            choices[level].id = names[level];
            choices[level].bytes = (job >> level) * menus[menu].scale;
        }
        for (unsigned pass = 0; pass < menus[menu].passes; ++pass) {
            vr_handle used_regions, piece;
            VR(vr_v1_phase_enter(plan, "work", menus[menu].key));
            VR(vr_v1_use(gpu, &region, 1, stream, &used_regions));
            uint64_t src;
            VR(vr_v1_use_ptr(used_regions, 0, &src));
            vr_split_selection_v1 level;
            VR(vr_v1_split_select(gpu, menus[menu].key, choices, LEVELS, stream, &level, sizeof(level)));
            uint64_t split;
            VR(vr_v1_split_begin(plan, menus[menu].key, choices[level.index].id, stream, level.generation, &split));
            uint64_t pieces = (uint64_t)1 << level.index, floats = (job >> level.index) / sizeof(float);
            uint32_t mask = WEIGHTS - 1;
            for (uint64_t index = 0; index < pieces; ++index) {
                uint64_t start = index * floats, dst;
                VR(vr_v1_alloc(gpu, floats * sizeof(float), stream, &piece));
                VR(vr_v1_allocation_ptr(piece, &dst));
                void *args[] = {&src, &dst, &start, &mask};
                CU(cuLaunchKernel(kernel, (unsigned)(floats / BLOCK), 1, 1, BLOCK, 1, 1, 0, stream_, args, NULL));
                CU(cuEventRecord(done, stream_));
                CU(cuEventSynchronize(done));
                uint64_t tail = (floats / BLOCK * BLOCK) - CHECK;
                for (uint64_t at = 0; at <= tail; at += tail) {
                    CU(cuMemcpyDtoH_v2(check, dst + at * sizeof(float), CHECK * sizeof(float)));
                    for (uint32_t i = 0; i < CHECK; ++i) {
                        uint32_t weight = (uint32_t)(start + at + i) & mask;
                        if (check[i] != 2 * weights[weight]) {
                            snprintf(report, size, "wrong value at piece %llu float %llu", (unsigned long long)index, (unsigned long long)(at + i));
                            return 1;
                        }
                    }
                }
                VR(vr_v1_free(piece));
            }
            vr_split_end_v1 end;
            VR(vr_v1_split_end(plan, split, 1, &end, sizeof(end)));
            VR(vr_v1_done(used_regions));
            VR(vr_v1_phase_leave(plan, 1));
            used += (size_t)snprintf(report + used, size - used,
                "%s{\"key\":\"%s\",\"pass\":%u,\"level\":%llu,\"pieces\":%llu,\"estimate\":%llu,\"bytes\":%llu,"
                "\"measured\":%llu,\"budget\":%llu,\"peak\":%llu,\"counted\":%llu}",
                first ? "" : ",", menus[menu].key, pass, (unsigned long long)level.index, (unsigned long long)pieces,
                (unsigned long long)choices[level.index].bytes, (unsigned long long)level.bytes,
                (unsigned long long)level.measured, (unsigned long long)level.budget,
                (unsigned long long)end.peak, (unsigned long long)end.counted);
            first = 0;
        }
    }
    vr_stats_v1 stats;
    VR(vr_v1_stats(gpu, &stats, sizeof(stats)));
    snprintf(report + used, size - used, "],\"mapped\":%llu,\"resident\":%llu,\"peak\":%llu,\"driver_ooms\":%llu}",
        (unsigned long long)stats.working_mapped, (unsigned long long)stats.weights_resident,
        (unsigned long long)stats.working_peak, (unsigned long long)stats.driver_ooms);
    VR(vr_v1_plan_close(plan));
    VR(vr_v1_region_close(region));
    VR(vr_v1_gpu_close(gpu));
    CU(cuEventRecord(done, stream_));
    CU(cuEventSynchronize(done));
    CU(cuEventDestroy_v2(done));
    CU(cuStreamDestroy_v2(stream_));
    CU(cuModuleUnload(module));
    CU(cuCtxPopCurrent_v2(&context));
    CU(cuDevicePrimaryCtxRelease_v2(0));
    free(weights);
    free(check);
    return 0;
}
