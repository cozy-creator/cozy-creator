//! Explicit trim releases idle eager event metadata, never live/pending/captured custody.
use super::*;

type Copy = unsafe extern "C" fn(u64, u64, usize, Ctx) -> i32;
type Begin = unsafe extern "C" fn(Ctx, i32) -> i32;
type End = unsafe extern "C" fn(Ctx, *mut Ctx) -> i32;
type ObjectOp = unsafe extern "C" fn(Ctx) -> i32;
type EventNew = unsafe extern "C" fn(*mut Ctx, u32) -> i32;
type Record = unsafe extern "C" fn(Ctx, Ctx) -> i32;

fn snapshot(device: i32) -> Result<Value> {
    let s = working::stats(device)?;
    Ok(json!({"live":s.allocated_bytes,"capacity":s.allocated_capacity,
        "mapped":s.mapped_bytes,"pending":s.pending_bytes,"events":s.event_handles,
        "pools":s.pools,"caches":s.caches,"memory_calls":s.driver_memory_calls,
        "event_waits":s.event_waits,"context_joins":s.context_joins}))
}
fn read(cuda: &Cuda, pointer: u64, bytes: usize, value: u8) -> Result<()> {
    let mut output = vec![0u8; bytes];
    check("owned output readback", unsafe { (cuda.read)(output.as_mut_ptr().cast(),pointer,bytes) })?;
    require(output.iter().all(|byte| *byte==value),"full still-owned readback differs")
}
fn capture(cuda: &Cuda, peer: &cooperative::Peer, device: i32) -> Result<Value> {
    let stream=probes::Stream::new(cuda)?;
    let (begin,end,destroy,new_event,record,drop_event)=unsafe {
        (*cuda._library.get::<Begin>(b"cuStreamBeginCapture_v2\0")?,
         *cuda._library.get::<End>(b"cuStreamEndCapture\0")?,
         *cuda._library.get::<ObjectOp>(b"cuGraphDestroy\0")?,
         *cuda._library.get::<EventNew>(b"cuEventCreate\0")?,
         *cuda._library.get::<Record>(b"cuEventRecord\0")?,
         *cuda._library.get::<ObjectOp>(b"cuEventDestroy_v2\0")?)
    };
    let mut event=std::ptr::null_mut();
    check("capture fixture event",unsafe {new_event(&mut event,2)})?;
    let guard=varena::enter_capture(device)?;
    check("real capture begin",unsafe {begin(stream.ptr,0)})?;
    let result=(|| {
        check("real capture event node",unsafe {record(event,stream.ptr)})?;
        let before=snapshot(device)?;let calls=peer.cuda_calls()?;
        require(working::trim(device)?==0,"trim reclaimed during capture")?;
        let after=snapshot(device)?;
        require(before==after && calls==peer.cuda_calls()?,"capture trim touched native event/driver state")?;
        Ok(json!({"before":before,"after":after,"native_calls":calls,"actual_cuda_capture":true}))
    })();
    let mut graph=std::ptr::null_mut();let code=unsafe {end(stream.ptr,&mut graph)};
    if code!=0 {
        let _=emit(&json!({"event":"result","passed":false,"capture_end_status":code,
            "scope":"no GPU-owning teardown after unproven capture end"}));
        unsafe {libc::_exit(3)};
    }
    drop(guard);
    if !graph.is_null() {check("capture fixture graph destroy",unsafe {destroy(graph)})?;}
    check("capture fixture event destroy",unsafe {drop_event(event)})?;
    result
}

