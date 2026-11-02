/* Test-only control adapter around the ACTUAL existing public Source provider.
   No producer, staging, failure or retirement implementation is copied here. */
#define main public_source_cli_main
#include "../public-source/source.c"
#undef main

#include <unistd.h>

struct te_counts_v1 {
    uint32_t prepares, submits, job_retired, job_dropped;
    uint32_t source_retired, source_dropped, source_pending;
};
struct te_gate { void *host; uint64_t device; };
struct te_fixture {
    pid_t pid;
    int device;
    CU context, control;
    struct te_gate gates[2];
    struct Source *source;
    pthread_mutex_t observation;
    pthread_mutex_t job_retirement;
    pthread_t job_hold_owner, source_hold_owner;
    int job_held, source_held;
    atomic_uint job_pending;
    uint64_t producer;
    CU borrowed[16];
    size_t borrowed_count;
};
static struct {
    int (*current)(CU *);
    int (*device)(int *);
    int (*retain)(CU *,int);
    int (*release)(int);
    int (*push)(CU);
    int (*pop)(CU *);
    int (*stream_context)(CU,CU *);
} te_context;
static pthread_mutex_t te_guard=PTHREAD_MUTEX_INITIALIZER;
static _Atomic(struct te_fixture *) te_active;
static _Thread_local char te_error[512];

uint32_t te_fixture_abi(void) {return 1;}
uint64_t te_core_identity(void) {return vr_v1_core_identity();}
const char *te_last_error(void) {return te_error;}
int te_host_contract(void) {return host_contract();}

static int te_fail(int code,const char *message) {
    snprintf(te_error,sizeof(te_error),"%s",message);
    return code;
}
static int te_cuda(int status,const char *operation) {
    if(!status) return VR_OK;
    snprintf(te_error,sizeof(te_error),"%s failed with CUDA status %d",operation,status);
    return VR_INVALID;
}
#define TE_CUDA(call) do {int result=te_cuda((call),#call);if(result)return result;} while(0)
#define TE_LOAD(table,field,name) do { \
    *(void **)(&(table).field)=dlsym(cuda.library,name); \
    if(!(table).field) return te_fail(VR_UNSUPPORTED,"missing " name); \
} while(0)

static int te_initialize(void) {
    if(te_context.current) return VR_OK;
    cuda.library=dlopen("libcuda.so.1",RTLD_NOW|RTLD_LOCAL);
    if(!cuda.library) return te_fail(VR_UNSUPPORTED,"cannot load libcuda.so.1");
    TE_LOAD(cuda,stream_create,"cuStreamCreate");
    TE_LOAD(cuda,stream_destroy,"cuStreamDestroy_v2");
    TE_LOAD(cuda,stream_sync,"cuStreamSynchronize");
    TE_LOAD(cuda,stream_query,"cuStreamQuery");
    TE_LOAD(cuda,write,"cuStreamWriteValue32_v2");
    TE_LOAD(cuda,wait,"cuStreamWaitValue32_v2");
    TE_LOAD(cuda,set,"cuMemsetD8Async");
    TE_LOAD(cuda,copy,"cuMemcpyDtoDAsync_v2");
    TE_LOAD(cuda,read,"cuMemcpyDtoH_v2");
    TE_LOAD(cuda,host_allocate,"cuMemHostAlloc");
    TE_LOAD(cuda,host_device,"cuMemHostGetDevicePointer_v2");
    TE_LOAD(cuda,host_free,"cuMemFreeHost");
    TE_LOAD(te_context,device,"cuCtxGetDevice");
    TE_LOAD(te_context,retain,"cuDevicePrimaryCtxRetain");
    TE_LOAD(te_context,release,"cuDevicePrimaryCtxRelease_v2");
    TE_LOAD(te_context,push,"cuCtxPushCurrent_v2");
    TE_LOAD(te_context,pop,"cuCtxPopCurrent_v2");
    TE_LOAD(te_context,stream_context,"cuStreamGetCtx");
    /* Publish readiness last; partially loaded tables never count as initialized. */
    TE_LOAD(te_context,current,"cuCtxGetCurrent");
    return VR_OK;
}
static int te_require(struct te_fixture *fixture) {
    if(!fixture||atomic_load_explicit(&te_active,memory_order_acquire)!=fixture)
        return te_fail(VR_INVALID,"unknown or closed Source test fixture");
    if(fixture->pid!=getpid())
        return te_fail(VR_INVALID,"Source test fixture inherited after fork");
    return VR_OK;
}
static int te_enter(struct te_fixture *fixture) {
    int result=te_require(fixture);
    return result?result:te_cuda(te_context.push(fixture->context),"cuCtxPushCurrent_v2");
}
static int te_leave(struct te_fixture *fixture,int result) {
    CU popped=NULL;
    int status=te_cuda(te_context.pop(&popped),"cuCtxPopCurrent_v2");
    if(status) return status;
    if(popped!=fixture->context) return te_fail(VR_FAULT,"fixture CUDA context stack changed");
    return result;
}
static int te_stream_context(struct te_fixture *fixture,uint64_t stream) {
    if(stream<=2||(uint64_t)(uintptr_t)stream!=stream)
        return te_fail(VR_INVALID,"test control requires an actual explicit stream");
    CU context=NULL;
    TE_CUDA(te_context.stream_context((CU)(uintptr_t)stream,&context));
    if(context!=fixture->context)
        return te_fail(VR_INVALID,"test stream is outside the retained primary context");
    return VR_OK;
}

