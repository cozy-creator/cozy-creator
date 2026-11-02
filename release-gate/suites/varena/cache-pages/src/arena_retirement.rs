//! Real observer detach, genuine-owner retention and event-scoped native retirement.
use super::*;
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};
use varena::{Arena, ArenaRetirement, ArenaRetirementState, CacheOwner};

fn until<T>(mut operation: impl FnMut() -> Result<Option<T>>) -> Result<T> {
    let end = Instant::now() + Duration::from_secs(10);
    loop {
        if let Some(value) = operation()? {
            return Ok(value);
        }
        require(
            Instant::now() < end,
            "retirement observation deadline; no completion inferred",
        )?;
        std::thread::sleep(Duration::from_millis(2));
    }
}

pub(super) fn request(arena: Arc<Arena>) -> Result<ArenaRetirement> {
    match arena.request_retirement() {
        Ok(ticket) => Ok(ticket),
        Err((arena, error)) => {
            let _ = emit(
                &json!({"event":"retirement_handoff_failed","arena":arena.id(),"error":error.to_string(),
                "boundary":"input owner retained; failed helper ends its own process"}),
            );
            std::mem::forget(arena);
            Err(error.into())
        }
    }
}

fn callback_observer(device: i32) -> Result<CacheOwner> {
    let calls = Arc::new(AtomicUsize::new(0));
    let seen = calls.clone();
    let owner = CacheOwner::new(device, move |_| {
        seen.fetch_add(1, Ordering::Release);
        Ok(varena::cache_owner::CallbackResult::Declined)
    })?;
    let key = varena::cache_owner::RequestKey {
        cause: varena::cache_owner::Cause::Cooperative,
        generation: varena::cache_owner::next_generation()?,
    };
    let request = varena::cache_owner::request_shrink(device, key, 1)?
        .ok_or_else(|| io::Error::other("missing real cache observer"))?;
    until(|| {
        let facts = owner.facts()?;
        Ok(
            (calls.load(Ordering::Acquire) == 1 && facts.returned == 1 && !facts.running)
                .then_some(()),
        )
    })?;
    request.cancel()?; // This fixture asks only for observation, never donated-byte credit.
    Ok(owner)
}

struct Event<'a> {
    _cuda: &'a Cuda,
    value: Ctx,
    record: unsafe extern "C" fn(Ctx, Ctx) -> i32,
    query: unsafe extern "C" fn(Ctx) -> i32,
    destroy: unsafe extern "C" fn(Ctx) -> i32,
}
impl<'a> Event<'a> {
    fn new(cuda: &'a Cuda) -> Result<Self> {
        let create = unsafe {
            *cuda
                ._library
                .get::<unsafe extern "C" fn(*mut Ctx, u32) -> i32>(b"cuEventCreate\0")?
        };
        let mut value = std::ptr::null_mut();
        check("cuEventCreate", unsafe { create(&mut value, 2) })?;
        Ok(Self {
            _cuda: cuda,
            value,
            record: unsafe { *cuda._library.get(b"cuEventRecord\0")? },
            query: unsafe { *cuda._library.get(b"cuEventQuery\0")? },
            destroy: unsafe { *cuda._library.get(b"cuEventDestroy_v2\0")? },
        })
    }
    fn record(&self, stream: Ctx) -> Result<()> {
        check("cuEventRecord", unsafe {
            (self.record)(self.value, stream)
        })
    }
    fn completed(&self) -> Result<bool> {
        let status = unsafe { (self.query)(self.value) };
        if status == 600 {
            return Ok(false);
        }
        check("cuEventQuery", status)?;
        Ok(true)
    }
}
impl Drop for Event<'_> {
    fn drop(&mut self) {
        unsafe {
            (self.destroy)(self.value);
        }
    }
}

pub(super) fn observers(
    cuda: &Cuda,
    peer: &cooperative::Peer,
    device: i32,
    directory: &str,
    uuid: &str,
) -> Result<Value> {
    observers_inner(cuda, peer, device, directory, uuid, None)
}

