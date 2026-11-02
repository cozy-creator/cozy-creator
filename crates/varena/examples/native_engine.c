/* cc -std=c11 native_engine.c -I../include -L$VARENA_LIB_DIR -Wl,-rpath-link,$VARENA_LIB_DIR -lvarena -ldl -o native_engine
 * Run with libvarena.so on LD_LIBRARY_PATH. Requires a CUDA GPU, no Python/CUDA SDK.
 */
#include "varena.h"
#include <dlfcn.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

#define VR(call) do { int status = (call); if (status) { fprintf(stderr, "%s: %s (%d)\n", #call, vr_v1_last_error(), status); exit(1); } } while (0)
#define CU(call) do { int status = (call); if (status) { fprintf(stderr, "%s: CUDA %d\n", #call, status); exit(1); } } while (0)
#define LOAD(name, args) int (*name) args; do { void *symbol = dlsym(cuda, #name); if (!symbol) { fputs(dlerror(), stderr); return 1; } memcpy(&name, &symbol, sizeof(name)); } while (0)

static const char ptx[] =
    ".version 7.0\n.target sm_70\n.address_size 64\n"
    ".visible .entry twice(.param .u64 src, .param .u64 dst) {\n"
    ".reg .u64 %a, %b, %o; .reg .u32 %i; .reg .f32 %x;\n"
    "ld.param.u64 %a, [src]; ld.param.u64 %b, [dst];\n"
    "mov.u32 %i, %tid.x; mul.wide.u32 %o, %i, 4;\n"
    "add.u64 %a, %a, %o; add.u64 %b, %b, %o;\n"
    "ld.global.f32 %x, [%a]; add.f32 %x, %x, %x; st.global.f32 [%b], %x; ret; }\n";

int main(int argc, char **argv) {
    if (argc == 2 && strcmp(argv[1], "--host") == 0) {
        vr_handle sentinel = 123;
        int status = vr_v1_gpu_open(-1, &sentinel);
        if (vr_v1_abi_version() != VR_ABI_VERSION || status != VR_INVALID || sentinel != 0) return 1;
        printf("{\"abi\":1,\"core\":%llu,\"invalid_constructor_zeroed\":true}\n", (unsigned long long)vr_v1_core_identity());
        return 0;
    }
    void *cuda = dlopen("libcuda.so.1", RTLD_NOW | RTLD_LOCAL);
    if (!cuda) { fputs(dlerror(), stderr); return 1; }
    LOAD(cuInit, (unsigned));
    LOAD(cuDevicePrimaryCtxRetain, (void **, int));
    LOAD(cuCtxPushCurrent_v2, (void *));
    LOAD(cuCtxPopCurrent_v2, (void **));
    LOAD(cuModuleLoadData, (void **, const void *));
    LOAD(cuModuleGetFunction, (void **, void *, const char *));
    LOAD(cuLaunchKernel, (void *, unsigned, unsigned, unsigned, unsigned, unsigned, unsigned, unsigned, void *, void **, void **));
    LOAD(cuMemcpyDtoH_v2, (void *, uint64_t, size_t));
    LOAD(cuCtxSynchronize, (void));
    LOAD(cuModuleUnload, (void *));
    LOAD(cuDevicePrimaryCtxRelease_v2, (int));
    CU(cuInit(0));
    void *context, *module, *kernel;
    CU(cuDevicePrimaryCtxRetain(&context, 0));
    CU(cuCtxPushCurrent_v2(context));
    CU(cuModuleLoadData(&module, ptx));
    CU(cuModuleGetFunction(&kernel, module, "twice"));
    float weights[] = {1, 2, 3, 4}, output[4] = {0};
    vr_handle gpu, region, plan, used, scratch;
    VR(vr_v1_gpu_open(0, &gpu));
    VR(vr_v1_region_from_host(gpu, (const uint8_t *)weights, sizeof(weights), &region));
    vr_phase_v1 phase = {"decode", &region, 1};
    VR(vr_v1_plan_new(gpu, &phase, 1, &plan));
    for (int pass = 0; pass < 2; ++pass) {
        VR(vr_v1_phase_enter(plan, "decode", "f32/4/twice/eager"));
        VR(vr_v1_use(gpu, &region, 1, 0, &used));
        VR(vr_v1_alloc(gpu, sizeof(output), 0, &scratch));
        uint64_t src, dst;
        VR(vr_v1_use_ptr(used, 0, &src));
        VR(vr_v1_allocation_ptr(scratch, &dst));
        void *args[] = {&src, &dst};
        CU(cuLaunchKernel(kernel, 1, 1, 1, 4, 1, 1, 0, NULL, args, NULL));
        CU(cuCtxSynchronize());
        CU(cuMemcpyDtoH_v2(output, dst, sizeof(output)));
        for (int i = 0; i < 4; ++i) if (output[i] != weights[i] * 2) return 2;
        VR(vr_v1_free(scratch));
        VR(vr_v1_done(used));
        VR(vr_v1_phase_leave(plan, 1));
        uint64_t released;
        VR(vr_v1_trim(gpu, &released)); /* second pass refills the same Region VA */
    }
    VR(vr_v1_plan_close(plan));
    VR(vr_v1_region_close(region));
    VR(vr_v1_gpu_close(gpu));
    CU(cuModuleUnload(module));
    CU(cuCtxPopCurrent_v2(&context));
    CU(cuDevicePrimaryCtxRelease_v2(0));
    dlclose(cuda);
    puts("{\"native_engine\":\"pass\",\"passes\":2,\"values\":[2,4,6,8]}");
    return 0;
}
