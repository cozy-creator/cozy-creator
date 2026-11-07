//! Actual public Rust Source provider. GPU mode is UUID-bound; no Python/Torch.
use libloading::Library;
use std::{
    ffi::c_void,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::{Duration, Instant},
};
use varena_client::{Error, Gpu, Source, SourceJob, SourceRetirement, Status, Stream};
type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;
type Obj = *mut c_void;
const BYTES: u64 = 1 << 20;
struct Cuda {
    _library: Library,
    create: unsafe extern "system" fn(*mut Obj, u32) -> i32,
    destroy: unsafe extern "system" fn(Obj) -> i32,
    sync: unsafe extern "system" fn(Obj) -> i32,
    query: unsafe extern "system" fn(Obj) -> i32,
    wait: unsafe extern "system" fn(Obj, u64, u32, u32) -> i32,
    write: unsafe extern "system" fn(Obj, u64, u32, u32) -> i32,
    set: unsafe extern "system" fn(u64, u8, usize, Obj) -> i32,
    copy: unsafe extern "system" fn(u64, u64, usize, Obj) -> i32,
    read: unsafe extern "system" fn(*mut c_void, u64, usize) -> i32,
    host_allocate: unsafe extern "system" fn(*mut Obj, usize, u32) -> i32,
    host_device: unsafe extern "system" fn(*mut u64, Obj, u32) -> i32,
    host_free: unsafe extern "system" fn(Obj) -> i32,
    uuid: String,
}
fn check(operation: &str, status: i32) -> varena_client::Result<()> {
    if status == 0 {
        Ok(())
    } else {
        Err(Error {
            status: Status::Fault,
            message: format!("{operation}:CUDA{status}"),
            requested_bytes: 0,
        })
    }
}
impl Cuda {
    fn open(device: i32, expected: &str) -> Result<Arc<Self>> {
        unsafe {
            let library = Library::new("libcuda.so.1")?;
            check(
                "init",
                library.get::<unsafe extern "system" fn(u32) -> i32>(b"cuInit\0")?(0),
            )?;
            let mut context = std::ptr::null_mut();
            check(
                "retain",
                library.get::<unsafe extern "system" fn(*mut Obj, i32) -> i32>(
                    b"cuDevicePrimaryCtxRetain\0",
                )?(&mut context, device),
            )?;
            check(
                "push",
                library.get::<unsafe extern "system" fn(Obj) -> i32>(b"cuCtxPushCurrent_v2\0")?(
                    context,
                ),
            )?;
            let mut uuid = [0u8; 16];
            check(
                "uuid",
                library.get::<unsafe extern "system" fn(*mut u8, i32) -> i32>(
                    b"cuDeviceGetUuid_v2\0",
                )?(uuid.as_mut_ptr(), device),
            )?;
            let uuid = uuid.iter().map(|b| format!("{b:02x}")).collect::<String>();
            if uuid
                != expected
                    .trim_start_matches("GPU-")
                    .replace('-', "")
                    .to_lowercase()
            {
                return Err("wrong GPU UUID".into());
            }
            Ok(Arc::new(Self {
                create: *library.get(b"cuStreamCreate\0")?,
                destroy: *library.get(b"cuStreamDestroy_v2\0")?,
                sync: *library.get(b"cuStreamSynchronize\0")?,
                query: *library.get(b"cuStreamQuery\0")?,
                wait: *library.get(b"cuStreamWaitValue32_v2\0")?,
                write: *library.get(b"cuStreamWriteValue32_v2\0")?,
                set: *library.get(b"cuMemsetD8Async\0")?,
                copy: *library.get(b"cuMemcpyDtoDAsync_v2\0")?,
                read: *library.get(b"cuMemcpyDtoH_v2\0")?,
                host_allocate: *library.get(b"cuMemHostAlloc\0")?,
                host_device: *library.get(b"cuMemHostGetDevicePointer_v2\0")?,
                host_free: *library.get(b"cuMemFreeHost\0")?,
                _library: library,
                uuid,
            }))
        }
    }
    fn stream(&self) -> varena_client::Result<u64> {
        let mut stream = std::ptr::null_mut();
        unsafe {
            check("stream", (self.create)(&mut stream, 1))?;
        }
        Ok(stream as u64)
    }
    fn sync(&self, stream: u64) -> varena_client::Result<()> {
        unsafe { check("stream completion", (self.sync)(stream as Obj)) }
    }
}
#[derive(Default)]
struct Counts {
    jobs: AtomicUsize,
    sources: AtomicUsize,
}
struct Provider {
    cuda: Arc<Cuda>,
    gpu: Gpu,
    gate: u64,
    counts: Arc<Counts>,
    retired: bool,
}
struct Producer {
    cuda: Arc<Cuda>,
    stream: Option<u64>,
    views: Vec<Stream>,
    gate: u64,
    counts: Arc<Counts>,
}
// SAFETY: Source is CPU-only until prepare, owns its library/Gpu, and returns an exclusive
// job with its actual created nonblocking stream. It has no hidden host submitter.
unsafe impl Source for Provider {
    type Job = Producer;
    unsafe fn prepare(
        &self,
        device: i32,
        _offset: u64,
        _bytes: u64,
    ) -> varena_client::Result<Producer> {
        if device != self.gpu.device() {
            return Err(Error {
                status: Status::Invalid,
                message: "wrong Source device".into(),
                requested_bytes: 0,
            });
        }
        let stream = self.cuda.stream()?;
        Ok(Producer {
            cuda: self.cuda.clone(),
            stream: Some(stream),
            views: vec![unsafe { self.gpu.borrow_stream(stream) }],
            gate: self.gate,
            counts: self.counts.clone(),
        })
    }
    unsafe fn retire(&mut self) -> varena_client::Result<SourceRetirement> {
        self.retired = true;
        Ok(SourceRetirement::Complete)
    }
}
impl Drop for Provider {
    fn drop(&mut self) {
        if self.retired {
            self.counts.sources.fetch_add(1, Ordering::Release);
        }
    }
}
// SAFETY: every destination write uses this one retained stream; submit returns closed.
// Checked native completion precedes retire. Drop never destroys an unreleased GPU owner.
unsafe impl SourceJob for Producer {
    fn streams(&self) -> &[Stream] {
        &self.views
    }
    unsafe fn submit(&mut self, dst: u64, bytes: u64) -> varena_client::Result<()> {
        let stream = self.stream.expect("live producer");
        unsafe {
            check(
                "actual producer gate",
                (self.cuda.wait)(stream as Obj, self.gate, 1, 1),
            )?;
            check(
                "actual whole fill",
                (self.cuda.set)(dst, 117, bytes as usize, stream as Obj),
            )?;
        }
        Ok(())
    }
    unsafe fn retire(&mut self) -> varena_client::Result<SourceRetirement> {
        if let Some(stream) = self.stream {
            unsafe {
                check(
                    "destroy checked producer",
                    (self.cuda.destroy)(stream as Obj),
                )?;
            }
            self.stream = None;
        }
        self.views.clear();
        Ok(SourceRetirement::Complete)
    }
}
impl Drop for Producer {
    fn drop(&mut self) {
        if self.stream.is_none() {
            self.counts.jobs.fetch_add(1, Ordering::Release);
        } else {
            std::mem::forget(self.cuda.clone());
        }
    }
}
fn require(value: bool, why: &str) -> Result<()> {
    if value { Ok(()) } else { Err(why.into()) }
}
fn run() -> Result<()> {
    let args = std::env::args().collect::<Vec<_>>();
    if args.len() != 3 {
        return Err("usage: source GPU-UUID DEVICE".into());
    }
    let device = args[2].parse()?;
    let cuda = Cuda::open(device, &args[1])?;
    let gpu = Gpu::open(device)?;
    let control = cuda.stream()?;
    let consumer = cuda.stream()?;
    let control_view = unsafe { gpu.borrow_stream(control) };
    let consumer_view = unsafe { gpu.borrow_stream(consumer) };
    // Retained mapped host gate: Source-owned streams may be destroyed before its
    // release, so this is not a Working allocation with unrecorded borrowed consumers.
    let mut gate_host = std::ptr::null_mut();
    let mut gate = 0;
    unsafe {
        check(
            "mapped host gate",
            (cuda.host_allocate)(&mut gate_host, 4, 3),
        )?;
        check(
            "gate device address",
            (cuda.host_device)(&mut gate, gate_host, 0),
        )?;
    }
    let mut output = gpu.alloc(BYTES, &consumer_view)?;
    unsafe {
        check("gate init", (cuda.write)(control as Obj, gate, 0, 0))?;
    }
    cuda.sync(control)?;
    let counts = Arc::new(Counts::default());
    let source = Provider {
        cuda: cuda.clone(),
        gpu: gpu.clone(),
        gate: gate,
        counts: counts.clone(),
        retired: false,
    };
    let region = match gpu.region_from_source(BYTES, source) {
        Ok(region) => region,
        Err((source, error)) => {
            std::mem::forget(source);
            return Err(error.into());
        }
    };
    let mut used = gpu.use_regions(&[&region], &consumer_view)?;
    unsafe {
        check(
            "ordered full consumer",
            (cuda.copy)(output.ptr()?, used.ptr(0)?, BYTES as usize, consumer as Obj),
        )?;
    }
    require(
        unsafe { (cuda.query)(consumer as Obj) } == 600,
        "consumer bypassed producer gate",
    )?;
    require(
        counts.jobs.load(Ordering::Acquire) == 0,
        "producer dropped before completion",
    )?;
    used.done()?;
    drop(used);
    drop(region);
    require(
        varena_client::retirement_stats()?.queued != 0,
        "pending Source was invisible to retirement stats",
    )?;
    unsafe {
        check("open gate", (cuda.write)(control as Obj, gate, 1, 0))?;
    }
    cuda.sync(control)?;
    cuda.sync(consumer)?;
    let mut bytes = vec![0u8; BYTES as usize];
    unsafe {
        check(
            "full readback",
            (cuda.read)(bytes.as_mut_ptr().cast(), output.ptr()?, bytes.len()),
        )?;
    }
    require(
        bytes.iter().all(|&byte| byte == 117),
        "Source consumer bytes differ",
    )?;
    let deadline = Instant::now() + Duration::from_secs(10);
    while counts.sources.load(Ordering::Acquire) != 1 {
        require(
            Instant::now() < deadline,
            "Source retirement protocol deadline",
        )?;
        require(
            varena_client::retirement_stats()?.quarantined == 0,
            "Source retirement quarantined",
        )?;
        std::thread::sleep(Duration::from_millis(1));
    }
    require(
        counts.jobs.load(Ordering::Acquire) == 1,
        "job drop count differs",
    )?;
    output.free()?;
    cuda.sync(consumer)?;
    cuda.sync(control)?;
    let stats = gpu.stats()?;
    require(
        stats.working_live == 0,
        "physical baseline differs",
    )?;
    drop((output, consumer_view, control_view, gpu));
    let deadline = Instant::now() + Duration::from_secs(10);
    while varena_client::retirement_stats()?.queued != 0 {
        require(
            Instant::now() < deadline,
            "final native retirement deadline",
        )?;
        std::thread::sleep(Duration::from_millis(1));
    }
    unsafe {
        check("free checked host gate", (cuda.host_free)(gate_host))?;
        check("destroy consumer", (cuda.destroy)(consumer as Obj))?;
        check("destroy control", (cuda.destroy)(control as Obj))?;
    }
    println!(
        "{{\"passed\":true,\"gpu_uuid\":\"{}\",\"full_readback\":true,\"job_drops\":1,\"source_drops\":1}}",
        cuda.uuid
    );
    Ok(())
}
fn main() {
    if let Err(error) = run() {
        eprintln!("{error}");
        std::process::exit(1);
    }
}
