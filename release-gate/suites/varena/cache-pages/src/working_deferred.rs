//! Actual native Working/Driver cells; no Torch, simulated completion or allocator stand-in.
#![allow(unsafe_code)]
use super::*;
use std::sync::atomic::{AtomicBool, Ordering};

const BYTES: usize = 4 << 20;
type Create = unsafe extern "C" fn(*mut Ctx, u32) -> i32;
type LaunchHost = unsafe extern "C" fn(Ctx, unsafe extern "C" fn(*mut c_void), *mut c_void) -> i32;
type Copy = unsafe extern "C" fn(u64, u64, usize, Ctx) -> i32;

struct Gate {
    entered: AtomicBool,
    open: AtomicBool,
}
unsafe extern "C" fn blocked(data: *mut c_void) {
    // SAFETY: the stream owns this stable Box until checked completion; only atomics cross threads.
    let gate = unsafe { &*data.cast::<Gate>() };
    gate.entered.store(true, Ordering::Release);
    while !gate.open.load(Ordering::Acquire) {
        std::thread::park_timeout(Duration::from_millis(1));
    }
}

struct Stream<'a> {
    cuda: &'a Cuda,
    ptr: Ctx,
    gate: Option<Box<Gate>>,
    close_started: bool,
}
impl<'a> Stream<'a> {
    fn new(cuda: &'a Cuda) -> Result<Self> {
        // SAFETY: exact Driver ABI, retained Library and current primary context.
        let create = unsafe { *cuda._library.get::<Create>(b"cuStreamCreate\0")? };
        let mut ptr = std::ptr::null_mut();
        check("deferred nonblocking stream", unsafe {
            create(&mut ptr, 1)
        })?;
        Ok(Self {
            cuda,
            ptr,
            gate: None,
            close_started: false,
        })
    }
    fn raw(&self) -> u64 {
        self.ptr as u64
    }
    fn block(&mut self) -> Result<()> {
        require(self.gate.is_none(), "one submitted gate per owned stream")?;
        self.gate = Some(Box::new(Gate {
            entered: AtomicBool::new(false),
            open: AtomicBool::new(false),
        }));
        let gate = self.gate.as_deref().expect("owned gate");
        // SAFETY: callback does not call CUDA; userdata remains owned until this stream drains.
        let launch = unsafe {
            *self
                .cuda
                ._library
                .get::<LaunchHost>(b"cuLaunchHostFunc\0")?
        };
        check("deferred submitted gate", unsafe {
            launch(self.ptr, blocked, (gate as *const Gate).cast_mut().cast())
        })?;
        let deadline = Instant::now() + Duration::from_secs(5);
        while !gate.entered.load(Ordering::Acquire) {
            require(
                Instant::now() < deadline,
                "submitted CUDA gate did not enter within fixture protocol",
            )?;
            std::thread::sleep(Duration::from_millis(1));
        }
        Ok(())
    }
    fn set(&self, ptr: u64, value: u8) -> Result<()> {
        // SAFETY: callers retain the exact BYTES allocation or captured pool through this submission.
        check("deferred stream memset", unsafe {
            (self.cuda.set)(ptr, value, BYTES, self.ptr)
        })
    }
    fn copy(&self, dst: u64, src: u64) -> Result<()> {
        // SAFETY: both real BYTES allocations remain retained; all writes/reads are ordered here.
        let copy = unsafe { *self.cuda._library.get::<Copy>(b"cuMemcpyDtoDAsync_v2\0")? };
        check("deferred stream copy", unsafe {
            copy(dst, src, BYTES, self.ptr)
        })
    }
    fn finish(&self) -> Result<()> {
        if let Some(gate) = &self.gate {
            gate.open.store(true, Ordering::Release);
        }
        // SAFETY: the owned live stream is not destroyed until checked native retirement.
        check("deferred stream completion", unsafe {
            (self.cuda.sync)(self.ptr)
        })
    }
    fn close(&mut self) -> Result<()> {
        require(!self.close_started, "owned stream close attempted once")?;
        self.close_started = true;
        self.finish()?;
        working::retire_stream(self.cuda.device, self.raw())?;
        let ptr = std::mem::replace(&mut self.ptr, std::ptr::null_mut());
        // SAFETY: submissions stopped, actual stream drained, native references retired.
        check("destroy retired explicit stream", unsafe {
            (self.cuda.destroy)(ptr)
        })
    }

