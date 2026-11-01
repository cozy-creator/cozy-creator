//! Real standalone CUDA fill ownership; no Python, Torch, mocks or manufactured OOM.
use libloading::Library;
use serde_json::{Value, json};
use std::{
    ffi::c_void,
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
    time::{Duration, Instant},
};
use varena::{Arena, FillFacts, FillOwners, FillState, Preparation, StreamPreparation, working};
type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;
type Obj = *mut c_void;
type Copy = unsafe extern "system" fn(u64, *const c_void, usize, Obj) -> i32;
type D2D = unsafe extern "system" fn(u64, u64, usize, Obj) -> i32;
type StreamOp = unsafe extern "system" fn(Obj) -> i32;
const BYTES: usize = 4 << 20;
fn check(op: &str, code: i32) -> Result<()> {
    if code == 0 {
        Ok(())
    } else {
        Err(format!("{op}: CUDA {code}").into())
    }
}
fn require(condition: bool, message: &str) -> Result<()> {
    if condition {
        Ok(())
    } else {
        Err(message.into())
    }
}
struct Cuda {
    _library: Library,
    context: usize,
    push: unsafe extern "system" fn(Obj) -> i32,
    pop: unsafe extern "system" fn(*mut Obj) -> i32,
    stream_new: unsafe extern "system" fn(*mut Obj, u32) -> i32,
    stream_destroy: StreamOp,
    stream_sync: StreamOp,
    stream_query: StreamOp,
    capture_begin: unsafe extern "system" fn(Obj, u32) -> i32,
    capture_end: unsafe extern "system" fn(Obj, *mut Obj) -> i32,
    graph_destroy: StreamOp,
    host_alloc: unsafe extern "system" fn(*mut Obj, usize, u32) -> i32,
    host_free: StreamOp,
    copy: Copy,
    d2d: D2D,
    read: unsafe extern "system" fn(*mut c_void, u64, usize) -> i32,
    set: unsafe extern "system" fn(u64, u8, usize) -> i32,
    set_async: unsafe extern "system" fn(u64, u8, usize, Obj) -> i32,
    wait: unsafe extern "system" fn(Obj, u64, u32, u32) -> i32,
    write: unsafe extern "system" fn(Obj, u64, u32, u32) -> i32,
    uuid: String,
}
impl Cuda {
    fn open(device: i32) -> Result<Arc<Self>> {
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
            let push = *library.get(b"cuCtxPushCurrent_v2\0")?;
            let mut uuid = [0u8; 16];
            check(
                "cuDeviceGetUuid",
                library.get::<unsafe extern "system" fn(*mut u8, i32) -> i32>(
                    b"cuDeviceGetUuid_v2\0",
                )?(uuid.as_mut_ptr(), device),
            )?;
            let encoded = uuid.iter().map(|b| format!("{b:02x}")).collect::<String>();
            let cuda = Arc::new(Self {
                context: context as usize,
                push,
                pop: *library.get(b"cuCtxPopCurrent_v2\0")?,
                stream_new: *library.get(b"cuStreamCreate\0")?,
                stream_destroy: *library.get(b"cuStreamDestroy_v2\0")?,
                stream_sync: *library.get(b"cuStreamSynchronize\0")?,
                stream_query: *library.get(b"cuStreamQuery\0")?,
                capture_begin: *library.get(b"cuStreamBeginCapture_v2\0")?,
                capture_end: *library.get(b"cuStreamEndCapture\0")?,
                graph_destroy: *library.get(b"cuGraphDestroy\0")?,
                host_alloc: *library.get(b"cuMemHostAlloc\0")?,
                host_free: *library.get(b"cuMemFreeHost\0")?,
                copy: *library.get(b"cuMemcpyHtoDAsync_v2\0")?,
                d2d: *library.get(b"cuMemcpyDtoDAsync_v2\0")?,
                read: *library.get(b"cuMemcpyDtoH_v2\0")?,
                set: *library.get(b"cuMemsetD8_v2\0")?,
                set_async: *library.get(b"cuMemsetD8Async\0")?,
                wait: *library.get(b"cuStreamWaitValue32\0")?,
                write: *library.get(b"cuStreamWriteValue32\0")?,
                uuid: encoded,
                _library: library,
            });
            check("cuCtxPushCurrent", (cuda.push)(context))?;
            Ok(cuda)
        }
    }
    fn stream(&self) -> Result<u64> {
        let mut stream = std::ptr::null_mut();
        unsafe { check("cuStreamCreate", (self.stream_new)(&mut stream, 1))? };
        Ok(stream as u64)
    }
    fn sync(&self, stream: u64) -> Result<()> {
        unsafe { check("cuStreamSynchronize", (self.stream_sync)(stream as Obj)) }
    }
    fn pending(&self, stream: u64) -> Result<bool> {
        let code = unsafe { (self.stream_query)(stream as Obj) };
        if code == 600 {
            Ok(true)
        } else {
            check("cuStreamQuery", code)?;
            Ok(false)
        }
    }
    fn verify(&self, ptr: u64) -> Result<()> {
        let mut bytes = vec![0u8; BYTES];
        unsafe {
            check(
                "cuMemcpyDtoH",
                (self.read)(bytes.as_mut_ptr().cast(), ptr, BYTES),
            )?;
        }
        require(
            bytes[..BYTES / 2].iter().all(|&b| b == 41)
                && bytes[BYTES / 2..].iter().all(|&b| b == 73),
            "full byte readback differs",
        )
    }
}
#[derive(Default)]
struct ResourceState {
    streams_released: usize,
    host_released: bool,
    failed: bool,
    retired: bool,
}
struct Resources {
    state: Mutex<ResourceState>,
    cuda: Arc<Cuda>,
    streams: Vec<u64>,
    host: usize,
    dropped: Arc<AtomicUsize>,
    errors: Arc<AtomicUsize>,
}
impl Resources {
    fn new(cuda: Arc<Cuda>) -> Result<Arc<Self>> {
        let streams = vec![cuda.stream()?, cuda.stream()?];
        let mut host = std::ptr::null_mut();
        unsafe {
            check("cuMemHostAlloc", (cuda.host_alloc)(&mut host, BYTES, 1))?;
            let bytes = std::slice::from_raw_parts_mut(host.cast::<u8>(), BYTES);
            bytes[..BYTES / 2].fill(41);
            bytes[BYTES / 2..].fill(73);
        }
        Ok(Arc::new(Self {
            state: Mutex::new(ResourceState::default()),
            cuda,
            streams,
            host: host as usize,
            dropped: Arc::new(AtomicUsize::new(0)),
            errors: Arc::new(AtomicUsize::new(0)),
        }))
    }
}
impl Resources {
    fn retire_checked(&self) -> varena::Result<()> {
        let mut state = self
            .state
            .lock()
            .map_err(|_| varena::Error::Poisoned("fixture resource lock".into()))?;
        if state.retired {
            return Ok(());
        }
        if state.failed {
            return Err(varena::Error::Poisoned(
                "fixture resource release was unproven".into(),
            ));
        }
        state.failed = true; // a failing driver call must never be repeated by Drop
        let status = |operation, code| {
            if code == 0 {
                Ok(())
            } else {
                self.errors.fetch_add(1, Ordering::SeqCst);
                Err(varena::Error::Cuda { operation, code })
            }
        };
        // SAFETY: this wrapper retains every exact native handle. Varena calls this only
        // after all producers complete under retirement exclusion; setup failure has no work.
        unsafe {
            status(
                "fixture cuCtxPushCurrent",
                (self.cuda.push)(self.cuda.context as Obj),
            )?;
            let released: varena::Result<()> = (|| {
                while state.streams_released < self.streams.len() {
                    status(
                        "fixture cuStreamDestroy",
                        (self.cuda.stream_destroy)(self.streams[state.streams_released] as Obj),
                    )?;
                    state.streams_released += 1;
                }
                if !state.host_released {
                    status(
                        "fixture cuMemFreeHost",
                        (self.cuda.host_free)(self.host as Obj),
                    )?;
                    state.host_released = true;
                }
                Ok(())
            })();
            let mut old = std::ptr::null_mut();
            let popped = status("fixture cuCtxPopCurrent", (self.cuda.pop)(&mut old));
            released?;
            popped?;
        }
        state.retired = true;
        state.failed = false;
        self.dropped.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
}
impl Drop for Resources {
    fn drop(&mut self) {
        // Only an unsubmitted setup failure can reach a non-inert normal destructor.
        // A checked failure leaves its remaining handles quarantined, never retried.
        let state = self
            .state
            .get_mut()
            .unwrap_or_else(|error| error.into_inner());
        if !state.retired && !state.failed {
            let _ = self.retire_checked();
        }
    }
}

fn facts(f: FillFacts) -> Value {
    json!({"state":format!("{:?}",f.state),"submission_closed":f.submission_closed,
    "published":f.published,"completed":f.completed,"retirement_finished":f.retirement_finished,"producers":f.producer_count,
    "recorded":f.recorded_count,"completed_producers":f.completed_count,"failure":f.failure.map(|e|e.to_string()),
    "retirement_failure":f.retirement_failure.map(|e|e.to_string()),"resources_remaining":f.resources_remaining})
}
fn stats(arena: &Arena) -> Result<Value> {
    let s = arena.stats()?;
    Ok(json!({"resident":s.resident_bytes,"ready":s.ready_bytes,
    "pinned":s.pinned_bytes,"reserved":s.reserved_bytes,"fills":s.fills,"reclaimed":s.reclaimed_bytes,
    "joins":s.context_joins,"event_waits":s.event_waits,"poisoned":s.poisoned}))
}
fn until(mut condition: impl FnMut() -> Result<bool>) -> Result<()> {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        if condition()? {
            return Ok(());
        }
        require(
            Instant::now() < deadline,
            "native completion/retirement did not reach its predicate",
        )?;
        std::thread::sleep(Duration::from_millis(1));
    }
}
fn fill(region: &Arc<varena::Region>) -> Result<varena::Fill> {
    match region.prepare()? {
        Preparation::Fill(fill) => Ok(fill),
        _ => Err("expected fresh fill".into()),
    }
}
struct NativeResources(Option<Arc<Resources>>);
// SAFETY: only checked producer completion reaches retire. Taking the Arc leaves the
// wrapper inert; Resources releases its native stream/host handles without Python/GIL.
unsafe impl varena::FillResource for NativeResources {
    fn retire(&mut self) -> varena::Result<varena::FillRetirement> {
        if let Some(resources) = &self.0 {
            resources.retire_checked()?;
        }
        drop(self.0.take()); // successful checked retirement leaves this wrapper inert
        Ok(varena::FillRetirement::Complete)
    }
}
fn owned(fill: varena::Fill, resources: &Arc<Resources>) -> varena::FillSession {
    // SAFETY: the fixed same-context streams and pinned host page remain owned by this
    // native checked-retirement wrapper through all copies and their actual completion.
    unsafe {
        fill.into_async(FillOwners::new(
            resources.streams.clone(),
            vec![Box::new(NativeResources(Some(Arc::clone(resources))))],
        ))
    }
}