/* These adapters add observation and controlled retirement exclusion only.
   All source/job work remains
   the literal provider above, including its real closed partial-failure mode. */
static int te_job_retire(void *context) {
    struct te_fixture *fixture=atomic_load_explicit(&te_active,memory_order_acquire);
    if(!fixture||fixture->pid!=getpid()) return VR_FAULT;
    pthread_mutex_lock(&fixture->observation);
    /* Actual retirement exclusion, not an invented CUDA completion result.
       The original Job and stream stay untouched on every Busy retry. */
    if(pthread_mutex_trylock(&fixture->job_retirement)!=0) {
        atomic_fetch_add(&fixture->job_pending,1);
        pthread_mutex_unlock(&fixture->observation);
        return VR_BUSY;
    }
    int result=job_retire(context);
    pthread_mutex_unlock(&fixture->job_retirement);
    pthread_mutex_unlock(&fixture->observation);
    return result;
}
static int te_job_submit(void *context,uint64_t destination,uint64_t bytes) {
    struct te_fixture *fixture=atomic_load_explicit(&te_active,memory_order_acquire);
    if(!fixture||fixture->pid!=getpid()) return VR_FAULT;
    pthread_mutex_lock(&fixture->observation);
    int result=job_submit(context,destination,bytes);
    pthread_mutex_unlock(&fixture->observation);
    return result;
}
static void te_job_drop(void *context) {
    struct te_fixture *fixture=atomic_load_explicit(&te_active,memory_order_acquire);
    if(!fixture||fixture->pid!=getpid()) return;
    pthread_mutex_lock(&fixture->observation);
    job_drop(context);
    pthread_mutex_unlock(&fixture->observation);
}
static int te_prepare(void *context,int32_t device,uint64_t offset,uint64_t bytes,vr_source_job_v1 *out) {
    struct te_fixture *fixture=context;
    pthread_mutex_lock(&fixture->observation);
    int result=source_prepare(fixture->source,device,offset,bytes,out);
    if(result==VR_OK) {
        fixture->producer=out->streams[0];
        out->submit=te_job_submit;
        out->retire=te_job_retire;
        out->drop=te_job_drop;
    }
    pthread_mutex_unlock(&fixture->observation);
    return result;
}
static int te_source_retire(void *context) {
    struct te_fixture *fixture=context;
    pthread_mutex_lock(&fixture->observation);
    int result=source_retire(fixture->source);
    pthread_mutex_unlock(&fixture->observation);
    return result;
}
static void te_source_drop(void *context) {
    struct te_fixture *fixture=context;
    pthread_mutex_lock(&fixture->observation);
    source_drop(fixture->source);
    fixture->source=NULL;
    pthread_mutex_unlock(&fixture->observation);
}
static void te_reset_counts(void) {
    atomic_store(&counts.prepares,0);atomic_store(&counts.submits,0);
    atomic_store(&counts.job_retired,0);atomic_store(&counts.job_dropped,0);
    atomic_store(&counts.source_retired,0);atomic_store(&counts.source_dropped,0);
    atomic_store(&counts.source_pending,0);
}
static int te_write_gate(struct te_fixture *fixture,unsigned index,uint32_t value) {
    if(value>1) return te_fail(VR_INVALID,"gate value must be zero or one");
    TE_CUDA(cuda.write(fixture->control,fixture->gates[index].device,value,0));
    return te_cuda(cuda.stream_sync(fixture->control),"fixture control stream synchronization");
}