    fn close_public_borrowed(&mut self) -> Result<()> {
        require(!self.close_started, "borrowed stream close attempted once")?;
        self.close_started = true;
        self.finish()?;
        let ptr = std::mem::replace(&mut self.ptr, std::ptr::null_mut());
        // SAFETY: this cell completed public free/ABI retirement and dropped all Rust
        // stream/resource clones. Deliberately do not call private working::retire_stream.
        check("destroy publicly released borrowed stream", unsafe {
            (self.cuda.destroy)(ptr)
        })
    }
}
impl Drop for Stream<'_> {
    fn drop(&mut self) {
        if self.ptr.is_null() {
            return;
        }
        // Failed cells open their actual gate. Ambiguous explicit close is not retried.
        if !self.close_started
            && self.finish().is_ok()
            && working::retire_stream(self.cuda.device, self.raw()).is_ok()
        {
            // SAFETY: the fallback established the same completion/retirement boundary.
            unsafe {
                (self.cuda.destroy)(self.ptr);
            }
        } else if let Some(gate) = self.gate.take() {
            std::mem::forget(gate); // An unproven callback must not dereference freed userdata.
        }
    }
}

fn snapshot(device: i32) -> Result<Value> {
    let s = working::stats(device)?;
    Ok(
        json!({"live":s.allocated_bytes,"mapped":s.mapped_bytes,"pending":s.pending_bytes,
        "reclaimable":s.reclaimable_bytes,"records":s.event_records,"queries":s.event_queries,
        "waits":s.event_waits,"joins":s.context_joins,"memory_calls":s.driver_memory_calls}),
    )
}
fn driver_counters(device: i32) -> Result<(u64, u64, u64, u64, u64)> {
    let s = working::stats(device)?;
    Ok((
        s.driver_memory_calls,
        s.event_records,
        s.event_queries,
        s.event_waits,
        s.context_joins,
    ))
}
fn verify(cuda: &Cuda, ptr: u64, value: u8) -> Result<()> {
    cuda.verify(&Block {
        ptr,
        bytes: BYTES,
        value,
    })
}

pub(super) fn same_stream(cuda: &Cuda, peer: &cooperative::Peer, device: i32) -> Result<Value> {
    let mut stream = Stream::new(cuda)?;
    let ptr = working::alloc(device, BYTES, stream.raw(), None)?;
    let output = working::alloc(device, BYTES, stream.raw(), None)?;
    stream.block()?;
    let before = driver_counters(device)?;
    let activity_before = peer.cuda_calls()?;
    for value in 1..=64 {
        stream.set(ptr, value)?;
        working::free(device, ptr, stream.raw())?;
        let freed = working::stats(device)?;
        require(
            freed.pending_bytes >= BYTES && freed.reclaimable_bytes == 0,
            "ordered deferred backing stays charged and not globally reclaimable",
        )?;
        require(
            working::alloc(device, BYTES, stream.raw(), None)? == ptr,
            "same-stream ordered reuse retains the exact allocation",
        )?;
    }
    stream.copy(output, ptr)?;
    working::free(device, ptr, stream.raw())?;
    let after = driver_counters(device)?;
    let activity_after = peer.cuda_calls()?;
    let detail = json!({"iterations":64,"before":before,"after":after,
        "native_activity_before":activity_before,"native_activity_after":activity_after,
        "pending":snapshot(device)?});
    emit(&json!({"event":"deferred_same_stream_before_assertion","detail":detail}))?;
    require(
        before == after && activity_before == activity_after,
        "same-stream eager free/reuse made a native driver call",
    )?;
    stream.finish()?;
    verify(cuda, output, 64)?; // Copy was queued before releasing ptr, not a new read after free.
    working::free(device, output, stream.raw())?;
    stream.close()?;
    Ok(detail)
}

