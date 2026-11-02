/* Official CUDA header types; actual Driver entrypoints, no Varena replacement.
 * Reuse the already-qualified CPU-only gate and bounded observation helpers. */
#define main pinned_legacy_main
#include "pinned_host.c"
#undef main
#include <cuda.h>

#define FN(name) __typeof__(&name) name
static struct {
    FN(cuDeviceGetAttribute); FN(cuMemPoolCreate); FN(cuMemPoolSetAccess);
    FN(cuMemAllocFromPoolAsync); FN(cuMemFreeAsync); FN(cuMemPoolDestroy); FN(cuMemPoolGetAttribute);
    FN(cuMemGetAllocationGranularity); FN(cuMemAddressReserve); FN(cuMemCreate);
    FN(cuMemMap); FN(cuMemSetAccess); FN(cuMemUnmap); FN(cuMemRelease);
    FN(cuMemAddressFree); FN(cuMemAlloc_v2); FN(cuMemFree_v2);
    FN(cuMemcpyHtoDAsync_v2); FN(cuMemcpyDtoHAsync_v2);
} modern;
#undef FN
#define MOD(name) do { *(void **)(&modern.name)=dlsym(lib,#name); REQUIRE(modern.name!=NULL,#name); } while(0)
static void stage(const char *label) {
    fprintf(stderr,"{\"diagnostic\":\"modern_host.%s\"}\n",label);fflush(stderr);
}
struct ModernTrial {
    Obj context, stream, event;
    CUdeviceptr pointer;
    CUmemGenericAllocationHandle handle;
    CUmemoryPool pool;
    size_t size;
    uint64_t reserved_after_alloc, reserved_after_free, used_after_alloc, used_after_free;
    int pooled;
    _Atomic int entered,returned,release_completed;
    uint64_t entered_ns,returned_ns,completed_ns;
};
static void *modern_trial(void *raw) {
    struct ModernTrial *t=raw;OK("worker push",api.push(t->context));
    t->entered_ns=now_ns();atomic_store_explicit(&t->entered,1,memory_order_release);
    if(t->pooled) {
        stage("before_free_async");OK("cuMemFreeAsync",modern.cuMemFreeAsync(t->pointer,t->stream));stage("after_free_async");
        OK("free event record",api.record(t->event,t->stream));completed(t->event,"free event completion");
        t->completed_ns=now_ns();atomic_store_explicit(&t->release_completed,1,memory_order_release);
        OK("pool reserved after free",modern.cuMemPoolGetAttribute(t->pool,CU_MEMPOOL_ATTR_RESERVED_MEM_CURRENT,&t->reserved_after_free));
        OK("pool used after free",modern.cuMemPoolGetAttribute(t->pool,CU_MEMPOOL_ATTR_USED_MEM_CURRENT,&t->used_after_free));
        stage("before_pool_destroy");OK("cuMemPoolDestroy",modern.cuMemPoolDestroy(t->pool));stage("after_pool_destroy");
    } else {
        stage("before_unmap");OK("cuMemUnmap",modern.cuMemUnmap(t->pointer,t->size));stage("after_unmap");
        stage("before_release");OK("cuMemRelease",modern.cuMemRelease(t->handle));stage("after_release");
        stage("before_address_free");OK("cuMemAddressFree",modern.cuMemAddressFree(t->pointer,t->size));stage("after_address_free");
        t->completed_ns=now_ns();atomic_store_explicit(&t->release_completed,1,memory_order_release);
    }
    stage("before_target_event_destroy");OK("target event destroy",api.event_destroy(t->event));stage("after_target_event_destroy");
    stage("before_target_stream_destroy");OK("target stream destroy",api.stream_destroy(t->stream));stage("after_target_stream_destroy");
    t->returned_ns=now_ns();Obj old=NULL;OK("worker pop",api.pop(&old));
    atomic_store_explicit(&t->returned,1,memory_order_release);return NULL;
}
static int unsupported(const char *mode,const char *stage_name,int rc,int host_pool,int numa_pool,int device,const char *uuid) {
    Obj old=NULL;OK("unsupported pop",api.pop(&old));OK("unsupported release",api.release(device));
    printf("{\"passed\":true,\"qualified\":false,\"supported\":false,\"mode\":\"%s\",\"gpu_uuid\":\"%s\",\"stage\":\"%s\",\"status\":%d,\"host_pool_support\":%d,\"host_numa_pool_support\":%d,\"scope\":\"capability refusal; no allocation/release claim\"}\n",mode,uuid,stage_name,rc,host_pool,numa_pool);
    return 0;
}
int main(int argc,char **argv) {
    REQUIRE(argc==3||argc==4,"expected pool|pool_small|vmm|vmm_small actual-UUID [device]");
    int small=!strcmp(argv[1],"pool_small")||!strcmp(argv[1],"vmm_small");int pooled=!strcmp(argv[1],"pool_small")||!strcmp(argv[1],"pool");REQUIRE(pooled||!strcmp(argv[1],"vmm")||!strcmp(argv[1],"vmm_small"),"mode");
    int device=argc==4?atoi(argv[3]):0;
    void *lib=dlopen("libcuda.so.1",RTLD_NOW|RTLD_LOCAL);REQUIRE(lib!=NULL,"libcuda");
    LOAD(init,"cuInit");LOAD(retain,"cuDevicePrimaryCtxRetain");LOAD(push,"cuCtxPushCurrent_v2");LOAD(pop,"cuCtxPopCurrent_v2");LOAD(release,"cuDevicePrimaryCtxRelease_v2");
    LOAD(uuid,"cuDeviceGetUuid_v2");LOAD(stream_new,"cuStreamCreate");LOAD(stream_destroy,"cuStreamDestroy_v2");LOAD(wait,"cuStreamWaitValue32_v2");
    LOAD(event_new,"cuEventCreate");LOAD(record,"cuEventRecord");LOAD(query,"cuEventQuery");LOAD(event_destroy,"cuEventDestroy_v2");
    LOAD(host_new,"cuMemHostAlloc");LOAD(host_device,"cuMemHostGetDevicePointer_v2");LOAD(host_free,"cuMemFreeHost");
    MOD(cuDeviceGetAttribute);MOD(cuMemPoolCreate);MOD(cuMemPoolSetAccess);MOD(cuMemAllocFromPoolAsync);MOD(cuMemFreeAsync);MOD(cuMemPoolDestroy);MOD(cuMemPoolGetAttribute);
    MOD(cuMemGetAllocationGranularity);MOD(cuMemAddressReserve);MOD(cuMemCreate);MOD(cuMemMap);MOD(cuMemSetAccess);MOD(cuMemUnmap);MOD(cuMemRelease);MOD(cuMemAddressFree);
    MOD(cuMemAlloc_v2);MOD(cuMemFree_v2);MOD(cuMemcpyHtoDAsync_v2);MOD(cuMemcpyDtoHAsync_v2);
    OK("init",api.init(0));Obj context=NULL;OK("retain",api.retain(&context,device));OK("push",api.push(context));
    unsigned char uuid[16];char actual[33];OK("uuid",api.uuid(uuid,device));
    for(int i=0;i<16;++i)snprintf(actual+2*i,3,"%02x",uuid[i]);
    const char *wanted=argv[2];if(!strncmp(wanted,"GPU-",4)||!strncmp(wanted,"MIG-",4))wanted+=4;
    char normalized[33];size_t n=0;for(;*wanted;++wanted)if(*wanted!='-'){REQUIRE(n<32,"UUID length");char c=*wanted;normalized[n++]=(c>='A'&&c<='F')?(char)(c+32):c;}
    normalized[n]='\0';REQUIRE(n==32&&!strcmp(normalized,actual),"GPU UUID mismatch");
    int host_pool=-1,numa_pool=-1;
    int host_rc=modern.cuDeviceGetAttribute(&host_pool,CU_DEVICE_ATTRIBUTE_HOST_MEMORY_POOLS_SUPPORTED,device);
    int numa_rc=modern.cuDeviceGetAttribute(&numa_pool,CU_DEVICE_ATTRIBUTE_HOST_NUMA_MEMORY_POOLS_SUPPORTED,device);
    fprintf(stderr,"{\"host_pool_support\":%d,\"host_pool_status\":%d,\"host_numa_pool_support\":%d,\"host_numa_pool_status\":%d,\"CUDA_header_version\":%d}\n",host_pool,host_rc,numa_pool,numa_rc,CUDA_VERSION);fflush(stderr);
    if(pooled&&(host_rc||!host_pool))return unsupported(argv[1],"HOST_MEMORY_POOLS_SUPPORTED",host_rc,host_pool,numa_pool,device,argv[2]);
    CUmemAllocationProp prop={0};prop.type=CU_MEM_ALLOCATION_TYPE_PINNED;prop.location.type=CU_MEM_LOCATION_TYPE_HOST;
    size_t bytes=(small?2u:4u)<<20,granularity=0;size_t maximum=small?bytes:8u<<20;
    int granularity_status=modern.cuMemGetAllocationGranularity(&granularity,&prop,CU_MEM_ALLOC_GRANULARITY_MINIMUM);
    fprintf(stderr,"{\"host_vmm_granularity_status\":%d,\"host_vmm_granularity\":%zu}\n",granularity_status,granularity);fflush(stderr);
    if(!pooled) {
        if(granularity_status)return unsupported(argv[1],"HOST granularity",granularity_status,host_pool,numa_pool,device,argv[2]);
        REQUIRE(granularity&&granularity<=64u<<20,"bounded granularity");
        bytes=((bytes+granularity-1)/granularity)*granularity;
    }
    Obj stream=NULL,event=NULL,blocked=NULL,gate_event=NULL;
    OK("stream",api.stream_new(&stream,1));OK("event",api.event_new(&event,2));
    OK("blocked",api.stream_new(&blocked,1));OK("gate event",api.event_new(&gate_event,2));
    struct ModernTrial t={.context=context,.stream=stream,.event=event,.size=bytes,.pooled=pooled};
    CUmemAccessDesc access[2]={0};access[0].location.type=CU_MEM_LOCATION_TYPE_DEVICE;access[0].location.id=device;access[0].flags=CU_MEM_ACCESS_FLAGS_PROT_READWRITE;
    access[1].location.type=CU_MEM_LOCATION_TYPE_HOST;access[1].flags=CU_MEM_ACCESS_FLAGS_PROT_READWRITE;
    if(pooled) {
        CUmemPoolProps properties={0};properties.allocType=CU_MEM_ALLOCATION_TYPE_PINNED;properties.location.type=CU_MEM_LOCATION_TYPE_HOST;properties.maxSize=maximum;
        stage("before_pool_create");OK("HOST pool create",modern.cuMemPoolCreate(&t.pool,&properties));stage("after_pool_create");
        OK("pool device access",modern.cuMemPoolSetAccess(t.pool,access,1));
        OK("pool alloc",modern.cuMemAllocFromPoolAsync(&t.pointer,bytes,t.pool,stream));
        OK("allocation event",api.record(event,stream));completed(event,"allocation completion before CPU access");
        OK("pool reserved after alloc",modern.cuMemPoolGetAttribute(t.pool,CU_MEMPOOL_ATTR_RESERVED_MEM_CURRENT,&t.reserved_after_alloc));
        OK("pool used after alloc",modern.cuMemPoolGetAttribute(t.pool,CU_MEMPOOL_ATTR_USED_MEM_CURRENT,&t.used_after_alloc));
        fprintf(stderr,"{\"event\":\"pool_after_allocation\",\"maximum_bytes\":%zu,\"allocation_bytes\":%zu,\"reserved_bytes\":%llu,\"used_bytes\":%llu}\n",maximum,bytes,(unsigned long long)t.reserved_after_alloc,(unsigned long long)t.used_after_alloc);fflush(stderr);
        REQUIRE(t.reserved_after_alloc<=maximum,"pool actual backing exceeds maxSize");
    } else {
        stage("before_vmm_create");OK("HOST VMM create",modern.cuMemCreate(&t.handle,bytes,&prop,0));stage("after_vmm_create");
        OK("reserve",modern.cuMemAddressReserve(&t.pointer,bytes,0,0,0));OK("map",modern.cuMemMap(t.pointer,bytes,0,t.handle,0));
        OK("VMM CPU+GPU access",modern.cuMemSetAccess(t.pointer,bytes,access,2));
    }
    /* Actual CPU write -> H2D -> D2H full-byte proof, before unrelated work starts. */
    memset((void *)(uintptr_t)t.pointer,0x5a,bytes);
    CUdeviceptr output=0;OK("device output",modern.cuMemAlloc_v2(&output,bytes));
    void *readback=NULL;OK("pinned readback",api.host_new(&readback,bytes,1));
    OK("H2D",modern.cuMemcpyHtoDAsync_v2(output,(void *)(uintptr_t)t.pointer,bytes,stream));
    OK("D2H",modern.cuMemcpyDtoHAsync_v2(readback,output,bytes,stream));
    OK("copy event",api.record(event,stream));completed(event,"actual copies");
    for(size_t i=0;i<bytes;++i)REQUIRE(((unsigned char *)readback)[i]==0x5a,"full-byte readback");
    OK("readback free before gate",api.host_free(readback));OK("output free before gate",modern.cuMemFree_v2(output));
    void *gate_raw=NULL;uint64_t gate_device=0;OK("mapped gate",api.host_new(&gate_raw,4,3));OK("gate pointer",api.host_device(&gate_device,gate_raw,0));
    REQUIRE((uintptr_t)gate_raw%_Alignof(_Atomic uint32_t)==0,"gate alignment");_Atomic uint32_t *gate=gate_raw;atomic_init(gate,0);REQUIRE(atomic_is_lock_free(gate),"CPU gate store lock-free");
    OK("preflight wait",api.wait(blocked,gate_device,1,1));OK("preflight event",api.record(gate_event,blocked));REQUIRE(api.query(gate_event)==600,"preflight pending");
    atomic_store_explicit(gate,1,memory_order_release);completed(gate_event,"CPU gate preflight");
    atomic_store_explicit(gate,0,memory_order_release);OK("hold",api.wait(blocked,gate_device,1,1));OK("held event",api.record(gate_event,blocked));REQUIRE(api.query(gate_event)==600,"experiment pending");
    pthread_t thread;REQUIRE(!pthread_create(&thread,NULL,modern_trial,&t),"pthread_create");
    uint64_t deadline=now_ns()+10000000000ull;while(!atomic_load_explicit(&t.entered,memory_order_acquire)) {if(now_ns()>=deadline){atomic_store_explicit(gate,1,memory_order_release);failed("API entry",-1);}pause_cpu();}
    uint64_t observe_until=now_ns()+100000000ull;while(!atomic_load_explicit(&t.returned,memory_order_acquire)&&now_ns()<observe_until)pause_cpu();
    int returned_closed=atomic_load_explicit(&t.returned,memory_order_acquire);
    int completed_closed=atomic_load_explicit(&t.release_completed,memory_order_acquire);
    uint64_t release_ns=now_ns();atomic_store_explicit(gate,1,memory_order_release);stage("CPU_gate_released");
    deadline=now_ns()+10000000000ull;while(!atomic_load_explicit(&t.returned,memory_order_acquire)){if(now_ns()>=deadline)failed("release return after CPU signal",-1);pause_cpu();}
    REQUIRE(!pthread_join(thread,NULL),"join");completed(gate_event,"gate complete");
    OK("destroy gate event",api.event_destroy(gate_event));OK("destroy blocked",api.stream_destroy(blocked));OK("free gate",api.host_free(gate_raw));
    Obj old=NULL;OK("pop",api.pop(&old));OK("release context",api.release(device));
    if(pooled)fprintf(stderr,"{\"event\":\"pool_accounting\",\"maximum_bytes\":%zu,\"allocation_bytes\":%zu,\"reserved_after_alloc\":%llu,\"used_after_alloc\":%llu,\"reserved_after_free\":%llu,\"used_after_free\":%llu}\n",maximum,bytes,(unsigned long long)t.reserved_after_alloc,(unsigned long long)t.used_after_alloc,(unsigned long long)t.reserved_after_free,(unsigned long long)t.used_after_free);
    printf("{\"passed\":true,\"supported\":true,\"qualified\":%s,\"mode\":\"%s\",\"gpu_uuid\":\"%s\",\"bytes\":%zu,\"host_pool_support\":%d,\"host_numa_pool_support\":%d,\"full_byte_readback\":true,\"entire_target_retirement_measured\":true,\"returned_while_gate_closed\":%s,\"release_completed_while_gate_closed\":%s,\"entered_ns\":%llu,\"completed_ns\":%llu,\"returned_ns\":%llu,\"CPU_gate_released_ns\":%llu,\"gate_release_used_cuda\":false,\"scope\":\"one direct API capability/progress sample; not a product or all-platform guarantee\"}\n",returned_closed&&completed_closed?"true":"false",argv[1],argv[2],bytes,host_pool,numa_pool,returned_closed?"true":"false",completed_closed?"true":"false",(unsigned long long)t.entered_ns,(unsigned long long)t.completed_ns,(unsigned long long)t.returned_ns,(unsigned long long)release_ns);
    return 0;
}