fn submit(cuda: &Cuda, resources: &Resources, ptr: u64, gate: u64, partial: bool) -> Result<()> {
    unsafe {
        // Actual device dependency keeps one writer pending, independent of host timing.
        check(
            "cuStreamWaitValue32",
            (cuda.wait)(resources.streams[0] as Obj, gate, 1, 1),
        )?;
        check(
            "H2D first",
            (cuda.copy)(
                ptr,
                resources.host as *const c_void,
                BYTES / 2,
                resources.streams[0] as Obj,
            ),
        )?;
        if !partial {
            check(
                "H2D second",
                (cuda.copy)(
                    ptr + (BYTES / 2) as u64,
                    (resources.host + BYTES / 2) as *const c_void,
                    BYTES / 2,
                    resources.streams[1] as Obj,
                ),
            )?;
        }
    }
    Ok(())
}
fn release_gate(cuda: &Cuda, gate: u64, control: u64) -> Result<()> {
    unsafe {
        check(
            "cuStreamWriteValue32",
            (cuda.write)(control as Obj, gate, 1, 0),
        )?;
    }
    cuda.sync(control)
}
fn finish_working(cuda: &Cuda, device: i32, allocations: &[u64], streams: &[u64]) -> Result<()> {
    for &stream in streams {
        cuda.sync(stream)?;
    }
    for &ptr in allocations {
        unsafe {
            working::free_completed(device, ptr)?;
        }
    }
    working::trim(device)?;
    for &stream in streams {
        unsafe {
            check("cuStreamDestroy", (cuda.stream_destroy)(stream as Obj))?;
        }
    }
    Ok(())
}
fn ordering(cuda: Arc<Cuda>, device: i32) -> Result<Value> {
    let arena = Arena::new(device)?;
    let region = arena.reserve(BYTES)?;
    let ptr = region.ptr();
    let resources = Resources::new(Arc::clone(&cuda))?;
    let dropped = Arc::clone(&resources.dropped);
    let errors = Arc::clone(&resources.errors);
    let control = cuda.stream()?;
    let a = cuda.stream()?;
    let b = cuda.stream()?;
    let gate = working::alloc(device, 4, control, None)?;
    let out_a = working::alloc(device, BYTES, a, None)?;
    let out_b = working::alloc(device, BYTES, b, None)?;
    unsafe {
        check("clear gate", (cuda.set)(gate, 0, 4))?;
    }
    let mut session = owned(fill(&region)?, &resources);
    let destination = session.begin()?;
    submit(&cuda, &resources, destination.ptr, gate, false)?;
    let ticket = unsafe { session.publish()? };
    drop(resources);
    drop(session);
    let published = ticket.facts()?;
    require(
        published.published && !published.completed && published.recorded_count == 2,
        "publication pretended completion",
    )?;
    require(
        !region.is_resident()? && region.try_pin()?.is_none(),
        "legacy ready path exposed pending fill",
    )?;
    require(
        matches!(region.prepare(), Err(varena::Error::Busy)),
        "legacy prepare accepted pending fill",
    )?;
    let pin = match unsafe { region.prepare_on(a)? } {
        StreamPreparation::Ordered(pin) => pin,
        _ => return Err("expected native ordered pin".into()),
    };
    let other = pin.try_clone()?;
    unsafe {
        other.order_on(b)?;
        check("D2D A", (cuda.d2d)(out_a, pin.ptr(), BYTES, a as Obj))?;
        check("D2D B", (cuda.d2d)(out_b, other.ptr(), BYTES, b as Obj))?;
    }
    require(
        cuda.pending(a)? && cuda.pending(b)?,
        "consumer bypassed pending producer",
    )?;
    unsafe {
        drop(pin.into_fenced_region(&[a]));
        drop(other.into_fenced_region(&[b]));
    }
    let pending = stats(&arena)?;
    require(
        dropped.load(Ordering::SeqCst) == 0,
        "source owner dropped while GPU reads it",
    )?;
    let reclaimed = arena.reclaim(BYTES)?;
    require(
        reclaimed.reclaimed_bytes == 0,
        "pending producer lost physical ownership",
    )?;
    release_gate(&cuda, gate, control)?;
    cuda.sync(a)?;
    cuda.sync(b)?;
    cuda.verify(out_a)?;
    cuda.verify(out_b)?;
    until(|| Ok(ticket.facts()?.retirement_finished && dropped.load(Ordering::SeqCst) == 1))?;
    require(
        errors.load(Ordering::SeqCst) == 0,
        "native resource destructor failed",
    )?;
    require(
        region.is_resident()? && region.ptr() == ptr,
        "completed fill lost stable reservation",
    )?;
    let completed = stats(&arena)?;
    let reclaimed = arena.reclaim(BYTES)?;
    require(
        reclaimed.reclaimed_bytes >= BYTES,
        "completed consumers did not release backing",
    )?;
    require(
        arena.stats()?.context_joins == 0,
        "ordered native consumers required a context join",
    )?;
    finish_working(&cuda, device, &[gate, out_a, out_b], &[control, a, b])?;
    Ok(
        json!({"published":facts(published),"pending":pending,"completed":completed,"ticket":facts(ticket.facts()?),
        "full_readback_both_consumers":true,"resource_drops":dropped.load(Ordering::SeqCst),"final":stats(&arena)?}),
    )
}
fn abort_fill(cuda: Arc<Cuda>, device: i32) -> Result<Value> {
    let arena = Arena::new(device)?;
    let region = arena.reserve(BYTES)?;
    let ptr = region.ptr();
    let resources = Resources::new(Arc::clone(&cuda))?;
    let dropped = Arc::clone(&resources.dropped);
    let errors = Arc::clone(&resources.errors);
    let control = cuda.stream()?;
    let gate = working::alloc(device, 4, control, None)?;
    unsafe {
        check("clear gate", (cuda.set)(gate, 0, 4))?;
    }
    let raw = fill(&region)?;
    let alias = raw.pin()?;
    let mut session = owned(raw, &resources);
    session.begin()?;
    submit(&cuda, &resources, ptr, gate, true)?;
    let ticket = unsafe {
        session.abort(varena::Error::Invalid(
            "actual producer stopped after first half",
        ))?
    };
    drop(session);
    drop(resources);
    require(
        !ticket.facts()?.published && !ticket.facts()?.completed,
        "aborted fill was published/completed early",
    )?;
    require(
        arena.reclaim(BYTES)?.reclaimed_bytes == 0,
        "pending abort reclaimed backing",
    )?;
    release_gate(&cuda, gate, control)?;
    until(|| Ok(ticket.facts()?.retirement_finished && dropped.load(Ordering::SeqCst) == 1))?;
    require(
        errors.load(Ordering::SeqCst) == 0,
        "aborted native resource retirement failed",
    )?;
    require(
        !region.is_resident()? && region.try_pin()?.is_none(),
        "partial bytes became Ready",
    )?;
    require(
        matches!(region.prepare(), Err(varena::Error::Busy)),
        "escaped failed-fill alias was overwritten",
    )?;
    let mut data = vec![0u8; BYTES / 2];
    unsafe {
        check(
            "partial readback",
            (cuda.read)(data.as_mut_ptr().cast(), alias.ptr(), data.len()),
        )?;
    }
    require(
        data.iter().all(|&byte| byte == 41),
        "escaped failed-fill bytes changed",
    )?;
    let held = stats(&arena)?;
    unsafe {
        drop(alias.into_completed_region());
    }
    require(
        region.release_unready()?,
        "checked aborted pages did not release",
    )?;
    let mut refill = fill(&region)?;
    let mut bytes = vec![41u8; BYTES];
    bytes[BYTES / 2..].fill(73);
    refill.copy_from_host(&bytes)?;
    let pin = refill.commit()?;
    cuda.verify(pin.ptr())?;
    unsafe {
        drop(pin.into_completed_region());
    }
    arena.reclaim(BYTES)?;
    finish_working(&cuda, device, &[gate], &[control])?;
    Ok(
        json!({"aborted":facts(ticket.facts()?),"escaped_alias":held,"stable_pointer":ptr==region.ptr(),"healthy_refill":true,"final":stats(&arena)?}),
    )
}
fn capture_refusal(cuda: Arc<Cuda>, device: i32) -> Result<Value> {
    let arena = Arena::new(device)?;
    let region = arena.reserve(BYTES)?;
    let resources = Resources::new(Arc::clone(&cuda))?;
    let dropped = Arc::clone(&resources.dropped);
    let errors = Arc::clone(&resources.errors);
    let mut session = owned(fill(&region)?, &resources);
    let capture_stream = cuda.stream()?;
    let capture = varena::enter_capture(device)?;
    unsafe {
        check(
            "cuStreamBeginCapture",
            (cuda.capture_begin)(capture_stream as Obj, 0),
        )?;
    }
    require(
        matches!(session.begin(), Err(varena::Error::CaptureActive)),
        "active capture did not refuse before exposure",
    )?;
    let refused = facts(session.facts()?);
    require(
        session.facts()?.state == FillState::Prepared && dropped.load(Ordering::SeqCst) == 0,
        "capture refusal lost session owners",
    )?;
    let mut graph = std::ptr::null_mut();
    unsafe {
        check(
            "cuStreamEndCapture",
            (cuda.capture_end)(capture_stream as Obj, &mut graph),
        )?;
    }
    drop(capture);
    unsafe {
        check("cuGraphDestroy", (cuda.graph_destroy)(graph))?;
        check(
            "cuStreamDestroy",
            (cuda.stream_destroy)(capture_stream as Obj),
        )?;
    }
    session.begin()?;
    require(
        varena::enter_capture(device).is_err(),
        "open producer did not exclude capture",
    )?;
    let ticket = unsafe { session.abort(varena::Error::Invalid("no copy requested after begin"))? };
    drop(session);
    drop(resources);
    until(|| Ok(ticket.facts()?.retirement_finished && dropped.load(Ordering::SeqCst) == 1))?;
    require(
        errors.load(Ordering::SeqCst) == 0,
        "capture retry retirement failed",
    )?;
    let capture = varena::enter_capture(device)?;
    drop(capture);
    Ok(json!({"refused":refused,"retry_abort":facts(ticket.facts()?),"final":stats(&arena)?}))
}
struct PendingScratch {
    cuda: Arc<Cuda>,
    device: i32,
    pointer: Option<u64>,
    stream: Option<u64>,
    producer: Option<varena::ProducerGuard>,
}
// SAFETY: the fixed private stream is the only user of this native allocation. A real
// stream query decides Pending; Complete follows checked stream destruction and memory
// retirement and leaves all owning fields empty. Errors retain every remaining field.
unsafe impl varena::FillResource for PendingScratch {
    fn retire(&mut self) -> varena::Result<varena::FillRetirement> {
        let Some(stream) = self.stream else {
            return Ok(varena::FillRetirement::Complete);
        };
        let status = |operation, code| {
            if code == 0 {
                Ok(())
            } else {
                Err(varena::Error::Cuda { operation, code })
            }
        };
        // SAFETY: this object owns the primary-context stream and allocation; its local
        // producer lease excludes capture until this exact resource is checked complete.
        unsafe {
            status(
                "scratch context enter",
                (self.cuda.push)(self.cuda.context as Obj),
            )?;
            let queried = (self.cuda.stream_query)(stream as Obj);
            let mut old = std::ptr::null_mut();
            let popped = status("scratch context leave", (self.cuda.pop)(&mut old));
            if queried == 600 {
                popped?;
                return Ok(varena::FillRetirement::Pending);
            }
            status("scratch stream query", queried)?;
            popped?;
            status(
                "scratch context enter",
                (self.cuda.push)(self.cuda.context as Obj),
            )?;
            let destroyed = status(
                "scratch stream destroy",
                (self.cuda.stream_destroy)(stream as Obj),
            );
            let popped = status("scratch context leave", (self.cuda.pop)(&mut old));
            destroyed?;
            self.stream = None;
            popped?;
            if let Some(pointer) = self.pointer {
                working::free_completed(self.device, pointer)?;
                self.pointer = None;
            }
        }
        drop(self.producer.take());
        Ok(varena::FillRetirement::Complete)
    }
}
fn retirement_pending(cuda: Arc<Cuda>, device: i32) -> Result<Value> {
    let arena = Arena::new(device)?;
    let region = arena.reserve(BYTES)?;
    let resources = Resources::new(Arc::clone(&cuda))?;
    let dropped = Arc::clone(&resources.dropped);
    let errors = Arc::clone(&resources.errors);
    let control = cuda.stream()?;
    let gate = working::alloc(device, 4, control, None)?;
    let producer = varena::enter_producer(device)?;
    let scratch_stream = cuda.stream()?;
    let scratch = working::alloc(device, BYTES, scratch_stream, None)?;
    // SAFETY: all addresses and streams are owned here. The private write really waits
    // for a device gate; no synthetic busy/refusal or fabricated completion is supplied.
    unsafe {
        check("clear private gate", (cuda.set)(gate, 0, 4))?;
        check(
            "private wait",
            (cuda.wait)(scratch_stream as Obj, gate, 1, 1),
        )?;
        check(
            "private GPU work",
            (cuda.set_async)(scratch, 91, BYTES, scratch_stream as Obj),
        )?;
    }
    let pending = PendingScratch {
        cuda: Arc::clone(&cuda),
        device,
        pointer: Some(scratch),
        stream: Some(scratch_stream),
        producer: Some(producer),
    };
    // SAFETY: fixed fill writers retain native resources; private scratch has its own
    // actual consumer completion and producer exclusion through checked retirement.
    let mut session = unsafe {
        fill(&region)?.into_async(FillOwners::new(
            resources.streams.clone(),
            vec![
                Box::new(pending),
                Box::new(NativeResources(Some(Arc::clone(&resources)))),
            ],
        ))
    };
    let destination = session.begin()?;
    // SAFETY: full disjoint halves are covered by the fixed producer streams; source
    // resources stay owned by Session after the caller's Arc is dropped below.
    unsafe {
        check(
            "pending case first half",
            (cuda.copy)(
                destination.ptr,
                resources.host as *const c_void,
                BYTES / 2,
                resources.streams[0] as Obj,
            ),
        )?;
        check(
            "pending case second half",
            (cuda.copy)(
                destination.ptr + (BYTES / 2) as u64,
                (resources.host + BYTES / 2) as *const c_void,
                BYTES / 2,
                resources.streams[1] as Obj,
            ),
        )?;
    }
    // SAFETY: these are the final writes; every logical byte was submitted above.
    let ticket = unsafe { session.publish()? };
    drop(session);
    drop(resources);
    until(|| {
        Ok(ticket.facts()?.completed
            && dropped.load(Ordering::SeqCst) == 1
            && ticket.facts()?.resources_remaining == 1)
    })?;
    let pending_facts = ticket.facts()?;
    require(
        !pending_facts.retirement_finished && pending_facts.retirement_failure.is_none(),
        "pending resource was treated as retired or failed",
    )?;
    require(
        region.is_resident()? && working::stats(device)?.allocated_bytes >= BYTES + 4,
        "pending private storage was freed",
    )?;
    require(
        varena::enter_capture(device).is_err(),
        "pending resource lost capture exclusion",
    )?;
    release_gate(&cuda, gate, control)?;
    until(|| Ok(ticket.facts()?.retirement_finished))?;
    require(
        errors.load(Ordering::SeqCst) == 0,
        "checked source retirement failed",
    )?;
    let pin = region.pin()?;
    cuda.verify(pin.ptr())?;
    // SAFETY: the only reader was the synchronous full readback, which completed.
    unsafe {
        drop(pin.into_completed_region());
    }
    arena.reclaim(BYTES)?;
    finish_working(&cuda, device, &[gate], &[control])?;
    Ok(
        json!({"pending":facts(pending_facts),"retired":facts(ticket.facts()?),
        "later_resource_retired_while_first_pending":true,"full_readback":true,"final":stats(&arena)?}),
    )
}

