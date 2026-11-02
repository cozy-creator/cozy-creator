use super::*;
use libloading::Library;
use std::{
    ffi::c_void,
    fs::OpenOptions,
    time::{Duration, Instant},
};
use tensorfs::io::source::{RawFill, RawRead};
use varena::{working, Arena, Preparation, StreamPreparation};
#[path = "extra.rs"]
mod extra;
type Obj = *mut c_void;

struct Cuda {
    _library: Library,
    stream_new: unsafe extern "system" fn(*mut Obj, u32) -> i32,
    stream_destroy: unsafe extern "system" fn(Obj) -> i32,
    stream_sync: unsafe extern "system" fn(Obj) -> i32,
    stream_query: unsafe extern "system" fn(Obj) -> i32,
    wait: unsafe extern "system" fn(Obj, u64, u32, u32) -> i32,
    write: unsafe extern "system" fn(Obj, u64, u32, u32) -> i32,
    set: unsafe extern "system" fn(u64, u8, usize) -> i32,
    copy: unsafe extern "system" fn(u64, u64, usize, Obj) -> i32,
    read: unsafe extern "system" fn(*mut c_void, u64, usize) -> i32,
    capture_begin: unsafe extern "system" fn(Obj, u32) -> i32,
    capture_end: unsafe extern "system" fn(Obj, *mut Obj) -> i32,
    graph_destroy: unsafe extern "system" fn(Obj) -> i32,
    uuid: String,
}
fn check(operation: &str, code: i32) -> Result<()> {
    if code == 0 {
        Ok(())
    } else {
        Err(format!("{operation}: CUDA {code}").into())
    }
}
impl Cuda {
    fn open(device: i32, expected: &str) -> Result<Self> {
        unsafe {
            let library = Library::new("libcuda.so.1")?;
            check(
                "cuInit",
                library.get::<unsafe extern "system" fn(u32) -> i32>(b"cuInit\0")?(0),
            )?;
            let mut context = std::ptr::null_mut();
            check(
                "cuDevicePrimaryCtxRetain",
                library.get::<unsafe extern "system" fn(*mut Obj, i32) -> i32>(
                    b"cuDevicePrimaryCtxRetain\0",
                )?(&mut context, device),
            )?;
            check(
                "cuCtxPushCurrent",
                library.get::<unsafe extern "system" fn(Obj) -> i32>(b"cuCtxPushCurrent_v2\0")?(
                    context,
                ),
            )?;
            let mut uuid = [0u8; 16];
            check(
                "cuDeviceGetUuid",
                library.get::<unsafe extern "system" fn(*mut u8, i32) -> i32>(
                    b"cuDeviceGetUuid_v2\0",
                )?(uuid.as_mut_ptr(), device),
            )?;
            let uuid = uuid
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect::<String>();
            require(
                uuid == expected
                    .trim_start_matches("GPU-")
                    .replace('-', "")
                    .to_lowercase(),
                "wrong GPU UUID",
            )?;
            Ok(Self {
                stream_new: *library.get(b"cuStreamCreate\0")?,
                stream_destroy: *library.get(b"cuStreamDestroy_v2\0")?,
                stream_sync: *library.get(b"cuStreamSynchronize\0")?,
                stream_query: *library.get(b"cuStreamQuery\0")?,
                wait: *library.get(b"cuStreamWaitValue32_v2\0")?,
                write: *library.get(b"cuStreamWriteValue32_v2\0")?,
                set: *library.get(b"cuMemsetD8_v2\0")?,
                copy: *library.get(b"cuMemcpyDtoDAsync_v2\0")?,
                read: *library.get(b"cuMemcpyDtoH_v2\0")?,
                capture_begin: *library.get(b"cuStreamBeginCapture_v2\0")?,
                capture_end: *library.get(b"cuStreamEndCapture\0")?,
                graph_destroy: *library.get(b"cuGraphDestroy\0")?,
                _library: library,
                uuid,
            })
        }
    }
    fn stream(&self) -> Result<u64> {
        let mut stream = std::ptr::null_mut();
        unsafe {
            check("nonblocking stream", (self.stream_new)(&mut stream, 1))?;
        }
        Ok(stream as u64)
    }
    fn sync(&self, stream: u64) -> Result<()> {
        unsafe { check("stream completion", (self.stream_sync)(stream as Obj)) }
    }
    fn pending(&self, stream: u64) -> Result<bool> {
        let status = unsafe { (self.stream_query)(stream as Obj) };
        if status == 600 {
            Ok(true)
        } else {
            check("stream query", status)?;
            Ok(false)
        }
    }
    fn bytes(&self, pointer: u64, count: usize) -> Result<Vec<u8>> {
        let mut bytes = vec![0; count];
        unsafe {
            check(
                "full D2H readback",
                (self.read)(bytes.as_mut_ptr().cast(), pointer, count),
            )?;
        }
        Ok(bytes)
    }
    fn open_gate(&self, stream: u64, gate: u64) -> Result<()> {
        unsafe {
            check(
                "open actual device gate",
                (self.write)(stream as Obj, gate, 1, 0),
            )?;
        }
        self.sync(stream)
    }
}
fn until(mut test: impl FnMut() -> Result<bool>) -> Result<()> {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        if test()? {
            return Ok(());
        }
        if Instant::now() >= deadline {
            return Err("native source protocol deadline".into());
        }
        std::thread::sleep(Duration::from_millis(1));
    }
}
fn fill(region: &Arc<varena::Region>) -> Result<varena::Fill> {
    match region.prepare()? {
        Preparation::Fill(fill) => Ok(fill),
        Preparation::Resident(_) => Err("expected cold Fill".into()),
    }
}
fn prepare(
    source: &Arc<Source>,
    fill: varena::Fill,
    items: Vec<Item>,
    threads: usize,
) -> Result<RawRead> {
    match unsafe {
        source.prepare_raw(
            vec![RawFill {
                fill,
                items,
                origin: 0,
            }],
            threads,
            Mode::Buffered,
        )
    } {
        Ok(read) => Ok(read),
        Err((owners, error)) => {
            std::mem::forget(owners);
            Err(error.into())
        }
    }
}
fn observed(read: &RawRead) -> Result<Value> {
    let f = read.facts()?;
    Ok(
        json!({"submission_closed":f.submission_closed,"published_regions":f.published_regions,
        "completed":f.completed,"resources_retired":f.resources_retired,
        "submitted_fills_retired":f.submitted_fills_retired,"requested_bytes":f.requested_bytes,
        "issued_bytes":f.issued_bytes,"submission_ns":f.submission_ns,"failure":f.failure,
        "retirement_failure":f.retirement_failure,"producer_streams":f.producer_streams,
        "stats":f.stats.map(|s|json!({"bytes":s.bytes,"ns":s.ns,"busy_ns":s.busy_ns,
            "cached":s.cached,"direct":s.direct,"staged":s.staged}))}),
    )
}
fn arena_stats(arena: &Arena) -> Result<Value> {
    let f = arena.stats()?;
    Ok(
        json!({"resident":f.resident_bytes,"ready":f.ready_bytes,"pinned":f.pinned_bytes,
        "reserved":f.reserved_bytes,"joins":f.context_joins,"poisoned":f.poisoned}),
    )
}
fn retire(arena: Arc<Arena>) -> Result<()> {
    let ticket = match arena.request_retirement() {
        Ok(ticket) => ticket,
        Err((owner, error)) => {
            std::mem::forget(owner);
            return Err(error.into());
        }
    };
    until(|| ticket.try_complete().map_err(Into::into))?;
    require(
        ticket.facts()?.remaining_physical_bytes == 0,
        "Arena retired with physical bytes",
    )
}
fn finish(cuda: &Cuda, device: i32, pointers: &[u64], streams: &[u64]) -> Result<()> {
    for &stream in streams {
        cuda.sync(stream)?;
    }
    for &pointer in pointers {
        unsafe {
            working::free_completed(device, pointer)?;
        }
    }
    working::trim(device)?;
    for &stream in streams {
        unsafe {
            check("destroy owned stream", (cuda.stream_destroy)(stream as Obj))?;
        }
    }
    Ok(())
}
fn independent(cuda: &Cuda, region: &Arc<varena::Region>, stream: u64, output: u64) -> Result<()> {
    let pin = region.pin()?;
    unsafe {
        check(
            "independent Arena D2D",
            (cuda.copy)(output, pin.ptr(), 4096, stream as Obj),
        )?;
    }
    cuda.sync(stream)?;
    require(
        cuda.bytes(output, 4096)?.iter().all(|&byte| byte == 91),
        "independent Arena bytes differ",
    )?;
    unsafe {
        drop(pin.into_completed_region());
    }
    Ok(())
}
fn ordering(cuda: &Cuda, root: &Path, device: i32) -> Result<Value> {
    let recipe = checkpoint(root)?;
    let checkpoint_id = recipe.checkpoint.id().clone();
    let arena = Arena::new(device)?;
    let region = arena.reserve(LENGTH)?;
    let mut read = prepare(&recipe.source, fill(&region)?, recipe.items, 2)?;
    let producer_streams = read.facts()?.producer_streams;
    require(
        producer_streams.len() == 2,
        "unexpected raw Source producer set",
    )?;
    // Exactly two staged pieces (one file, one inline); no ring reuse needs the closed gate.
    let control = cuda.stream()?;
    let a = cuda.stream()?;
    let b = cuda.stream()?;
    let other = cuda.stream()?;
    let gate = working::alloc(device, 4, control, None)?;
    let consumer_gate = working::alloc(device, 4, control, None)?;
    let out_a = working::alloc(device, LENGTH, a, None)?;
    let out_b = working::alloc(device, LENGTH, b, None)?;
    let other_out = working::alloc(device, 4096, other, None)?;
    let live = Arena::new(device)?;
    let live_region = live.reserve(4096)?;
    let mut live_fill = fill(&live_region)?;
    live_fill.copy_from_host(&[91; 4096])?;
    unsafe {
        drop(live_fill.commit_completed()?.into_completed_region());
    }
    unsafe {
        check("clear producer gate", (cuda.set)(gate, 0, 4))?;
        check("clear consumer gate", (cuda.set)(consumer_gate, 0, 4))?;
        // Borrowed actual producer IDs, used only before Source submission. This does not
        // add a writer of Fill bytes or mutate the inventory. Gate memory stays owned.
        for &stream in &producer_streams {
            check(
                "actual Source producer gate",
                (cuda.wait)(stream as Obj, gate, 1, 1),
            )?;
        }
    }
    drop(recipe.checkpoint);
    drop(recipe.source);
    let cancel = ReadControl::default();
    read.submit(&cancel)?;
    let pending = read.facts()?;
    require(
        pending.submission_closed && pending.published_regions == 1 && !pending.completed,
        "Source publication pretended producer completion",
    )?;
    require(
        pending.requested_bytes == (PAYLOAD + 17) as u64 && pending.issued_bytes == LENGTH as u64,
        "Source requested/issued byte counters differ",
    )?;
    require(
        read.poll_complete()?.is_none() && !region.is_resident()?,
        "gated Source appeared Ready",
    )?;
    let pin = match unsafe { region.prepare_on(a)? } {
        StreamPreparation::Ordered(pin) => pin,
        _ => return Err("expected Ordered Source pin".into()),
    };
    let second = pin.try_clone()?;
    unsafe {
        second.order_on(b)?;
        check(
            "consumer A",
            (cuda.copy)(out_a, pin.ptr(), LENGTH, a as Obj),
        )?;
        check(
            "consumer B gate",
            (cuda.wait)(b as Obj, consumer_gate, 1, 1),
        )?;
        check(
            "consumer B",
            (cuda.copy)(out_b, second.ptr(), LENGTH, b as Obj),
        )?;
        drop(pin.into_fenced_region(&[a]));
    }
    require(
        cuda.pending(a)? && cuda.pending(b)?,
        "consumer bypassed actual Source producers",
    )?;
    independent(cuda, &live_region, other, other_out)?;
    require(
        arena.reclaim(LENGTH)?.reclaimed_bytes == 0,
        "pending Source backing was reclaimed",
    )?;
    let pending_facts = observed(&read)?;
    cuda.open_gate(control, gate)?;
    cuda.sync(a)?;
    require(
        cuda.bytes(out_a, LENGTH)? == recipe.expected,
        "first Source consumer bytes differ",
    )?;
    until(|| Ok(read.poll_complete()?.is_some()))?;
    until(|| Ok(read.facts()?.resources_retired))?;
    require(
        cuda.pending(b)?,
        "consumer gate did not hold independent ownership",
    )?;
    require(
        arena.reclaim(LENGTH)?.reclaimed_bytes == 0,
        "source completion reclaimed a live consumer",
    )?;
    independent(cuda, &live_region, other, other_out)?;
    let completed = observed(&read)?;
    drop(read);
    cuda.open_gate(control, consumer_gate)?;
    cuda.sync(b)?;
    require(
        cuda.bytes(out_b, LENGTH)? == recipe.expected,
        "second Source consumer bytes differ",
    )?;
    unsafe {
        drop(second.into_fenced_region(&[b]));
    }
    arena.reclaim(LENGTH)?;
    require(
        arena.stats()?.resident_bytes == 0 && arena.stats()?.context_joins == 0,
        "Source cleanup joined context or kept backing",
    )?;
    let final_stats = arena_stats(&arena)?;
    finish(
        cuda,
        device,
        &[gate, consumer_gate, out_a, out_b, other_out],
        &[control, a, b, other],
    )?;
    drop(region);
    retire(arena)?;
    drop(live_region);
    retire(live)?;
    Ok(
        json!({"checkpoint":checkpoint_id,"pending":pending_facts,"completed":completed,
        "source_handles_dropped_before_submission":true,"two_full_consumer_readbacks":true,
        "independent_arena_readbacks":2,"final":final_stats}),
    )
}
fn canceled_before(cuda: &Cuda, root: &Path, device: i32, drop_prepared: bool) -> Result<Value> {
    let recipe = checkpoint(root)?;
    let arena = Arena::new(device)?;
    let region = arena.reserve(LENGTH)?;
    let mut read = prepare(&recipe.source, fill(&region)?, recipe.items, 2)?;
    let cancel = ReadControl::default();
    let facts = if drop_prepared {
        drop(read);
        json!({"prepared_dropped":true})
    } else {
        cancel.cancel();
        require(read.submit(&cancel).is_err(), "canceled Source submitted")?;
        until(|| Ok(read.poll_complete()?.is_some()))?;
        require(
            read.facts()?.published_regions == 0 && cancel.issued_bytes() == 0,
            "cancel fabricated publication or bytes",
        )?;
        let facts = observed(&read)?;
        drop(read);
        facts
    };
    until(|| Ok(arena.stats()?.pinned_bytes == 0 && arena.stats()?.resident_bytes == 0))?;
    require(
        !region.is_resident()? && arena.stats()?.context_joins == 0,
        "unstarted cleanup joined or became Ready",
    )?;
    let result = json!({"facts":facts,"final":arena_stats(&arena)?,"gpu":cuda.uuid});
    drop(region);
    retire(arena)?;
    Ok(result)
}
fn partial_error(cuda: &Cuda, root: &Path, device: i32) -> Result<Value> {
    let recipe = checkpoint(root)?;
    let arena = Arena::new(device)?;
    let region = arena.reserve(LENGTH)?;
    let pointer = region.ptr();
    let object = match &recipe.items[0].source {
        ItemSource::Object(range) => range.obj.clone(),
        _ => unreachable!(),
    };
    let items = vec![
        Item {
            offset: 0,
            len: 65536,
            source: ItemSource::Object(io::ObjectRange {
                obj: object.clone(),
                off: 0,
                len: 65536,
            }),
            part_end: true,
        },
        Item {
            offset: 262144,
            len: 65536,
            source: ItemSource::Object(io::ObjectRange {
                obj: object,
                off: 262144,
                len: 65536,
            }),
            part_end: true,
        },
    ];
    let initial = fill(&region)?;
    let alias = initial.pin()?;
    let mut read = prepare(&recipe.source, initial, items, 1)?;
    // Corrupt only this invocation's newly created checkpoint chunk, after the Source owns
    // its verified descriptor. The real second file read reaches EOF after the first copy.
    let mut permissions = std::fs::metadata(&recipe.chunk_path)?.permissions();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        permissions.set_mode(0o600);
    }
    std::fs::set_permissions(&recipe.chunk_path, permissions)?;
    OpenOptions::new()
        .write(true)
        .open(&recipe.chunk_path)?
        .set_len(65536)?;
    let cancel = ReadControl::default();
    let error = read
        .submit(&cancel)
        .expect_err("truncated real checkpoint unexpectedly read");
    until(|| Ok(read.poll_complete()?.is_some()))?;
    require(
        read.facts()?.published_regions == 0 && !region.is_resident()?,
        "partial Source bytes became Ready",
    )?;
    require(
        cancel.issued_bytes() >= 65536,
        "first actual copy was not submitted before EOF",
    )?;
    require(
        cuda.bytes(alias.ptr(), 65536)? == recipe.payload[..65536],
        "escaped partial bytes differ",
    )?;
    require(
        matches!(region.prepare(), Err(varena::Error::Busy)),
        "escaped failed-fill alias was overwritten",
    )?;
    let failed = observed(&read)?;
    unsafe {
        drop(alias.into_completed_region());
    }
    drop(read);
    until(|| Ok(arena.stats()?.pinned_bytes == 0 && arena.stats()?.resident_bytes == 0))?;
    std::fs::write(&recipe.chunk_path, &recipe.payload)?;
    let mut refill = prepare(&recipe.source, fill(&region)?, recipe.items, 2)?;
    refill.submit(&ReadControl::default())?;
    until(|| Ok(refill.poll_complete()?.is_some()))?;
    let pin = region.pin()?;
    require(
        cuda.bytes(pin.ptr(), LENGTH)? == recipe.expected,
        "restored real checkpoint refill differs",
    )?;
    unsafe {
        drop(pin.into_completed_region());
    }
    until(|| Ok(refill.facts()?.resources_retired && arena.stats()?.pinned_bytes == 0))?;
    drop(refill);
    arena.reclaim(LENGTH)?;
    require(
        region.ptr() == pointer && arena.stats()?.context_joins == 0,
        "refill changed VA or joined context",
    )?;
    let result = json!({"actual_io_error":error.to_string(),"failed":failed,"escaped_partial_readback":true,
        "restored_checkpoint_refill":true,"stable_va":true,"final":arena_stats(&arena)?});
    drop(region);
    retire(arena)?;
    Ok(result)
}
fn capture(cuda: &Cuda, root: &Path, device: i32) -> Result<Value> {
    let recipe = checkpoint(root)?;
    let arena = Arena::new(device)?;
    let region = arena.reserve(LENGTH)?;
    let inputs = vec![RawFill {
        fill: fill(&region)?,
        items: recipe.items,
        origin: 0,
    }];
    let stream = cuda.stream()?;
    let guard = varena::enter_capture(device)?;
    unsafe {
        check(
            "begin actual CUDA capture",
            (cuda.capture_begin)(stream as Obj, 0),
        )?;
    }
    let inputs = match unsafe { recipe.source.prepare_raw(inputs, 2, Mode::Buffered) } {
        Ok(owner) => {
            std::mem::forget(owner);
            return Err("Source prepared during capture".into());
        }
        Err((inputs, error)) => {
            require(
                matches!(&error,io::source::Error::Read(error) if matches!(error.cuda(),Some(io::cuda::Error::Allocation(varena::Error::CaptureActive)))),
                "capture refusal returned the wrong reason",
            )?;
            inputs
        }
    };
    let mut graph = std::ptr::null_mut();
    unsafe {
        check(
            "end actual CUDA capture",
            (cuda.capture_end)(stream as Obj, &mut graph),
        )?;
    }
    drop(guard);
    unsafe {
        check("destroy capture graph", (cuda.graph_destroy)(graph))?;
    }
    let mut read = match unsafe { recipe.source.prepare_raw(inputs, 2, Mode::Buffered) } {
        Ok(read) => read,
        Err((owners, error)) => {
            std::mem::forget(owners);
            return Err(error.into());
        }
    };
    read.submit(&ReadControl::default())?;
    until(|| Ok(read.poll_complete()?.is_some()))?;
    let pin = region.pin()?;
    require(
        cuda.bytes(pin.ptr(), LENGTH)? == recipe.expected,
        "capture retry read differs",
    )?;
    unsafe {
        drop(pin.into_completed_region());
    }
    until(|| Ok(read.facts()?.resources_retired && arena.stats()?.pinned_bytes == 0))?;
    let facts = observed(&read)?;
    drop(read);
    arena.reclaim(LENGTH)?;
    finish(cuda, device, &[], &[stream])?;
    drop(region);
    retire(arena)?;
    Ok(
        json!({"actual_capture_refused_before_engine":true,"same_owned_inputs_retried":true,"full_readback":true,"read":facts}),
    )
}
pub(super) fn run(mode: &str, root: &Path, uuid: &str, device: i32) -> Result<Value> {
    let cuda = Cuda::open(device, uuid)?;
    let detail = match mode {
        "ordering" => ordering(&cuda, root, device)?,
        "cancel" => canceled_before(&cuda, root, device, false)?,
        "prepared_drop" => canceled_before(&cuda, root, device, true)?,
        "partial_error" => partial_error(&cuda, root, device)?,
        "capture" => capture(&cuda, root, device)?,
        "cancel_active" => extra::active_cancel(&cuda, root, device)?,
        "multi_fill" => extra::multi_fill(&cuda, root, device)?,
        _ => return Err("unknown raw Source GPU mode".into()),
    };
    Ok(
        json!({"passed":true,"mode":mode,"gpu_uuid":cuda.uuid,"pid":std::process::id(),"detail":detail}),
    )
}