fn observers_inner(
    cuda: &Cuda,
    peer: &cooperative::Peer,
    device: i32,
    directory: &str,
    uuid: &str,
    during_pending: Option<Box<dyn FnOnce(usize) -> Result<()>>>,
) -> Result<Value> {
    let a = Arena::new(device)?;
    let b = Arena::new(device)?;
    let page = a.granularity();
    let (ra, pa) = probes::filled(&a, page, 37)?;
    let (rb, pb) = probes::filled(&b, page, 53)?;
    cuda.synchronize()?; // Explicit fixture preparation, before the pending consumer.
    let mut consumer = probes::Stream::new(cuda)?;
    let active = probes::Stream::new(cuda)?;
    let event = Event::new(cuda)?;
    let warm = working::alloc(device, page, active.ptr as u64, None)?;
    cuda.fill(warm, page, 11)?;
    cuda.verify(&Block {
        ptr: warm,
        bytes: page,
        value: 11,
    })?;
    let callbacks = callback_observer(device)?;
    peer.publish(0)?; // Both real persistent observer stores now hold B.
    unsafe {
        working::free_completed(device, warm)?;
    }
    let before_calls = peer.cuda_calls()?;
    let b_id = b.id();
    let transient = b.clone();
    let held_plan = varena::plan::Plan::new(a.clone(), vec![("held-b".into(), vec![rb.clone()])])?;
    let plan = varena::plan::Plan::new(a.clone(), vec![("active-a".into(), vec![ra.clone()])])?;
    plan.activate("active-a")?;
    let abandoned_ticket = request(b.clone())?;
    require(
        abandoned_ticket.facts()?.arena_id == b_id,
        "request ticket identity",
    )?;
    drop(abandoned_ticket); // Native custody cannot depend on this host handle.
    let ticket = request(b)?; // Coalesces; must not introduce a second permanent Arena root.
    until(|| Ok(ticket.facts()?.observers_detached.then_some(())))?;
    require(
        !ticket.try_complete()?,
        "retirement revoked genuine Region/Pin/Plan owners",
    )?;
    let fresh_dir = std::path::Path::new(directory).join("fresh-observer");
    use std::os::unix::fs::DirBuilderExt;
    std::fs::DirBuilder::new().mode(0o700).create(&fresh_dir)?;
    let fresh = cooperative::Peer::open(
        device,
        ledger::Device::parse(uuid)?,
        &fresh_dir,
        ledger::Sharing::Dedicated,
        0,
    )?;
    let alive = fresh.publish(0)?;
    require(
        alive.physical.weights.live >= 2 * page as u64,
        "new observer omitted requested live backing",
    )?;

    consumer.block()?; // Real submitted work; its event cannot complete until finish().
    drop(unsafe { pb.into_fenced_region(&[consumer.ptr as u64]) });
    drop(rb);
    drop(held_plan);
    require(
        !ticket.try_complete()?,
        "retirement revoked a transient admission owner",
    )?;
    drop(transient);
    let pending = until(|| {
        let facts = ticket.facts()?;
        require(
            facts.state == ArenaRetirementState::Pending,
            "pending event was treated as failure/completion",
        )?;
        let reading = match peer.publish(0) {
            Ok(reading) => reading,
            Err(cooperative::Error::Native(varena::Error::Busy | varena::Error::CaptureActive)) => {
                return Ok(None);
            }
            Err(error) => return Err(error.into()),
        };
        Ok((reading.physical.weights.pending >= page as u64).then_some((facts, reading)))
    })?;
    require(
        pending.0.remaining_physical_bytes == page
            && pending.1.reclaimable == pending.1.physical.working.idle
            && pending.1.pending_release == 0,
        "queued finalization was credited as a donor release",
    )?;
    plan.activate("active-a")?; // A known Finalizing B must not force Busy for all plans.
    let choice = working::split_select(
        device,
        "retiring-b-live-a",
        vec![("one".into(), vec![page])],
        "one",
        active.ptr as u64,
    )?;
    require(
        choice.phase_weight_bytes == page,
        "live Plan lost its real A region",
    )?;
    let output = working::alloc(device, page, active.ptr as u64, None)?; // Actual reuse still acquires native exclusion.
    let memset = unsafe {
        *cuda
            ._library
            .get::<unsafe extern "C" fn(u64, u8, usize, Ctx) -> i32>(b"cuMemsetD8Async\0")?
    };
    check("cuMemsetD8Async", unsafe {
        memset(output, 71, page, active.ptr)
    })?;
    event.record(active.ptr)?;
    until(|| Ok(event.completed()?.then_some(())))?;
    require(
        !ticket.try_complete()? && ticket.facts()?.remaining_physical_bytes == page,
        "B finished before its real consumer event",
    )?;
    let during = fresh.publish(0)?;
    require(
        during.physical.weights.pending >= page as u64,
        "Finalizing physical bytes disappeared",
    )?;
    if let Some(during_pending) = during_pending {
        during_pending(ticket.facts()?.remaining_physical_bytes)?;
    }
    consumer.finish()?;
    until(|| Ok(ticket.try_complete()?.then_some(())))?;
    let completed = ticket.facts()?;
    require(
        completed.remaining_physical_bytes == 0 && callbacks.facts()?.registered,
        "retirement required ending an unrelated cache owner",
    )?;
    let mut bytes = vec![0u8; page];
    check("cuMemcpyDtoH", unsafe {
        (cuda.read)(bytes.as_mut_ptr().cast(), output, page)
    })?;
    require(
        bytes.iter().all(|byte| *byte == 71),
        "A output changed while B retired",
    )?;
    unsafe {
        working::free_completed(device, output)?;
    }
    working::trim(device)?;
    require(
        peer.cuda_calls()? >= before_calls,
        "observer detach decreased historical native counters",
    )?;
    let after = peer.publish(0)?;
    require(
        after.physical.weights.live == page as u64 && after.physical.weights.pending == 0,
        "actual B release not reflected in physical accounting",
    )?;
    plan.deactivate()?;
    drop(plan);
    drop(unsafe { pa.into_completed_region() });
    drop(ra);
    let a_ticket = request(a)?;
    until(|| Ok(a_ticket.try_complete()?.then_some(())))?;
    require(
        peer.publish(0)?.physical.total()? == 0,
        "retirement physical baseline",
    )?;
    callbacks.close()?;
    if let Err((fresh, error)) = fresh.try_close() {
        std::mem::forget(fresh);
        return Err(error.into());
    }
    Ok(
        json!({"arena":b_id,"page":page,"both_observer_stores_live":true,"dropped_ticket_and_duplicate_request_retained_custody":true,
        "genuine_region_pin_plan_retained":true,"transient_owner_retained":true,
        "new_observer_accounted_without_reacquiring_root":true,"live_a_completed_while_b_pending":true,
        "remaining_pending_bytes":pending.0.remaining_physical_bytes,"actual_released_bytes":page,
        "a_full_readback_bytes":page,"driver_history_monotone":true,"physical_baseline_restored":true,
        "completion":"recorded consumer event; no native context/device synchronization"}),
    )
}