pub(super) fn cross_stream(
    cuda: &Cuda,
    peer: &cooperative::Peer,
    device: i32,
    additional: bool,
) -> Result<Value> {
    let mut producer = Stream::new(cuda)?;
    let mut consumer = Stream::new(cuda)?;
    let ptr = working::alloc(device, BYTES, producer.raw(), None)?;
    let output = working::alloc(device, BYTES, consumer.raw(), None)?;
    let spare = working::alloc(device, BYTES, consumer.raw(), None)?;
    // Supply completed alternate capacity before closing the gate: this cell does not
    // depend on whether a fresh driver allocation waits behind a CUDA host callback.
    working::free(device, spare, consumer.raw())?;
    consumer.finish()?;
    peer.publish(0)?;
    producer.set(ptr, 37)?;
    producer.finish()?;
    if additional {
        consumer.block()?;
        consumer.copy(output, ptr)?;
        working::record_stream(device, ptr, consumer.raw())?;
    } else {
        producer.block()?;
        producer.set(ptr, 37)?;
        producer.copy(output, ptr)?;
        working::record_stream(device, output, producer.raw())?;
    }
    let before = working::stats(device)?;
    working::free(device, ptr, producer.raw())?;
    let freed = working::stats(device)?;
    require(
        if additional {
            freed.event_records >= before.event_records + 2
        } else {
            freed.event_records == before.event_records
        },
        "multi-stream frees fence immediately; sole-stream free defers",
    )?;
    let other = working::alloc(device, BYTES, consumer.raw(), None)?;
    require(
        other == spare && other != ptr,
        "another stream reused pending producer backing",
    )?;
    let mapped = working::stats(device)?.mapped_bytes;
    let released = working::trim(device)?;
    let pending = snapshot(device)?;
    require(
        released == 0 && working::stats(device)?.mapped_bytes == mapped,
        "trim released an actual pending consumer mapping",
    )?;
    require(
        working::stats(device)?.event_records > before.event_records,
        "cross-stream/reclaim boundary did not seal a real event",
    )?;
    producer.finish()?;
    consumer.finish()?;
    verify(cuda, output, 37)?;
    working::free(device, other, consumer.raw())?;
    working::free(device, output, consumer.raw())?;
    producer.close()?;
    consumer.close()?;
    Ok(
        json!({"additional_consumer":additional,"pending":pending,"released_while_pending":released,
        "cross_stream_pointer_distinct":true,"queued_copy_all_bytes_correct":true}),
    )
}

pub(super) fn capture(cuda: &Cuda, device: i32) -> Result<Value> {
    type Begin = unsafe extern "C" fn(Ctx, i32) -> i32;
    type End = unsafe extern "C" fn(Ctx, *mut Ctx) -> i32;
    type Instantiate = unsafe extern "C" fn(*mut Ctx, Ctx, u64) -> i32;
    type Launch = unsafe extern "C" fn(Ctx, Ctx) -> i32;
    // SAFETY: exact Driver graph ABI; library, streams and pool remain owned through replay.
    let (begin, end, instantiate, launch, destroy_graph, destroy_exec) = unsafe {
        (
            *cuda._library.get::<Begin>(b"cuStreamBeginCapture_v2\0")?,
            *cuda._library.get::<End>(b"cuStreamEndCapture\0")?,
            *cuda
                ._library
                .get::<Instantiate>(b"cuGraphInstantiateWithFlags\0")?,
            *cuda._library.get::<Launch>(b"cuGraphLaunch\0")?,
            *cuda._library.get::<Sync>(b"cuGraphDestroy\0")?,
            *cuda._library.get::<Sync>(b"cuGraphExecDestroy\0")?,
        )
    };
    let mut eager = Stream::new(cuda)?;
    let mut captured = Stream::new(cuda)?;
    let ptr = working::alloc(device, BYTES, eager.raw(), None)?;
    let pool = (0x6465666572726564, 1);
    working::pool_reserve(device, pool, BYTES)?;
    working::pool_begin(device, pool, captured.raw())?;
    if let Err(error) = check("deferred actual begin capture", unsafe {
        begin(captured.ptr, 0)
    }) {
        working::pool_end(device, pool)?;
        working::pool_capture_complete(device, pool)?;
        working::pool_release(device, pool)?;
        return Err(error);
    }
    let observed = (|| -> Result<(u64, Value)> {
        let before = driver_counters(device)?;
        working::free(device, ptr, eager.raw())?;
        require(
            matches!(
                working::alloc(device, BYTES, eager.raw(), None),
                Err(varena::Error::CaptureActive)
            ),
            "non-pool free during capture became ordered eager capacity",
        )?;
        let first = working::alloc(device, BYTES, captured.raw(), Some(pool))?;
        captured.set(first, 71)?;
        working::free(device, first, captured.raw())?;
        let second = working::alloc(device, BYTES, captured.raw(), Some(pool))?;
        require(
            first == second,
            "prepared pool lost same-stream captured reuse",
        )?;
        captured.set(second, 83)?;
        working::free(device, second, captured.raw())?;
        require(
            working::trim(device)? == 0,
            "trim released backing during capture",
        )?;
        let after = driver_counters(device)?;
        require(
            before == after,
            "native free/reuse/trim made driver calls in capture",
        )?;
        Ok((
            second,
            json!({"before":before,"after":after,"pool_same_pointer":true}),
        ))
    })();
    working::pool_end(device, pool)?;
    let mut graph = std::ptr::null_mut();
    let ended = unsafe { end(captured.ptr, &mut graph) };
    if ended != 0 {
        let _ = emit(
            &json!({"event":"deferred_capture_end_failed","status":ended,"retirement_quarantined":true}),
        );
        // SAFETY: failed capture cannot unwind GPU-owning destructors as successful cleanup.
        unsafe {
            libc::_exit(3);
        }
    }
    working::pool_capture_complete(device, pool)?;
    let (ptr, detail) = observed?;
    let mut executable = std::ptr::null_mut();
    check("instantiate deferred actual graph", unsafe {
        instantiate(&mut executable, graph, 0)
    })?;
    check("launch deferred actual graph", unsafe {
        launch(executable, captured.ptr)
    })?;
    working::pool_record_stream(device, pool, captured.raw())?;
    captured.finish()?;
    verify(cuda, ptr, 83)?; // The live graph reference owns this pool's captured addresses.
    check("destroy deferred graph executable", unsafe {
        destroy_exec(executable)
    })?;
    check("destroy deferred graph definition", unsafe {
        destroy_graph(graph)
    })?;
    working::pool_release(device, pool)?;
    captured.close()?;
    eager.close()?;
    Ok(detail)
}