pub(super) fn run(cuda: &Cuda, peer: &cooperative::Peer, device: i32) -> Result<Value> {
    let control=probes::Stream::new(cuda)?;
    let empty=snapshot(device)?;
    require(empty["mapped"]==0 && empty["live"]==0,"isolated eager setup must start empty")?;
    let anchor=working::alloc(device,512,control.ptr as u64,None)?;
    let slab=snapshot(device)?;
    let slab_bytes=slab["mapped"].as_u64().ok_or("missing slab mapping fact")?;
    require(slab_bytes>=1024 && slab_bytes%512==0,"unexpected native 512-byte slab layout")?;
    // take() uses untouched free slots before poll(). Keep all but one measured
    // slot live so each next stream must poll and reuse the historical block.
    let mut holders=vec![anchor];
    for _ in 0..slab_bytes/512-2 {
        holders.push(working::alloc(device,512,control.ptr as u64,None)?);
    }
    let occupied=snapshot(device)?;
    require(occupied["mapped"]==slab["mapped"] && occupied["memory_calls"]==slab["memory_calls"]
        && occupied["capacity"].as_u64()==Some(slab_bytes-512),"setup did not leave exactly one slab slot")?;
    let output=working::alloc(device,1024,control.ptr as u64,None)?;
    check("anchor GPU fill",unsafe {(cuda.set)(anchor,47,512,control.ptr)})?;control.finish()?;
    let baseline=snapshot(device)?;
    require(baseline["events"]==0 && baseline["pools"]==0 && baseline["caches"]==0,"isolated eager baseline")?;
    let streams=(0..18).map(|_|probes::Stream::new(cuda)).collect::<Result<Vec<_>>>()?;
    let mut rows=Vec::new();let mut prior=None;
    for (index,stream) in streams.iter().enumerate() {
        let pointer=working::alloc(device,512,stream.ptr as u64,None)?;
        require(pointer!=anchor,"temporary aliases live anchor")?;
        if let Some(previous)=prior {require(pointer==previous,"cross-stream poll failed to reuse same block")?;}
        check("temporary GPU fill",unsafe {(cuda.set)(pointer,index as u8,512,stream.ptr)})?;
        working::free(device,pointer,stream.ptr as u64)?;stream.finish()?;
        let observed=snapshot(device)?;
        require(observed["events"]==index+1,"ordinary alloc/free/poll lost cached event reuse")?;
        require(observed["mapped"]==baseline["mapped"] && observed["memory_calls"]==baseline["memory_calls"],"event construction remapped eager backing")?;
        rows.push(observed);prior=Some(pointer);
    }
    let cached=snapshot(device)?;
    // A new physical owner may reuse the block, but trim must leave Live metadata alone.
    let live=working::alloc(device,512,streams[0].ptr as u64,None)?;
    require(Some(live)==prior,"live exclusion did not own historical block")?;
    check("live temporary fill",unsafe {(cuda.set)(live,85,512,streams[0].ptr)})?;streams[0].finish()?;
    let live_before=snapshot(device)?;
    require(working::trim(device)?==0,"live metadata trim granted GPU bytes")?;
    require(snapshot(device)?==live_before,"explicit trim changed Live event ownership")?;
    read(cuda,live,512,85)?;
    working::free(device,live,streams[0].ptr as u64)?;streams[0].finish()?;

    let mut pending=probes::Stream::new(cuda)?;
    let waiting=working::alloc(device,1024,pending.ptr as u64,None)?;
    pending.block()?; // Real queued host gate, opened by an owned CPU atomic store.
    check("pending GPU fill",unsafe {(cuda.set)(waiting,73,1024,pending.ptr)})?;
    let copy=unsafe {*cuda._library.get::<Copy>(b"cuMemcpyDtoDAsync_v2\0")?};
    check("pending owned-output copy",unsafe {copy(output,waiting,1024,pending.ptr)})?;
    working::record_stream(device,output,pending.ptr as u64)?;
    working::free(device,waiting,pending.ptr as u64)?;
    let captured=capture(cuda,peer,device)?;
    let before_trim=snapshot(device)?;
    let released=working::trim(device)?;
    let pending_after=snapshot(device)?;
    require(released==0 && pending_after["events"]==1 && pending_after["pending"]==1024,
        "explicit trim failed completed cleanup or touched pending fence")?;
    for field in ["live","capacity","mapped","memory_calls","event_waits","context_joins"] {
        require(pending_after[field]==before_trim[field],"metadata trim changed physical/consumer accounting")?;
    }
    let query=unsafe {*cuda._library.get::<ObjectOp>(b"cuStreamQuery\0")?};
    require(unsafe {query(pending.ptr)}==600,"pending stream unexpectedly completed")?;
    require(working::trim(device)?==0 && snapshot(device)?==pending_after,"second trim changed pending custody")?;
    pending.finish()?;read(cuda,output,1024,73)?;
    require(working::trim(device)?==0,"pending completion metadata granted GPU bytes")?;
    let completed=snapshot(device)?;
    require(completed["events"]==0 && completed["pending"]==0 && completed["mapped"]==baseline["mapped"],"completed pending metadata not retired independently")?;
    let reused=working::alloc(device,512,streams[0].ptr as u64,None)?;
    require(Some(reused)==prior,"trim discarded reusable block")?;
    check("post-trim new owned fill",unsafe {(cuda.set)(reused,123,512,streams[0].ptr)})?;streams[0].finish()?;
    read(cuda,reused,512,123)?;working::free(device,reused,streams[0].ptr as u64)?;streams[0].finish()?;
    require(working::trim(device)?==0 && snapshot(device)?["events"]==0,"new free did not recreate/retire its event")?;
    read(cuda,anchor,512,47)?;
    for pointer in &holders {working::free(device,*pointer,control.ptr as u64)?;}
    working::free(device,output,control.ptr as u64)?;
    control.finish()?;
    for stream in &streams {stream.finish()?;working::retire_stream(device,stream.ptr as u64)?;}
    working::retire_stream(device,pending.ptr as u64)?;working::retire_stream(device,control.ptr as u64)?;
    working::trim(device)?;let final_stats=snapshot(device)?;
    require(final_stats["live"]==0 && final_stats["mapped"]==0 && final_stats["events"]==0
        && final_stats["event_waits"]==baseline["event_waits"] && final_stats["context_joins"]==baseline["context_joins"],"final physical/event baseline")?;
    Ok(json!({"baseline":baseline,"slab_bytes":slab_bytes,"live_slab_holders":holders.len(),
        "ordinary_cache_growth":rows,"cached":cached,
        "live_exclusion":live_before,"capture":captured,"before_explicit_trim":before_trim,
        "pending_after_trim":pending_after,"completed_after_trim":completed,"trim_returned_gpu_bytes":released,
        "full_live_anchor_and_pending_output_readback":true,"post_trim_new_owner_readback":true,
        "final":final_stats,"scope":"explicit eager Ready metadata cleanup; no pool/live/pending pruning or LM attribution"}))
}