int te_create(vr_handle gpu,uint64_t bytes,int32_t mode,void **out_fixture,vr_handle *out_region) {
    if(!out_fixture||!out_region) return te_fail(VR_INVALID,"null fixture outputs");
    *out_fixture=NULL;*out_region=0;
    if(mode<0||mode>2||!bytes) return te_fail(VR_INVALID,"invalid fixture mode/length");
    /* No inherited mutex may be entered by a forked child. */
    struct te_fixture *existing=atomic_load_explicit(&te_active,memory_order_acquire);
    if(existing&&existing->pid!=getpid()) return te_fail(VR_INVALID,"fixture inherited after fork");
    pthread_mutex_lock(&te_guard);
    int result=VR_OK;
    if(atomic_load(&te_active)) {result=te_fail(VR_BUSY,"one Source fixture per process");goto finish;}
    result=te_initialize();if(result)goto finish;
    struct te_fixture *fixture=calloc(1,sizeof(*fixture));
    if(!fixture){result=te_fail(VR_SOURCE_ERROR,"cannot allocate Source test fixture");goto finish;}
    fixture->pid=getpid();pthread_mutex_init(&fixture->observation,NULL);
    pthread_mutex_init(&fixture->job_retirement,NULL);
    atomic_init(&fixture->job_pending,0);
    CU current=NULL;
    result=te_cuda(te_context.current(&current),"cuCtxGetCurrent");
    if(result||!current){if(!result)result=te_fail(VR_INVALID,"install Varena and select the explicit Torch device first");goto failed;}
    result=te_cuda(te_context.device(&fixture->device),"cuCtxGetDevice");if(result)goto failed;
    result=te_cuda(te_context.retain(&fixture->context,fixture->device),"cuDevicePrimaryCtxRetain");if(result)goto failed;
    if(current!=fixture->context){result=te_fail(VR_INVALID,"current CUDA context is not the retained primary context");goto failed;}
    /* The current primary context is already held on the calling thread here. */
    result=te_cuda(cuda.stream_create(&fixture->control,1),"fixture control stream create");if(result)goto failed;
    for(unsigned index=0;index<2;++index) {
        result=te_cuda(cuda.host_allocate(&fixture->gates[index].host,4,3),"fixture mapped HOST gate allocation");if(result)goto failed;
        result=te_cuda(cuda.host_device(&fixture->gates[index].device,fixture->gates[index].host,0),"fixture mapped HOST gate address");if(result)goto failed;
        result=te_write_gate(fixture,index,0);if(result)goto failed;
    }
    fixture->source=calloc(1,sizeof(*fixture->source));
    if(!fixture->source){result=te_fail(VR_SOURCE_ERROR,"cannot allocate existing Source provider");goto failed;}
    fixture->source->gate=fixture->gates[0].device;
    atomic_init(&fixture->source->partial,mode==1);
    fixture->source->fail_prepare=mode==2;
    pthread_mutex_init(&fixture->source->borrowed,NULL);
    te_reset_counts();
    atomic_store_explicit(&te_active,fixture,memory_order_release);
    vr_source_provider_v1 provider={te_prepare,te_source_retire,te_source_drop,fixture};
    vr_handle region=0;
    result=vr_v1_region_from_source(gpu,bytes,&provider,sizeof(provider),&region);
    if(result) {
        te_fail(result,vr_v1_last_error());
        atomic_store(&te_active,NULL);
        goto failed;
    }
    *out_fixture=fixture;*out_region=region;
    goto finish;
failed:
    /* Failed construction is a failed isolated proof child. Keep exact partial
       CUDA/context owners rooted until exit; do not infer successful cleanup or
       destroy an uncertain handle while reporting the original setup failure. */
    atomic_store_explicit(&te_active,fixture,memory_order_release);
finish:
    pthread_mutex_unlock(&te_guard);
    return result;
}