fn abandoned(cuda: Arc<Cuda>, device: i32) -> Result<Value> {
    let arena = Arena::new(device)?;
    let region = arena.reserve(BYTES)?;
    let resources = Resources::new(Arc::clone(&cuda))?;
    let dropped = Arc::clone(&resources.dropped);
    let mut session = owned(fill(&region)?, &resources);
    let destination = session.begin()?;
    unsafe {
        check(
            "first half",
            (cuda.copy)(
                destination.ptr,
                resources.host as *const c_void,
                BYTES / 2,
                resources.streams[0] as Obj,
            ),
        )?;
        check(
            "second half",
            (cuda.copy)(
                destination.ptr + (BYTES / 2) as u64,
                (resources.host + BYTES / 2) as *const c_void,
                BYTES / 2,
                resources.streams[1] as Obj,
            ),
        )?;
    }
    cuda.sync(resources.streams[0])?;
    cuda.sync(resources.streams[1])?;
    drop(resources);
    drop(session);
    // The core was never told submissions were closed/completed, even though this fixture
    // knows its own streams are now done. It must not invent that proof from Drop.
    require(
        arena.stats()?.poisoned && arena.stats()?.pinned_bytes >= BYTES,
        "unsealed abandonment was not quarantined",
    )?;
    require(
        dropped.load(Ordering::SeqCst) == 0,
        "unsealed owners were freed",
    )?;
    cuda.verify(destination.ptr)?;
    Ok(
        json!({"retained":stats(&arena)?,"resource_drops":0,"actual_full_readback":true,"scope":"intentional quarantine, process exit reclaims it; no cleanup success claim"}),
    )
}
fn run() -> Result<Value> {
    let args: Vec<_> = std::env::args().collect();
    let mode = args.get(1).ok_or("mode required")?;
    let expected = args.get(2).ok_or("actual GPU UUID required")?;
    let device = args.get(3).map_or(Ok(0), |s| s.parse::<i32>())?;
    let cuda = Cuda::open(device)?;
    let uuid = expected
        .trim_start_matches("GPU-")
        .replace('-', "")
        .to_lowercase();
    require(uuid == cuda.uuid, "wrong GPU")?;
    let maps = std::fs::read_to_string("/proc/self/maps")?;
    require(
        !maps.contains("libtorch") && !maps.contains("libpython"),
        "consumer links Python or Torch",
    )?;
    let detail = match mode.as_str() {
        "ordering" => ordering(Arc::clone(&cuda), device)?,
        "abort" => abort_fill(Arc::clone(&cuda), device)?,
        "capture" => capture_refusal(Arc::clone(&cuda), device)?,
        "abandoned" => abandoned(Arc::clone(&cuda), device)?,
        "retirement_pending" => retirement_pending(Arc::clone(&cuda), device)?,
        _ => return Err("unknown mode".into()),
    };
    Ok(
        json!({"passed":true,"mode":mode,"pid":std::process::id(),"gpu_uuid":cuda.uuid,"native_only":true,"detail":detail}),
    )
}
fn main() {
    match run() {
        Ok(result) => println!("{result}"),
        Err(error) => {
            eprintln!("{}", json!({"passed":false,"error":error.to_string()}));
            std::process::exit(1)
        }
    }
}
