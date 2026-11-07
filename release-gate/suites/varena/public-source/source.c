/* Real C provider jobs; no Python, Torch, fabricated CUDA completion or product hooks. */
#include "varena.h"
#include <dlfcn.h>
#include <pthread.h>
#include <stdatomic.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <time.h>

typedef void *CU;
static struct {
    void *library;
    int (*stream_create)(CU *, unsigned);
    int (*stream_destroy)(CU);
    int (*stream_sync)(CU);
    int (*stream_query)(CU);
    int (*write)(CU,uint64_t,uint32_t,unsigned);
    int (*wait)(CU,uint64_t,uint32_t,unsigned);
    int (*set)(uint64_t,unsigned char,size_t,CU);
    int (*copy)(uint64_t,uint64_t,size_t,CU);
    int (*read)(void *,uint64_t,size_t);
    int (*host_allocate)(void **,size_t,unsigned);
    int (*host_device)(uint64_t *,void *,unsigned);
    int (*host_free)(void *);
} cuda;
static struct {
    atomic_uint prepares, submits, job_retired, job_dropped, source_retired, source_dropped;
    atomic_uint source_pending;
} counts;
static int check(int status,const char *what) {
    if(status) {fprintf(stderr,"%s failed: %d (%s)\n",what,status,vr_v1_last_error());return 0;}
    return 1;
}
#define OK(call) do {if(!check((call),#call)) return 1;} while(0)
#define REQUIRE(test) do {if(!(test)){fprintf(stderr,"requirement failed: %s\n",#test);return 1;}} while(0)
#define LOAD(field,name) do {*(void **)(&cuda.field)=dlsym(cuda.library,name);if(!cuda.field){fprintf(stderr,"missing %s\n",name);return 1;}} while(0)
static uint64_t now_ms(void) {struct timespec t;clock_gettime(CLOCK_MONOTONIC,&t);return (uint64_t)t.tv_sec*1000+(uint64_t)t.tv_nsec/1000000;}
static void pause_poll(void){struct timespec t={0,1000000};nanosleep(&t,NULL);}
struct Source;
struct Job {struct Source *source;uint64_t streams[1];int retired;struct Job *next;};
struct Source {
    uint64_t gate;
    atomic_int partial;
    int fail_prepare;
    pthread_mutex_t borrowed;
    struct Job *failed;
    int retired;
};
static int job_retire(void *context) {
    struct Job *job=context;
    if(job->retired) return VR_OK;
    int status=cuda.stream_destroy((CU)(uintptr_t)job->streams[0]);
    if(status) return VR_FAULT;
    job->retired=1;
    atomic_fetch_add(&counts.job_retired,1);
    return VR_OK;
}
static void job_drop(void *context) {
    struct Job *job=context;
    if(!job->retired) abort(); /* inert destructor is the actual provider contract */
    atomic_fetch_add(&counts.job_dropped,1);
    free(job);
}
static int job_submit(void *context,uint64_t dst,uint64_t bytes) {
    struct Job *job=context;
    atomic_fetch_add(&counts.submits,1);
    int status=cuda.wait((CU)(uintptr_t)job->streams[0],job->source->gate,1,1);
    if(status) return VR_FAULT;
    int partial=atomic_load(&job->source->partial);
    status=cuda.set(dst,73,(size_t)(partial?bytes/2:bytes),(CU)(uintptr_t)job->streams[0]);
    if(status) return VR_FAULT;
    /* The C callback has no other host submitter. Partial mode really submitted only
       half the destination and reports a closed Source failure, never valid contents. */
    return partial?VR_SOURCE_ERROR:VR_OK;
}
static int source_prepare(void *context,int32_t device,uint64_t offset,uint64_t bytes,vr_source_job_v1 *out) {
    (void)device;(void)offset;(void)bytes;
    struct Source *source=context;
    atomic_fetch_add(&counts.prepares,1);
    struct Job *job=calloc(1,sizeof(*job));
    if(!job) return VR_SOURCE_ERROR;
    job->source=source;
    CU stream=NULL;
    int status=cuda.stream_create(&stream,1);
    if(status){free(job);return VR_FAULT;}
    job->streams[0]=(uint64_t)(uintptr_t)stream;
    if(source->fail_prepare) {
        /* Nothing transferred. The persistent Source keeps this actual created stream. */
        job->next=source->failed;source->failed=job;
        return VR_SOURCE_ERROR;
    }
    vr_source_job_v1 ready={job->streams,1,job_submit,job_retire,job_drop,job};
    *out=ready;
    return VR_OK;
}
static int source_retire(void *context) {
    struct Source *source=context;
    if(pthread_mutex_trylock(&source->borrowed)!=0) {
        atomic_fetch_add(&counts.source_pending,1);
        return VR_BUSY; /* real retained native owner lock; never blocks the retirement worker */
    }
    while(source->failed) {
        struct Job *job=source->failed;
        int status=job_retire(job);
        if(status){pthread_mutex_unlock(&source->borrowed);return status;}
        source->failed=job->next;job_drop(job);
    }
    source->retired=1;
    atomic_fetch_add(&counts.source_retired,1);
    pthread_mutex_unlock(&source->borrowed);
    return VR_OK;
}
static void source_drop(void *context) {
    struct Source *source=context;
    if(!source->retired||source->failed) abort();
    pthread_mutex_destroy(&source->borrowed);
    atomic_fetch_add(&counts.source_dropped,1);
    free(source);
}
static int retired_until(unsigned jobs,unsigned sources) {
    uint64_t deadline=now_ms()+10000;
    while(now_ms()<deadline) {
        vr_retirement_stats_v1 stats={0};
        if(vr_v1_retirement_stats(&stats,sizeof(stats))||stats.quarantined) return 0;
        if(!stats.queued&&atomic_load(&counts.job_dropped)>=jobs&&atomic_load(&counts.source_dropped)>=sources) return 1;
        pause_poll();
    }
    return 0;
}
static int host_contract(void) {
    vr_source_provider_v1 provider={source_prepare,source_retire,source_drop,NULL};
    vr_handle output=UINT64_MAX;
    REQUIRE(vr_v1_region_from_source(0,1024,&provider,sizeof(provider),&output)==VR_INVALID);
    REQUIRE(output==0&&atomic_load(&counts.prepares)==0&&atomic_load(&counts.source_dropped)==0);
    REQUIRE(vr_v1_region_from_source(0,1024,&provider,sizeof(provider)-1,&output)==VR_INVALID&&output==0);
    FILE *maps=fopen("/proc/self/maps","r");REQUIRE(maps!=NULL);char line[4096];
    while(fgets(line,sizeof(line),maps)) REQUIRE(strstr(line,"libcuda.so")==NULL);
    fclose(maps);
    puts("{\"passed\":true,\"mode\":\"host\",\"constructor_retained_provider\":true,\"cuda_loaded\":false}");
    return 0;
}
static int host_bytes(int device,const char *uuid) {
    CU control=NULL,consumer=NULL,blocked=NULL;
    OK(cuda.stream_create(&control,1));OK(cuda.stream_create(&consumer,1));OK(cuda.stream_create(&blocked,1));
    void *host_gate=NULL;uint64_t gate=0;OK(cuda.host_allocate(&host_gate,4,3));OK(cuda.host_device(&gate,host_gate,0));
    OK(cuda.write(control,gate,0,0));OK(cuda.stream_sync(control));
    OK(cuda.wait(blocked,gate,1,1));REQUIRE(cuda.stream_query(blocked)==600);
    vr_handle gpu=0,region=0,used=0,allocation=0;uint64_t out=0,pointer=0;
    const size_t bytes=(5u<<20)+31; /* More than two reuse boundaries and a short tail. */
    unsigned char *source=malloc(bytes);REQUIRE(source!=NULL);
    for(size_t i=0;i<bytes;++i) source[i]=(unsigned char)((i*17+9)%251);
    OK(vr_v1_gpu_open(device,&gpu));
    OK(vr_v1_region_from_host(gpu,source,bytes,&region));memset(source,0,bytes);free(source);
    OK(vr_v1_alloc(gpu,bytes,(uint64_t)(uintptr_t)consumer,&allocation));OK(vr_v1_allocation_ptr(allocation,&out));
    /* A context-wide source join would block here behind the independent gate. */
    OK(vr_v1_use(gpu,&region,1,(uint64_t)(uintptr_t)consumer,&used));OK(vr_v1_use_ptr(used,0,&pointer));
    OK(cuda.copy(out,pointer,bytes,consumer));OK(cuda.stream_sync(consumer));
    REQUIRE(cuda.stream_query(blocked)==600);
    unsigned char *read=malloc(bytes);REQUIRE(read!=NULL);OK(cuda.read(read,out,bytes));
    for(size_t i=0;i<bytes;++i) {REQUIRE(read[i]==(unsigned char)((i*17+9)%251));}free(read);
    OK(vr_v1_done(used));OK(vr_v1_region_close(region));
    OK(cuda.write(control,gate,1,0));OK(cuda.stream_sync(control));OK(cuda.stream_sync(blocked));
    OK(vr_v1_free(allocation));OK(cuda.stream_sync(consumer));
    OK(vr_v1_gpu_close(gpu));
    uint64_t deadline=now_ms()+10000;vr_retirement_stats_v1 stats={0};
    do{OK(vr_v1_retirement_stats(&stats,sizeof(stats)));if(!stats.queued)break;pause_poll();}while(now_ms()<deadline);
    REQUIRE(!stats.queued&&!stats.quarantined);OK(cuda.host_free(host_gate));
    OK(cuda.stream_destroy(blocked));OK(cuda.stream_destroy(consumer));OK(cuda.stream_destroy(control));
    printf("{\"passed\":true,\"mode\":\"host_bytes\",\"gpu_uuid\":\"%s\",\"full_bytes\":%zu,\"caller_bytes_dropped\":true,\"independent_gate_still_pending_during_readback\":true}\n",uuid,bytes);
    return 0;
}
int main(int argc,char **argv) {
    if(argc==2&&strcmp(argv[1],"host")==0) return host_contract();
    if(argc!=4){fprintf(stderr,"usage: source-c MODE GPU-UUID DEVICE\n");return 2;}
    const char *mode=argv[1];int device=atoi(argv[3]);
    int partial=strcmp(mode,"partial")==0, prepare_failure=strcmp(mode,"prepare_error")==0;
    int held=strcmp(mode,"source_pending")==0, retained=strcmp(mode,"retained")==0;
    int host_mode=strcmp(mode,"host_bytes")==0;
    REQUIRE(partial||prepare_failure||held||retained||host_mode||strcmp(mode,"ordering")==0);
    cuda.library=dlopen("libcuda.so.1",RTLD_NOW|RTLD_LOCAL);REQUIRE(cuda.library!=NULL);
    int (*init)(unsigned),(*retain)(CU*,int),(*push)(CU),(*uuid)(unsigned char*,int);
    *(void **)(&init)=dlsym(cuda.library,"cuInit");*(void **)(&retain)=dlsym(cuda.library,"cuDevicePrimaryCtxRetain");
    *(void **)(&push)=dlsym(cuda.library,"cuCtxPushCurrent_v2");*(void **)(&uuid)=dlsym(cuda.library,"cuDeviceGetUuid_v2");
    REQUIRE(init&&retain&&push&&uuid);OK(init(0));CU context=NULL;OK(retain(&context,device));OK(push(context));
    unsigned char identity[16];OK(uuid(identity,device));char actual[33];
    for(int i=0;i<16;++i) snprintf(actual+2*i,3,"%02x",identity[i]);
    char expected[33]={0};const char *input=argv[2];if(strncmp(input,"GPU-",4)==0) input+=4;
    int at=0;for(;*input&&at<32;++input)if(*input!='-')expected[at++]=*input;
    REQUIRE(at==32&&strcmp(expected,actual)==0);
    LOAD(stream_create,"cuStreamCreate");LOAD(stream_destroy,"cuStreamDestroy_v2");
    LOAD(stream_sync,"cuStreamSynchronize");LOAD(stream_query,"cuStreamQuery");
    LOAD(write,"cuStreamWriteValue32_v2");LOAD(wait,"cuStreamWaitValue32_v2");
    LOAD(set,"cuMemsetD8Async");LOAD(copy,"cuMemcpyDtoDAsync_v2");LOAD(read,"cuMemcpyDtoH_v2");
    LOAD(host_allocate,"cuMemHostAlloc");LOAD(host_device,"cuMemHostGetDevicePointer_v2");LOAD(host_free,"cuMemFreeHost");
    if(host_mode) return host_bytes(device,actual);
    CU control=NULL,consumer=NULL;OK(cuda.stream_create(&control,1));OK(cuda.stream_create(&consumer,1));
    vr_handle gpu=0,out_allocation=0,region=0,used=0,plan=0;
    const uint64_t bytes=1<<20;uint64_t gate=0,out=0,pointer=0,stable=0;
    OK(vr_v1_gpu_open(device,&gpu));
    /* This retained mapped HOST gate is not a GPU allocation or a borrowed Working
       side stream. Source-created streams may retire before this gate is released. */
    void *host_gate=NULL;OK(cuda.host_allocate(&host_gate,4,3));OK(cuda.host_device(&gate,host_gate,0));
    OK(vr_v1_alloc(gpu,bytes,(uint64_t)(uintptr_t)consumer,&out_allocation));OK(vr_v1_allocation_ptr(out_allocation,&out));
    OK(cuda.write(control,gate,0,0));OK(cuda.stream_sync(control));
    struct Source *source=calloc(1,sizeof(*source));REQUIRE(source!=NULL);source->gate=gate;
    atomic_init(&source->partial,partial);source->fail_prepare=prepare_failure;pthread_mutex_init(&source->borrowed,NULL);
    vr_source_provider_v1 provider={source_prepare,source_retire,source_drop,source};
    OK(vr_v1_region_from_source(gpu,bytes,&provider,sizeof(provider),&region));OK(vr_v1_region_ptr(region,&stable));
    if(retained) {vr_phase_v1 phase={"weights",&region,1};OK(vr_v1_plan_new(gpu,&phase,1,&plan));}
    int status=vr_v1_use(gpu,&region,1,(uint64_t)(uintptr_t)consumer,&used);
    if(prepare_failure) {
        REQUIRE(status==VR_SOURCE_ERROR&&used==0&&atomic_load(&counts.submits)==0);
        REQUIRE(atomic_load(&counts.job_dropped)==0);OK(vr_v1_region_close(region));
        REQUIRE(retired_until(1,1));
    } else {
        if(partial) {
            REQUIRE(status==VR_SOURCE_ERROR&&used==0&&atomic_load(&counts.job_dropped)==0);
            OK(cuda.write(control,gate,1,0));OK(cuda.stream_sync(control));REQUIRE(retired_until(1,0));
            atomic_store(&source->partial,0);OK(cuda.write(control,gate,0,0));OK(cuda.stream_sync(control));
            OK(vr_v1_use(gpu,&region,1,(uint64_t)(uintptr_t)consumer,&used));
        } else REQUIRE(status==VR_OK);
        OK(vr_v1_use_ptr(used,0,&pointer));REQUIRE(pointer==stable);
        OK(cuda.copy(out,pointer,(size_t)bytes,consumer));REQUIRE(cuda.stream_query(consumer)==600);
        REQUIRE(atomic_load(&counts.job_dropped)==(partial?1u:0u));
        if(held) pthread_mutex_lock(&source->borrowed);
        OK(vr_v1_region_close(region));
        if(!retained) {OK(vr_v1_done(used));}
        vr_retirement_stats_v1 pending={0};OK(vr_v1_retirement_stats(&pending,sizeof(pending)));REQUIRE(pending.queued>0);
        OK(cuda.write(control,gate,1,0));OK(cuda.stream_sync(control));OK(cuda.stream_sync(consumer));
        unsigned char *data=malloc((size_t)bytes);REQUIRE(data!=NULL);OK(cuda.read(data,out,(size_t)bytes));
        for(uint64_t i=0;i<bytes;++i) { REQUIRE(data[i]==73); }
        free(data);
        if(held) {
            uint64_t deadline=now_ms()+10000;
            while(atomic_load(&counts.source_pending)==0&&now_ms()<deadline) pause_poll();
            REQUIRE(atomic_load(&counts.source_pending)>0&&atomic_load(&counts.source_dropped)==0);
            OK(vr_v1_retirement_stats(&pending,sizeof(pending)));REQUIRE(pending.queued>0);
            pthread_mutex_unlock(&source->borrowed);
        }
        if(retained) {
            REQUIRE(retired_until(1,0));REQUIRE(atomic_load(&counts.source_dropped)==0);
            OK(vr_v1_done(used));REQUIRE(atomic_load(&counts.source_dropped)==0);
            OK(vr_v1_plan_close(plan));
        }
        REQUIRE(retired_until(partial?2:1,1));
    }
    OK(vr_v1_free(out_allocation));
    OK(cuda.stream_sync(consumer));OK(cuda.stream_sync(control));
    vr_stats_v1 final={0};OK(vr_v1_stats(gpu,&final,sizeof(final)));
    REQUIRE(final.working_live==0);
    OK(vr_v1_gpu_close(gpu));
    uint64_t deadline=now_ms()+10000;vr_retirement_stats_v1 retirement={0};
    do {OK(vr_v1_retirement_stats(&retirement,sizeof(retirement)));if(!retirement.queued)break;pause_poll();} while(now_ms()<deadline);
    REQUIRE(!retirement.queued&&!retirement.quarantined);
    OK(cuda.host_free(host_gate));
    OK(cuda.stream_destroy(consumer));OK(cuda.stream_destroy(control));
    printf("{\"passed\":true,\"mode\":\"%s\",\"gpu_uuid\":\"%s\",\"prepares\":%u,\"submits\":%u,\"job_drops\":%u,\"source_drops\":%u,\"retirement_pending_observed\":%u,\"physical_baseline\":true}\n",
        mode,actual,atomic_load(&counts.prepares),atomic_load(&counts.submits),atomic_load(&counts.job_dropped),atomic_load(&counts.source_dropped),atomic_load(&counts.source_pending));
    return 0;
}