static int te_gate_set_index(struct te_fixture *fixture,unsigned index,uint32_t value) {
    int result=te_enter(fixture);if(result)return result;
    return te_leave(fixture,te_write_gate(fixture,index,value));
}
int te_gate_set(void *context,uint32_t value) {return te_gate_set_index(context,0,value);}
int te_extra_gate_set(void *context,uint32_t value) {return te_gate_set_index(context,1,value);}
static int te_gate_wait_index(struct te_fixture *fixture,unsigned index,uint64_t stream) {
    int result=te_enter(fixture);if(result)return result;
    result=te_stream_context(fixture,stream);
    if(!result) {
        size_t at=0;
        while(at<fixture->borrowed_count&&fixture->borrowed[at]!=(CU)(uintptr_t)stream)++at;
        if(at==fixture->borrowed_count) {
            if(at==sizeof(fixture->borrowed)/sizeof(fixture->borrowed[0]))result=te_fail(VR_INVALID,"too many fixture consumer streams");
            else fixture->borrowed[fixture->borrowed_count++]=(CU)(uintptr_t)stream;
        }
        if(!result)result=te_cuda(cuda.wait((CU)(uintptr_t)stream,fixture->gates[index].device,1,1),"fixture mapped HOST gate wait");
    }
    return te_leave(fixture,result);
}
int te_gate_wait(void *context,uint64_t stream) {return te_gate_wait_index(context,0,stream);}
int te_extra_gate_wait(void *context,uint64_t stream) {return te_gate_wait_index(context,1,stream);}
static int te_retirement_hold(struct te_fixture *fixture,uint32_t hold,int source) {
    int result=te_require(fixture);if(result)return result;
    if(hold>1)return te_fail(VR_INVALID,"retirement hold must be zero or one");
    pthread_mutex_lock(&fixture->observation);
    int *held=source?&fixture->source_held:&fixture->job_held;
    pthread_t *owner=source?&fixture->source_hold_owner:&fixture->job_hold_owner;
    pthread_mutex_t *mutex=&fixture->job_retirement;
    if(source) {
        if(!fixture->source){result=te_fail(VR_INVALID,"Source already dropped");goto finish;}
        if(hold&&fixture->source->retired){result=te_fail(VR_INVALID,"Source already retired");goto finish;}
        mutex=&fixture->source->borrowed;
    }
    if(hold) {
        if(*held){result=te_fail(VR_INVALID,"retirement mutex already held");goto finish;}
        if(pthread_mutex_trylock(mutex)!=0){result=te_fail(VR_BUSY,"retirement mutex is busy");goto finish;}
        *owner=pthread_self();*held=1;
    } else {
        if(!*held||!pthread_equal(*owner,pthread_self())) {
            result=te_fail(VR_INVALID,"retirement mutex must be released by its holding thread");goto finish;
        }
        *held=0;pthread_mutex_unlock(mutex);
    }
finish:
    pthread_mutex_unlock(&fixture->observation);
    return result;
}
int te_job_retirement_hold(void *context,uint32_t hold) {return te_retirement_hold(context,hold,0);}
int te_source_retirement_hold(void *context,uint32_t hold) {return te_retirement_hold(context,hold,1);}
int te_job_retirement_pending(void *context,uint32_t *pending) {
    int result=te_require(context);if(result)return result;
    if(!pending)return te_fail(VR_INVALID,"null Job retirement Busy count");
    struct te_fixture *fixture=context;
    *pending=atomic_load(&fixture->job_pending);
    return VR_OK;
}
int te_stream_pending(void *context,uint64_t stream,int32_t *pending) {
    if(!pending)return te_fail(VR_INVALID,"null pending output");
    *pending=0;struct te_fixture *fixture=context;
    int result=te_enter(fixture);if(result)return result;
    result=te_stream_context(fixture,stream);
    if(!result){int status=cuda.stream_query((CU)(uintptr_t)stream);if(status==600)*pending=1;else result=te_cuda(status,"cuStreamQuery");}
    return te_leave(fixture,result);
}
int te_source_pending(void *context,int32_t *pending) {
    if(!pending)return te_fail(VR_INVALID,"null producer pending output");
    *pending=0;struct te_fixture *fixture=context;
    int result=te_enter(fixture);if(result)return result;
    pthread_mutex_lock(&fixture->observation);
    /* te_job_retire takes this same observation mutex before destroying the
       remembered stream. Never query a retired/dropped CUDA handle. */
    if(fixture->producer&&!atomic_load(&counts.job_retired)&&!atomic_load(&counts.job_dropped)) {
        int status=cuda.stream_query((CU)(uintptr_t)fixture->producer);
        if(status==600)*pending=1;else result=te_cuda(status,"producer cuStreamQuery");
    }
    pthread_mutex_unlock(&fixture->observation);
    return te_leave(fixture,result);
}
int te_counts(void *context,struct te_counts_v1 *out,size_t size) {
    int result=te_require(context);if(result)return result;
    if(!out||size!=sizeof(*out))return te_fail(VR_INVALID,"fixture counters layout mismatch");
    struct te_fixture *fixture=context;
    pthread_mutex_lock(&fixture->observation);
    *out=(struct te_counts_v1){atomic_load(&counts.prepares),atomic_load(&counts.submits),
        atomic_load(&counts.job_retired),atomic_load(&counts.job_dropped),
        atomic_load(&counts.source_retired),atomic_load(&counts.source_dropped),
        atomic_load(&counts.source_pending)};
    pthread_mutex_unlock(&fixture->observation);
    return VR_OK;
}
int te_close(void *context) {
    struct te_fixture *fixture=context;
    int result=te_require(fixture);if(result)return result;
    pthread_mutex_lock(&te_guard);
    pthread_mutex_lock(&fixture->observation);
    int retained=fixture->job_held||fixture->source_held||fixture->source!=NULL||atomic_load(&counts.job_retired)!=atomic_load(&counts.job_dropped);
    pthread_mutex_unlock(&fixture->observation);
    if(retained){result=te_fail(VR_BUSY,"native Source/jobs still retain the fixture gates and code");goto finish;}
    result=te_enter(fixture);if(result)goto finish;
    for(size_t index=0;index<fixture->borrowed_count;++index) {
        int status=cuda.stream_query(fixture->borrowed[index]);
        if(status==600){result=te_fail(VR_BUSY,"a fixture-gated consumer is still pending");break;}
        result=te_cuda(status,"fixture consumer cuStreamQuery");if(result)break;
    }
    for(unsigned index=0;!result&&index<2;++index)if(fixture->gates[index].host) {
        result=te_cuda(cuda.host_free(fixture->gates[index].host),"fixture mapped HOST gate free");
        if(!result)fixture->gates[index].host=NULL;
    }
    if(!result&&fixture->control){result=te_cuda(cuda.stream_destroy(fixture->control),"fixture control stream destroy");if(!result)fixture->control=NULL;}
    result=te_leave(fixture,result);if(result)goto finish;
    result=te_cuda(te_context.release(fixture->device),"cuDevicePrimaryCtxRelease_v2");if(result)goto finish;
    atomic_store_explicit(&te_active,NULL,memory_order_release);
    pthread_mutex_destroy(&fixture->job_retirement);
    pthread_mutex_destroy(&fixture->observation);free(fixture);
finish:
    pthread_mutex_unlock(&te_guard);
    return result;
}
