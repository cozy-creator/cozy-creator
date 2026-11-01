//! Genuine standalone Varena Cache proof. This process loads neither Python nor Torch.
use libloading::Library;
use serde_json::{Value, json};
use std::{
    ffi::c_void,
    io::{self, Write},
    time::{Duration, Instant},
};
use varena::{Cache, cooperative, ledger, working};
mod probes;
mod multi_plan;
type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;
type Ctx = *mut c_void;
type Retain = unsafe extern "C" fn(*mut Ctx, i32) -> i32;
type Push = unsafe extern "C" fn(Ctx) -> i32;
type Pop = unsafe extern "C" fn(*mut Ctx) -> i32;
type Release = unsafe extern "C" fn(i32) -> i32;
type Info = unsafe extern "C" fn(*mut usize, *mut usize) -> i32;
type Set = unsafe extern "C" fn(u64, u8, usize) -> i32;
type Read = unsafe extern "C" fn(*mut c_void, u64, usize) -> i32;
type Sync = unsafe extern "C" fn() -> i32;

struct Cuda {
    _library: Library,
    device: i32,
    pop: Pop,
    release: Release,
    info: Info,
    set: Set,
    read: Read,
    sync: Sync,
}

fn check(name: &str, code: i32) -> Result<()> {
    if code != 0 {
        return Err(io::Error::other(format!("{name}: CUDA status {code}")).into());
    }
    Ok(())
}

impl Cuda {
    fn open(device: i32) -> Result<Self> {
        // SAFETY: these exact CUDA Driver ABI signatures are used only while Library remains live.
        unsafe {
            let library = Library::new("libcuda.so.1")?;
            let init = *library.get::<unsafe extern "C" fn(u32) -> i32>(b"cuInit\0")?;
            check("cuInit", init(0))?;
            let retain = *library.get::<Retain>(b"cuDevicePrimaryCtxRetain\0")?;
            let push = *library.get::<Push>(b"cuCtxPushCurrent_v2\0")?;
            let pop = *library.get::<Pop>(b"cuCtxPopCurrent_v2\0")?;
            let release = *library.get::<Release>(b"cuDevicePrimaryCtxRelease_v2\0")?;
            let info = *library.get::<Info>(b"cuMemGetInfo_v2\0")?;
            let set = *library.get::<Set>(b"cuMemsetD8_v2\0")?;
            let read = *library.get::<Read>(b"cuMemcpyDtoH_v2\0")?;
            let sync = *library.get::<Sync>(b"cuCtxSynchronize\0")?;
            let mut context = std::ptr::null_mut();
            check("cuDevicePrimaryCtxRetain", retain(&mut context, device))?;
            if let Err(error) = check("cuCtxPushCurrent", push(context)) {
                let _ = release(device);
                return Err(error);
            }
            Ok(Self {
                _library: library,
                device,
                pop,
                release,
                info,
                set,
                read,
                sync,
            })
        }
    }
    fn memory(&self) -> Result<Value> {
        let (mut free, mut total) = (0, 0);
        // SAFETY: outputs are valid and this thread retains/pushed the device primary context.
        check("cuMemGetInfo", unsafe {
            (self.info)(&mut free, &mut total)
        })?;
        Ok(json!({"free":free,"total":total}))
    }
    fn synchronize(&self) -> Result<()> {
        // SAFETY: this thread owns a current primary context.
        check("cuCtxSynchronize", unsafe { (self.sync)() })
    }
    fn fill(&self, ptr: u64, bytes: usize, value: u8) -> Result<()> {
        // SAFETY: ptr/bytes name the exact live allocation just returned by Varena.
        check("cuMemsetD8", unsafe { (self.set)(ptr, value, bytes) })?;
        self.synchronize()
    }
    fn verify(&self, block: &Block) -> Result<()> {
        self.synchronize()?;
        // Read EVERY requested byte without allocating a GPU-sized host copy.
        let mut data = vec![0u8; block.bytes.min(8 << 20)];
        for offset in (0..block.bytes).step_by(data.len()) {
            let count = data.len().min(block.bytes - offset);
            let address = block
                .ptr
                .checked_add(offset as u64)
                .ok_or_else(|| io::Error::other("readback pointer overflow"))?;
            // SAFETY: destination owns count bytes; the source allocation remains live.
            check("cuMemcpyDtoH", unsafe {
                (self.read)(data.as_mut_ptr().cast(), address, count)
            })?;
            if data[..count].iter().any(|byte| *byte != block.value) {
                return Err(io::Error::other(format!(
                    "native allocation changed at readback window {offset}"
                ))
                .into());
            }
        }
        Ok(())
    }
}

impl Drop for Cuda {
    fn drop(&mut self) {
        // SAFETY: this releases only this helper's balanced current-context/primary reference.
        unsafe {
            let mut popped = std::ptr::null_mut();
            let _ = (self.pop)(&mut popped);
            let _ = (self.release)(self.device);
        }
    }
}

struct Block {
    ptr: u64,
    bytes: usize,
    value: u8,
}
fn libraries() -> Result<Value> {
    let maps = std::fs::read_to_string("/proc/self/maps")?;
    if maps.contains("libpython") || maps.contains("libtorch") {
        return Err(io::Error::other("standalone native process loaded Python or Torch").into());
    }
    Ok(
        json!({"python_loaded":maps.contains("libpython"),"torch_loaded":maps.contains("libtorch"),"cuda_loaded":maps.contains("libcuda.so")}),
    )
}
fn start_ticks() -> Result<u64> {
    let stat = std::fs::read_to_string("/proc/self/stat")?;
    let (_, tail) = stat
        .rsplit_once(") ")
        .ok_or_else(|| io::Error::other("invalid self stat"))?;
    Ok(tail
        .split_whitespace()
        .nth(19)
        .ok_or_else(|| io::Error::other("missing self birth"))?
        .parse()?)
}

