//! Actual event reuse in mixed-live eager backing, with no graph pool or cache owner.
//! It diagnoses a mechanism; it does not attribute a particular LM event to a block.
use super::*;

fn snapshot(device: i32) -> Result<Value> {
    let s = working::stats(device)?;
    Ok(json!({"allocated":s.allocated_bytes,"capacity":s.allocated_capacity,
        "mapped":s.mapped_bytes,"pending":s.pending_bytes,"free":s.free_bytes,
        "reclaimable":s.reclaimable_bytes,"events":s.event_handles,"pools":s.pools,
        "pool_bytes":s.pools_bytes,"caches":s.caches,"driver_memory_calls":s.driver_memory_calls}))
}

pub(super) fn run(cuda: &Cuda, device: i32) -> Result<Value> {
    let control = probes::Stream::new(cuda)?;
    let anchor = working::alloc(device, 512, control.ptr as u64, None)?;
    let set = unsafe { *cuda._library.get::<unsafe extern "C" fn(u64,u8,usize,Ctx)->i32>(b"cuMemsetD8Async\0")? };
    check("anchor fill", unsafe { set(anchor, 47, 512, control.ptr) })?;
    control.finish()?;
    let baseline = snapshot(device)?;
    require(baseline["pools"]==0 && baseline["caches"]==0,"event diagnostic must have no pool/cache events")?;
    let streams = (0..18).map(|_| probes::Stream::new(cuda)).collect::<Result<Vec<_>>>()?;
    let mut rows = Vec::new();
    let mut previous = None;
    for (index, stream) in streams.iter().enumerate() {
        let pointer = working::alloc(device,512,stream.ptr as u64,None)?;
        require(pointer != anchor,"temporary allocation aliased live anchor")?;
        if let Some(old) = previous { require(pointer==old,"diagnostic did not reuse the same eager block")?; }
        check("temporary GPU fill", unsafe { set(pointer, index as u8, 512, stream.ptr) })?;
        working::free(device,pointer,stream.ptr as u64)?;
        stream.finish()?; // All accesses complete; the old freed pointer is not read.
        working::trim(device)?;
        let observed = snapshot(device)?;
        require(observed["allocated"]==baseline["allocated"] && observed["capacity"]==baseline["capacity"]
            && observed["mapped"]==baseline["mapped"] && observed["pending"]==0
            && observed["reclaimable"]==0 && observed["pools"]==0 && observed["caches"]==0,
            "event history changed physical/live baseline")?;
        rows.push(json!({"index":index,"stream":stream.ptr as u64,"same_owned_block":pointer,"working":observed}));
        previous=Some(pointer);
    }
    let retained = snapshot(device)?;
    working::trim(device)?;
    let after_second_trim = snapshot(device)?;
    require(after_second_trim==retained,"second completed trim changed the diagnostic state")?;
    // These fixture-owned streams have no future submitter or retained graph. Retire
    // them only after their real completion, preserving the still-live anchor stream.
    for stream in &streams { stream.finish()?; working::retire_stream(device,stream.ptr as u64)?; }
    let after_stream_retirement = snapshot(device)?;
    require(after_stream_retirement["allocated"]==baseline["allocated"]
        && after_stream_retirement["mapped"]==baseline["mapped"]
        && after_stream_retirement["events"]==baseline["events"],
        "retiring only completed temporary streams failed to remove their event ownership")?;
    // Full live-anchor readback after all metadata transitions. No freed temporary is used.
    control.finish()?;
    let mut bytes=[0u8;512];
    check("live anchor readback",unsafe { (cuda.read)(bytes.as_mut_ptr().cast(),anchor,bytes.len()) })?;
    require(bytes.iter().all(|b|*b==47),"live anchor changed")?;
    working::free(device,anchor,control.ptr as u64)?;
    control.finish()?;
    working::retire_stream(device,control.ptr as u64)?;
    working::trim(device)?;
    let final_stats=snapshot(device)?;
    require(final_stats["allocated"]==0 && final_stats["mapped"]==0 && final_stats["events"]==0,
        "event diagnostic failed its final physical/event baseline")?;
    Ok(json!({"baseline":baseline,"per_stream":rows,"retained_completed":retained,
        "after_second_trim":after_second_trim,"after_owned_stream_retirement":after_stream_retirement,
        "final":final_stats,"full_live_anchor_readback":true,
        "scope":"mixed-live eager-block event-history mechanism; no pools/caches; not attribution of the original LM18 events"}))
}
