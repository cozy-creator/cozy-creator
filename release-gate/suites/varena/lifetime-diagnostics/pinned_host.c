/* Bounded actual Driver API observation. No Varena replacement allocator or fake CUDA.
 * The release signal is one aligned CPU store to a distinct mapped-host word;
 * it makes no CUDA call while the tested API may hold an internal driver lock. */
#define _POSIX_C_SOURCE 200809L
#include <dlfcn.h>
#include <pthread.h>
#include <stdatomic.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <time.h>
#include <unistd.h>

typedef void *Obj;
struct Api {
    int (*init)(unsigned);
    int (*retain)(Obj *,int);
    int (*push)(Obj);
    int (*pop)(Obj *);
    int (*release)(int);
    int (*uuid)(unsigned char *,int);
    int (*stream_new)(Obj *,unsigned);
    int (*stream_destroy)(Obj);
    int (*wait)(Obj,uint64_t,uint32_t,unsigned);
    int (*event_new)(Obj *,unsigned);
    int (*record)(Obj,Obj);
    int (*query)(Obj);
    int (*event_destroy)(Obj);
    int (*host_new)(void **,size_t,unsigned);
    int (*host_device)(uint64_t *,void *,unsigned);
    int (*host_free)(void *);
} api;

static uint64_t now_ns(void) {
    struct timespec t; clock_gettime(CLOCK_MONOTONIC,&t);
    return (uint64_t)t.tv_sec*1000000000ull+(uint64_t)t.tv_nsec;
}
static void pause_cpu(void) { struct timespec t={0,1000000}; nanosleep(&t,NULL); }
static void failed(const char *stage,int code) {
    fprintf(stderr,"{\"passed\":false,\"stage\":\"%s\",\"status\":%d,\"cleanup_proven\":false}\n",stage,code);
    fflush(stderr); _Exit(2); /* Unknown GPU ownership is left to this owned process exit. */
}
#define OK(stage,call) do { int rc=(call); if(rc)failed(stage,rc); } while(0)
#define REQUIRE(test,stage) do { if(!(test))failed(stage,-1); } while(0)
#define LOAD(field,name) do { *(void **)(&api.field)=dlsym(lib,name); REQUIRE(api.field!=NULL,name); } while(0)

static void completed(Obj event,const char *label) {
    uint64_t until=now_ns()+10000000000ull;
    for(;;) {
        int rc=api.query(event); if(!rc)return;
        if(rc!=600)failed(label,rc);
        if(now_ns()>=until)failed(label,600);
        pause_cpu();
    }
}
struct Trial {
    Obj context;
    int allocate;
    void *target;
    _Atomic int entered,returned;
    uint64_t entered_ns,returned_ns;
    int status;
};
static void *trial(void *raw) {
    struct Trial *t=raw;
    OK("worker_context_push",api.push(t->context));
    t->entered_ns=now_ns();
    fprintf(stderr,"{\"diagnostic\":\"pinned_api.enter\",\"operation\":\"%s\"}\n",t->allocate?"cuMemHostAlloc":"cuMemFreeHost");fflush(stderr);
    atomic_store_explicit(&t->entered,1,memory_order_release);
    t->status=t->allocate?api.host_new(&t->target,4u<<20,1):api.host_free(t->target);
    t->returned_ns=now_ns();
    fprintf(stderr,"{\"diagnostic\":\"pinned_api.return\",\"status\":%d}\n",t->status);fflush(stderr);
    Obj old=NULL; OK("worker_context_pop",api.pop(&old));
    atomic_store_explicit(&t->returned,1,memory_order_release);
    return NULL;
}