pub(super) fn unproven(cuda: &Cuda, peer: &cooperative::Peer, device: i32) -> Result<Value> {
    let arena = Arena::new(device)?;
    let page = arena.granularity();
    let (region, pin) = probes::filled(&arena, page, 89)?;
    cuda.synchronize()?;
    peer.publish(0)?;
    let ticket = request(arena)?;
    drop(pin); // Deliberately supplies no completion contract: never repair it by guessing.
    drop(region);
    let facts = until(|| {
        let facts = ticket.facts()?;
        Ok((facts.state != ArenaRetirementState::Pending).then_some(facts))
    })?;
    require(
        facts.state == ArenaRetirementState::Unproven && facts.remaining_physical_bytes == page,
        "unproven backing was freed or silently joined",
    )?;
    let before = peer.cuda_calls()?;
    for _ in 0..100 {
        require(
            ticket.try_complete().is_err(),
            "unproven ticket became complete",
        )?;
    }
    require(
        peer.cuda_calls()? == before,
        "quarantined ticket polls performed CUDA work",
    )?;
    let reading = peer.publish(0)?;
    require(
        reading.physical.weights.pending == page as u64
            && reading.reclaimable == 0
            && reading.pending_release == 0,
        "unproven backing was credited",
    )?;
    drop(ticket); // Native quarantine retains ownership, independently of the observer handle.
    require(
        varena::resident_bytes(device)? == page,
        "dropping failed ticket lost retained accounting",
    )?;
    Ok(
        json!({"page":page,"state":"Unproven","remaining_physical_bytes":page,
        "failure":facts.failure,"polls_without_cuda":100,"ticket_drop_retained_backing":true,
        "boundary":"isolated destructive-last refusal cell; retains backing until process exit, not cleanup qualification"}),
    )
}