fn require(ok: bool, reason: &str) -> Result<()> {
    if !ok {
        return Err(io::Error::other(reason).into());
    }
    Ok(())
}
fn emit(value: &Value) -> Result<()> {
    println!("{value}");
    io::stdout().flush()?;
    Ok(())
}
fn working_stats(device: i32) -> Result<Value> {
    let s = working::stats(device)?;
    Ok(
        json!({"live":s.allocated_bytes,"capacity":s.allocated_capacity,"mapped":s.mapped_bytes,
        "pending":s.pending_bytes,"pools":s.pools,"events":s.event_handles,
        "cache_reserved":s.cache_reserved_bytes,"cache_mapped":s.cache_mapped_bytes,
        "cache_live":s.cache_live_bytes,"cache_pending":s.cache_pending_bytes,
        "cache_ready":s.cache_ready_bytes,"caches":s.caches,
        "memory_calls":s.driver_memory_calls,"physical_ooms":s.driver_physical_ooms}),
    )
}
fn cache_stats(cache: &Cache) -> Result<Value> {
    let s = cache.stats()?;
    Ok(
        json!({"reserved":s.reserved_bytes,"mapped":s.mapped_bytes,"live":s.live_bytes,
        "pending":s.pending_bytes,"ready":s.ready_bytes}),
    )
}
fn accounting(peer: &cooperative::Peer) -> Result<Value> {
    let r = peer.publish(0)?;
    Ok(
        json!({"cache_live":r.physical.caches.live,"cache_pending":r.physical.caches.pending,
        "cache_idle":r.physical.caches.idle,"physical":r.physical.total()?,
        "weight_live":r.physical.weights.live,"weight_pending":r.physical.weights.pending,
        "weight_idle":r.physical.weights.idle,
        "reclaimable":r.reclaimable,"pending_release":r.pending_release,"refreshed":r.refreshed}),
    )
}
fn run() -> Result<Value> {
    let args = std::env::args().collect::<Vec<_>>();
    require(
        args.len() == 5,
        "expected mode, private tmpfs directory, GPU UUID and device",
    )?;
    ledger::initialize()?;
    let device = args[4].parse::<i32>()?;
    let cuda = Cuda::open(device)?;
    let peer = cooperative::Peer::open(
        device,
        ledger::Device::parse(&args[3])?,
        std::path::Path::new(&args[2]),
        ledger::Sharing::Shared,
        0,
    )?;
    let baseline = working_stats(device)?;
    require(
        baseline["live"] == 0 && baseline["mapped"] == 0 && baseline["caches"] == 0,
        "fresh native baseline",
    )?;
    emit(
        &json!({"event":"ready","pid":std::process::id(),"start_ticks":start_ticks()?,
        "mode":args[1],"gpu_uuid":args[3],"libraries":libraries()?,"baseline":baseline,"driver":cuda.memory()?}),
    )?;
    let detail = match args[1].as_str() {
        "pages" => probes::pages(&cuda, &peer, device)?,
        "rollback" => probes::rollback(&cuda, &peer, device)?,
        "abandoned" => probes::abandoned(&cuda, &peer, device)?,
        "choice" => probes::choice(&cuda, &peer, device)?,
        "choice_plan" => probes::choice_plan(&cuda, &peer, device)?,
        "multi_plan" => multi_plan::same_device(&cuda, &peer, device)?,
        "multi_plan_cross_device" => multi_plan::mismatched_device(device)?,
        "owners" => probes::owners(&cuda, &peer, device)?,
        "arena_concurrency" => probes::arena_concurrency(&peer, device)?,
        _ => return Err(io::Error::other("unknown Cache proof mode").into()),
    };
    cuda.synchronize()?;
    working::trim(device)?;
    let final_stats = working_stats(device)?;
    require(
        final_stats["live"] == 0
            && final_stats["mapped"] == 0
            && final_stats["pending"] == 0
            && final_stats["pools"] == 0
            && final_stats["events"] == 0,
        "ordinary working cleanup",
    )?;
    if args[1] != "abandoned" {
        require(
            final_stats["cache_mapped"] == 0
                && final_stats["cache_reserved"] == 0
                && final_stats["caches"] == 0,
            "Cache cleanup",
        )?;
    }
    let final_accounting = accounting(&peer)?;
    if let Err((peer, error)) = peer.try_close() {
        std::mem::forget(peer);
        return Err(error.into());
    }
    Ok(
        json!({"event":"result","passed":true,"mode":args[1],"pid":std::process::id(),
        "gpu_uuid":args[3],"libraries":libraries()?,"baseline":baseline,"detail":detail,
        "final":final_stats,"final_accounting":final_accounting,"driver":cuda.memory()?}),
    )
}
fn main() {
    match run() {
        Ok(result) => {
            if emit(&result).is_err() {
                std::process::exit(2)
            }
        }
        Err(error) => {
            let _ = emit(
                &json!({"event":"result","passed":false,"error":error.to_string(),"pid":std::process::id()}),
            );
            std::process::exit(1)
        }
    }
}