pub(super) fn donation(cuda: &Cuda, peer: &cooperative::Peer, device: i32) -> Result<Value> {
    let mut stream = Stream::new(cuda)?;
    let ptr = working::alloc(device, BYTES, stream.raw(), None)?;
    let output = working::alloc(device, BYTES, stream.raw(), None)?;
    stream.block()?;
    stream.set(ptr, 43)?;
    stream.copy(output, ptr)?;
    working::free(device, ptr, stream.raw())?;
    let unsealed = snapshot(device)?;
    require(
        unsealed["reclaimable"] == 0,
        "deferred bytes were promised as globally reclaimable",
    )?;
    let pending = peer.publish(0)?; // This real service boundary seals the pending stream tail.
    require(
        pending.reclaimable == 0 && pending.physical.working.pending >= BYTES as u64,
        "pending explicit-stream bytes became a donation offer",
    )?;
    let sample = peer.sample(1)?;
    peer.ask_device(sample.generation, BYTES as u64)?;
    let blocked = peer.service()?;
    require(
        blocked.released == 0 && working::trim(device)? == 0,
        "service/trim released pending explicit-stream bytes",
    )?;
    stream.finish()?;
    verify(cuda, output, 43)?;
    let completed = peer.publish(0)?;
    require(
        completed.reclaimable == BYTES as u64,
        "completed sole mapping missing from donation offer",
    )?;
    let sample = peer.sample(2)?;
    let round = peer.ask_device(sample.generation, BYTES as u64)?;
    let released = peer.service()?;
    require(
        released.released == BYTES as u64 && working::stats(device)?.mapped_bytes == BYTES,
        "donation did not release exactly the completed idle mapping",
    )?;
    peer.withdraw(round.generation)?;
    peer.clear_want()?;
    verify(cuda, output, 43)?;
    working::free(device, output, stream.raw())?;
    stream.close()?;
    Ok(
        json!({"unsealed":unsealed,"pending_released":blocked.released,
        "pending_physical":pending.physical.working.pending,"completed_released":released.released,
        "live_output_all_bytes_preserved":true}),
    )
}