pub(super) fn cross_device(
    cuda: &Cuda,
    peer: &cooperative::Peer,
    device: i32,
    directory: &str,
    other_uuid: &str,
) -> Result<Value> {
    use std::sync::mpsc;
    require(
        device == 0,
        "cross-device helper expects ordinary device zero lane",
    )?;
    let arena = Arena::new(device)?;
    let page = arena.granularity();
    let (region, pin) = probes::filled(&arena, page, 101)?;
    let plan = varena::plan::Plan::new(
        arena.clone(),
        vec![("device-zero".into(), vec![region.clone()])],
    )?;
    plan.activate("device-zero")?;
    let stream = probes::Stream::new(cuda)?;
    let event = Event::new(cuda)?;
    let warm = working::alloc(device, page, stream.ptr as u64, None)?;
    cuda.fill(warm, page, 103)?;
    cuda.verify(&Block {
        ptr: warm,
        bytes: page,
        value: 103,
    })?;
    unsafe {
        working::free_completed(device, warm)?;
    }
    let path = std::path::Path::new(directory).join("device-one");
    use std::os::unix::fs::DirBuilderExt;
    std::fs::DirBuilder::new().mode(0o700).create(&path)?;
    let uuid = other_uuid.to_owned();
    let (ready, waiting) = mpsc::sync_channel(1);
    let (resume, released) = mpsc::sync_channel(1);
    let other = std::thread::spawn(move || -> std::result::Result<Value, String> {
        (|| -> Result<Value> {
            let cuda = Cuda::open(1)?;
            let peer = cooperative::Peer::open(
                1,
                ledger::Device::parse(&uuid)?,
                &path,
                ledger::Sharing::Dedicated,
                0,
            )?;
            let gate = Box::new(move |remaining| -> Result<()> {
                ready.send(remaining).map_err(io::Error::other)?;
                released
                    .recv_timeout(Duration::from_secs(15))
                    .map_err(io::Error::other)?;
                Ok(())
            });
            let report = observers_inner(
                &cuda,
                &peer,
                1,
                path.to_str()
                    .ok_or_else(|| io::Error::other("fixture path UTF8"))?,
                &uuid,
                Some(gate),
            )?;
            if let Err((peer, error)) = peer.try_close() {
                std::mem::forget(peer);
                return Err(error.into());
            }
            Ok(report)
        })()
        .map_err(|error| error.to_string())
    });
    let pending = waiting
        .recv_timeout(Duration::from_secs(15))
        .map_err(io::Error::other)?;
    require(
        pending != 0,
        "other device was not retaining real pending backing",
    )?;
    // These are native allocator/capture gates on device zero while device one's
    // checked consumer event cannot finish. No context/device synchronization here.
    plan.activate("device-zero")?;
    let output = working::alloc(device, page, stream.ptr as u64, None)?;
    let memset = unsafe {
        *cuda
            ._library
            .get::<unsafe extern "C" fn(u64, u8, usize, Ctx) -> i32>(b"cuMemsetD8Async\0")?
    };
    check("cuMemsetD8Async", unsafe {
        memset(output, 107, page, stream.ptr)
    })?;
    event.record(stream.ptr)?;
    until(|| Ok(event.completed()?.then_some(())))?;
    let capture = varena::enter_capture(device)?;
    let begin = unsafe {
        *cuda
            ._library
            .get::<unsafe extern "C" fn(Ctx, i32) -> i32>(b"cuStreamBeginCapture_v2\0")?
    };
    let end = unsafe {
        *cuda
            ._library
            .get::<unsafe extern "C" fn(Ctx, *mut Ctx) -> i32>(b"cuStreamEndCapture\0")?
    };
    let destroy = unsafe {
        *cuda
            ._library
            .get::<unsafe extern "C" fn(Ctx) -> i32>(b"cuGraphDestroy\0")?
    };
    check("cuStreamBeginCapture", unsafe { begin(stream.ptr, 0) })?;
    let mut graph = std::ptr::null_mut();
    let ended = unsafe { end(stream.ptr, &mut graph) };
    if ended != 0 {
        let _ = emit(&json!({"event":"capture_end_failed","status":ended}));
        std::process::exit(3);
    }
    drop(capture);
    if !graph.is_null() {
        check("cuGraphDestroy", unsafe { destroy(graph) })?;
    }
    resume.send(()).map_err(io::Error::other)?;
    let report = other
        .join()
        .map_err(|_| io::Error::other("other-device native proof panicked"))?
        .map_err(io::Error::other)?;
    let mut bytes = vec![0u8; page];
    check("cuMemcpyDtoH", unsafe {
        (cuda.read)(bytes.as_mut_ptr().cast(), output, page)
    })?;
    require(
        bytes.iter().all(|byte| *byte == 107),
        "device-zero progress output changed",
    )?;
    unsafe {
        working::free_completed(device, output)?;
    }
    working::trim(device)?;
    plan.deactivate()?;
    drop(plan);
    drop(unsafe { pin.into_completed_region() });
    drop(region);
    let ticket = request(arena)?;
    until(|| Ok(ticket.try_complete()?.then_some(())))?;
    require(
        peer.publish(0)?.physical.total()? == 0,
        "device-zero final accounting",
    )?;
    Ok(
        json!({"other_device":1,"other_uuid":other_uuid,"pending_other_bytes":pending,
        "native_allocation_progress_before_other_completion":true,"actual_capture_before_other_completion":true,
        "full_readback_bytes":page,"other_device_result":report}),
    )
}