int main(int argc,char **argv) {
    REQUIRE(argc==3||argc==4,"expected alloc|free actual-UUID [device]");
    int allocate=strcmp(argv[1],"alloc")==0;
    REQUIRE(allocate||strcmp(argv[1],"free")==0,"operation");
    int device=argc==4?atoi(argv[3]):0;
    void *lib=dlopen("libcuda.so.1",RTLD_NOW|RTLD_LOCAL);REQUIRE(lib!=NULL,"libcuda");
    LOAD(init,"cuInit");LOAD(retain,"cuDevicePrimaryCtxRetain");LOAD(push,"cuCtxPushCurrent_v2");LOAD(pop,"cuCtxPopCurrent_v2");LOAD(release,"cuDevicePrimaryCtxRelease_v2");
    LOAD(uuid,"cuDeviceGetUuid_v2");LOAD(stream_new,"cuStreamCreate");LOAD(stream_destroy,"cuStreamDestroy_v2");LOAD(wait,"cuStreamWaitValue32_v2");
    LOAD(event_new,"cuEventCreate");LOAD(record,"cuEventRecord");LOAD(query,"cuEventQuery");LOAD(event_destroy,"cuEventDestroy_v2");
    LOAD(host_new,"cuMemHostAlloc");LOAD(host_device,"cuMemHostGetDevicePointer_v2");LOAD(host_free,"cuMemFreeHost");
    OK("init",api.init(0));Obj context=NULL;OK("retain",api.retain(&context,device));OK("push",api.push(context));
    unsigned char uuid[16];char actual[33];OK("uuid",api.uuid(uuid,device));
    for(int i=0;i<16;++i)snprintf(actual+2*i,3,"%02x",uuid[i]);
    const char *wanted=argv[2];if(!strncmp(wanted,"GPU-",4)||!strncmp(wanted,"MIG-",4))wanted+=4;
    char normalized[33];size_t n=0;for(;*wanted;++wanted)if(*wanted!='-'){REQUIRE(n<32,"UUID length");char c=*wanted;normalized[n++]=(c>='A'&&c<='F')?(char)(c+32):c;}
    normalized[n]='\0';REQUIRE(n==32&&!strcmp(normalized,actual),"GPU UUID mismatch");
    Obj stream=NULL,event=NULL;OK("stream",api.stream_new(&stream,1));OK("event",api.event_new(&event,2));
    void *gate_raw=NULL;uint64_t gate_device=0;OK("mapped_gate",api.host_new(&gate_raw,4,3));OK("gate_pointer",api.host_device(&gate_device,gate_raw,0));
    REQUIRE((uintptr_t)gate_raw%_Alignof(_Atomic uint32_t)==0,"gate alignment");
    _Atomic uint32_t *gate=gate_raw;atomic_init(gate,0);REQUIRE(atomic_is_lock_free(gate),"CPU gate store lock-free");
    /* One CPU writer, GPU read-only EQ wait, no payload publication or GPU atomics.
     * The event makes wait completion CUDA-visible; preflight proves this host's
     * mapping/coherence before the potentially blocking allocation/free experiment. */
    OK("preflight_wait",api.wait(stream,gate_device,1,1));OK("preflight_record",api.record(event,stream));REQUIRE(api.query(event)==600,"preflight actually pending");
    atomic_store_explicit(gate,1,memory_order_release);completed(event,"CPU mapped gate preflight");
    void *baseline=NULL;uint64_t baseline_start=now_ns();OK("baseline_alloc",api.host_new(&baseline,4u<<20,1));uint64_t baseline_alloc_ns=now_ns()-baseline_start;baseline_start=now_ns();OK("baseline_free",api.host_free(baseline));uint64_t baseline_free_ns=now_ns()-baseline_start;
    struct Trial t={.context=context,.allocate=allocate,.target=NULL,.entered=0,.returned=0,.status=-1};
    if(!allocate)OK("target_alloc_before_gate",api.host_new(&t.target,4u<<20,1));
    atomic_store_explicit(gate,0,memory_order_release);
    OK("experiment_wait",api.wait(stream,gate_device,1,1));OK("experiment_record",api.record(event,stream));REQUIRE(api.query(event)==600,"experiment actually pending");
    pthread_t thread;REQUIRE(pthread_create(&thread,NULL,trial,&t)==0,"pthread_create");
    uint64_t deadline=now_ns()+10000000000ull;
    while(!atomic_load_explicit(&t.entered,memory_order_acquire)) {
        if(now_ns()>=deadline){atomic_store_explicit(gate,1,memory_order_release);failed("API entry observation",-1);}pause_cpu();
    }
    /* A bounded observation window, never a failure/death inference or completion
     * credit. Both early return and return after release are recorded outcomes. */
    uint64_t observe_until=now_ns()+100000000ull;
    while(!atomic_load_explicit(&t.returned,memory_order_acquire)&&now_ns()<observe_until)pause_cpu();
    int returned_while_closed=atomic_load_explicit(&t.returned,memory_order_acquire);
    uint64_t released_ns=now_ns();atomic_store_explicit(gate,1,memory_order_release);
    fprintf(stderr,"{\"diagnostic\":\"CPU_gate_released\",\"CUDA_call_used\":false}\n");fflush(stderr);
    deadline=now_ns()+10000000000ull;
    while(!atomic_load_explicit(&t.returned,memory_order_acquire)) {
        if(now_ns()>=deadline)failed("API return after CPU gate release",-1);
        pause_cpu();
    }
    REQUIRE(pthread_join(thread,NULL)==0,"pthread_join");OK("tested_API",t.status);
    completed(event,"experiment event completion");
    if(allocate)OK("target_free_after_completion",api.host_free(t.target));
    OK("destroy_event",api.event_destroy(event));OK("destroy_stream",api.stream_destroy(stream));OK("free_gate",api.host_free(gate_raw));
    Obj old=NULL;OK("pop",api.pop(&old));OK("release",api.release(device));
    printf("{\"passed\":true,\"mode\":\"%s\",\"gpu_uuid\":\"%s\",\"gpu_uuid_hex\":\"%s\",\"baseline_alloc_ns\":%llu,\"baseline_free_ns\":%llu,\"entered_ns\":%llu,\"returned_ns\":%llu,\"cpu_gate_released_ns\":%llu,\"returned_while_gate_closed\":%s,\"cpu_gate_preflight_complete\":true,\"gpu_event_complete\":true,\"gate_release_used_cuda\":false,\"scope\":\"diagnostic observation only; elapsed time is not a deadlock or release-policy proof\"}\n",argv[1],argv[2],actual,(unsigned long long)baseline_alloc_ns,(unsigned long long)baseline_free_ns,(unsigned long long)t.entered_ns,(unsigned long long)t.returned_ns,(unsigned long long)released_ns,returned_while_closed?"true":"false");
    return 0;
}