pub(super) fn retire_destroy(cuda: &Cuda, device: i32) -> Result<Value> {
    let mut old = Stream::new(cuda)?;
    let old_handle = old.raw();
    let live = working::alloc(device, BYTES, old_handle, None)?;
    let freed = working::alloc(device, BYTES, old_handle, None)?;
    old.set(live, 47)?;
    old.set(freed, 49)?;
    working::free(device, freed, old_handle)?;
    old.finish()?;
    verify(cuda, live, 47)?;
    old.close()?; // Retires both live and Deferred origin references before actual destruction.
    let before = driver_counters(device)?;
    working::free(device, live, old_handle)?; // A surviving tensor supplies its old origin handle.
    let after = driver_counters(device)?;
    require(
        before == after,
        "free after retired stream destruction used the dead stream",
    )?;
    let mut next = Stream::new(cuda)?;
    let ptr = working::alloc(device, BYTES, next.raw(), None)?;
    next.set(ptr, 53)?;
    next.finish()?;
    verify(cuda, ptr, 53)?;
    working::free(device, ptr, next.raw())?;
    let reused = next.raw() == old_handle;
    next.close()?;
    Ok(
        json!({"before_old_origin_free":before,"after_old_origin_free":after,
        "actual_old_stream_destroyed":true,"numeric_handle_reused":reused,"new_stream_bytes_correct":true}),
    )
}

fn public_retired(minimum: u64) -> Result<varena_client::RetirementStats> {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let stats = varena_client::retirement_stats()?;
        require(
            stats.quarantined == 0 && stats.abandoned_cache_bytes == 0,
            "public retirement quarantined or abandoned the fixture allocation",
        )?;
        if stats.queued == 0 && stats.retired >= minimum {
            return Ok(stats);
        }
        require(
            Instant::now() < deadline,
            "public retirement did not finish within fixture protocol",
        )?;
        std::thread::sleep(Duration::from_millis(1));
    }
}

pub(super) fn public_borrowed(cuda: &Cuda, device: i32, deferred: bool) -> Result<Value> {
    require(
        varena_client::core_identity() == varena::abi::vr_v1_core_identity(),
        "public client and native fixture must resolve the same core",
    )?;
    let gpu = varena_client::Gpu::open(device)?;
    let mut producer = Stream::new(cuda)?;
    // SAFETY: this primary-context stream remains alive through checked public free,
    // all retaining Rust objects, and any queued ABI retirement, then actual completion.
    let borrowed = unsafe { gpu.borrow_stream(producer.raw()) };
    let mut allocation = gpu.alloc(BYTES as u64, &borrowed)?;
    let pointer = allocation.ptr()?;
    producer.set(pointer, 59)?;
    producer.finish()?;
    verify(cuda, pointer, 59)?;
    let before = working::stats(device)?;
    let retired_before = varena_client::retirement_stats()?.retired;
    if !deferred {
        allocation.free()?;
    }
    drop(allocation); // The deferred case uses the actual Rust finalizer → C ABI queue.
    let retirement = public_retired(retired_before + u64::from(deferred))?;
    drop(borrowed);
    let after = working::stats(device)?;
    require(
        after.allocated_bytes == 0 && after.event_records > before.event_records,
        "public Allocation release did not establish its immediate borrowed-stream fence",
    )?;
    producer.close_public_borrowed()?;
    let trimmed = gpu.trim()?; // Must not try to record a fresh fence on the destroyed handle.
    require(
        working::stats(device)?.mapped_bytes == 0,
        "public trim retained freed borrowed-stream backing",
    )?;

    let mut next = Stream::new(cuda)?;
    // SAFETY: the new actual stream has the same explicit public lifetime discipline.
    let borrowed = unsafe { gpu.borrow_stream(next.raw()) };
    let mut allocation = gpu.alloc(BYTES as u64, &borrowed)?;
    next.set(allocation.ptr()?, 61)?;
    next.finish()?;
    verify(cuda, allocation.ptr()?, 61)?;
    allocation.free()?;
    drop(allocation);
    drop(borrowed);
    next.close_public_borrowed()?;
    gpu.trim()?;
    let retired_before = varena_client::retirement_stats()?.retired;
    drop(gpu);
    public_retired(retired_before + 1)?;
    Ok(
        json!({"deferred_abi_retirement":deferred,"retired_before_stream_destroy":retirement.retired,
        "queue_drained_before_stream_destroy":retirement.queued == 0,
        "public_free_record_delta":after.event_records - before.event_records,
        "private_retire_stream_used":false,"trim_after_destroy":trimmed,
        "cross_stream_allocation_all_bytes_correct":true}),
    )
}
