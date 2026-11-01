//! Real native consumer; all CUDA pointer access is confined to this release-gate process.
use libloading::Library;
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    ffi::c_void,
    io::{self, BufRead, Write},
    path::PathBuf,
    time::{Duration, Instant},
};
use varena::{cooperative, ledger, working};

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
        let mut data = vec![0u8; block.bytes];
        // SAFETY: destination owns bytes, and the source remains live in our allocation map.
        check("cuMemcpyDtoH", unsafe {
            (self.read)(data.as_mut_ptr().cast(), block.ptr, block.bytes)
        })?;
        if data.iter().any(|byte| *byte != block.value) {
            return Err(io::Error::other("native working allocation contents changed").into());
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
enum Service {
    Auto(cooperative::Owner),
    Manual(cooperative::Peer),
    None,
}

fn row(id: ledger::RowId) -> Value {
    json!({"slot":id.slot,"incarnation":id.incarnation})
}
fn libraries() -> Result<Value> {
    let maps = std::fs::read_to_string("/proc/self/maps")?;
    Ok(
        json!({"python_loaded":maps.contains("libpython"),"torch_loaded":maps.contains("libtorch"),"cuda_loaded":maps.contains("libcuda.so")}),
    )
}
fn emit(value: Value) -> Result<()> {
    println!("{value}");
    io::stdout().flush()?;
    Ok(())
}
fn number(value: &Value, key: &str) -> Result<usize> {
    value[key]
        .as_u64()
        .and_then(|n| usize::try_from(n).ok())
        .ok_or_else(|| io::Error::other(format!("missing/invalid {key}")).into())
}
fn stats(device: i32) -> Result<Value> {
    let s = working::stats(device)?;
    Ok(
        json!({"live":s.allocated_bytes,"capacity":s.allocated_capacity,"mapped":s.mapped_bytes,
        "free":s.free_bytes,"pending":s.pending_bytes,"pools":s.pools,"driver_calls":s.driver_memory_calls,
        "driver_physical_ooms":s.driver_physical_ooms,"ooms":s.ooms}),
    )
}
fn facts(service: &Service) -> Result<Value> {
    match service {
        Service::Auto(owner) => {
            let f = owner.facts()?;
            Ok(
                json!({"active":f.active,"recoveries":f.recoveries,"failed":f.failed,
                "last_recovery":f.last_recovery.map(|r|json!({"outcome":r.outcome.as_str(),
                    "requested_bytes":r.requested_bytes,"released_bytes":r.released_bytes,
                    "round":r.round,"completed_bytes":r.completed_bytes,"elapsed_ns":r.elapsed_ns}))}),
            )
        }
        _ => Ok(Value::Null),
    }
}

fn command(
    value: &Value,
    device: i32,
    cuda: &Cuda,
    service: &Service,
    blocks: &mut BTreeMap<String, Block>,
) -> Result<Value> {
    let name = value["name"].as_str().unwrap_or("block");
    match value["op"].as_str().unwrap_or("") {
        "allocate" => {
            if blocks.contains_key(name) {
                return Err(io::Error::other("allocation name already live").into());
            }
            let bytes = number(value, "bytes")?;
            if bytes == 0 {
                return Err(io::Error::other("fixture allocations must be nonempty").into());
            }
            let byte = value["value"].as_u64().unwrap_or(41);
            if byte > 255 {
                return Err(io::Error::other("invalid fill byte").into());
            }
            let ptr = working::alloc(device, bytes, 0, None)?;
            blocks.insert(
                name.to_owned(),
                Block {
                    ptr,
                    bytes,
                    value: byte as u8,
                },
            );
            cuda.fill(ptr, bytes, byte as u8)?;
            Ok(json!({"ptr":ptr,"bytes":bytes,"value":byte}))
        }
        "verify" => {
            let block = blocks
                .get(name)
                .ok_or_else(|| io::Error::other("allocation is not live"))?;
            cuda.verify(block)?;
            Ok(json!({"bytes_verified":block.bytes}))
        }
        "free" => {
            let block = blocks
                .get(name)
                .ok_or_else(|| io::Error::other("allocation is not live"))?;
            cuda.synchronize()?;
            working::free(device, block.ptr, 0)?;
            blocks.remove(name);
            Ok(json!({"freed":name}))
        }
        "trim" => {
            cuda.synchronize()?;
            Ok(json!({"released":working::trim(device)?}))
        }
        "publish" => match service {
            Service::Manual(peer) => {
                let a = peer.publish(0)?;
                Ok(
                    json!({"physical":a.physical.total()?,"reclaimable":a.reclaimable,"refreshed":a.refreshed}),
                )
            }
            _ => Err(io::Error::other("publish belongs only to the manual donor").into()),
        },
        "service" => match service {
            Service::Manual(peer) => {
                let a = peer.service()?;
                Ok(json!({"round":a.round,"released":a.released,"deferred":a.deferred}))
            }
            _ => Err(io::Error::other("service belongs only to the manual donor").into()),
        },
        "stats" => Ok(Value::Null),
        _ => Err(io::Error::other("unknown native fixture operation").into()),
    }
}

fn consumer(
    mode: &str,
    directory: &std::path::Path,
    descriptor: ledger::Device,
    device: i32,
) -> Result<()> {
    let cuda = Cuda::open(device)?;
    working::initialize(device)?;
    let service = match mode {
        "auto" => Service::Auto(cooperative::Owner::open(
            device,
            descriptor,
            directory,
            ledger::Sharing::Shared,
            10,
        )?),
        "manual" => Service::Manual(cooperative::Peer::open(
            device,
            descriptor,
            directory,
            ledger::Sharing::Shared,
            0,
        )?),
        "plain" => Service::None,
        _ => return Err(io::Error::other("unknown consumer mode").into()),
    };
    let id = match &service {
        Service::Auto(x) => Some(row(x.id())),
        Service::Manual(x) => Some(row(x.id())),
        Service::None => None,
    };
    emit(
        json!({"event":"ready","pid":std::process::id(),"mode":mode,"row":id,"libraries":libraries()?,"stats":stats(device)?,"driver":cuda.memory()?}),
    )?;
    let mut blocks = BTreeMap::new();
    for line in io::stdin().lock().lines() {
        let value: Value = serde_json::from_str(&line?)?;
        if value["op"] == "quit" {
            break;
        }
        let start = Instant::now();
        let result = command(&value, device, &cuda, &service, &mut blocks);
        let (ok, result, error) = match result {
            Ok(x) => (true, x, Value::Null),
            Err(e) => (false, Value::Null, Value::String(e.to_string())),
        };
        emit(
            json!({"event":"reply","ok":ok,"result":result,"error":error,"elapsed_ns":start.elapsed().as_nanos().min(u64::MAX as u128) as u64,"stats":stats(device)?,"driver":cuda.memory()?,"owner":facts(&service)?}),
        )?;
    }
    cuda.synchronize()?;
    for block in blocks.into_values() {
        working::free(device, block.ptr, 0)?;
    }
    cuda.synchronize()?;
    working::trim(device)?;
    match service {
        Service::Auto(owner) => owner.close()?,
        Service::Manual(peer) => match peer.retire() {
            Ok(retired) => drop(retired),
            Err((peer, error)) => {
                std::mem::forget(peer);
                return Err(error.into());
            }
        },
        Service::None => {}
    }
    emit(json!({"event":"closed","stats":stats(device)?,"driver":cuda.memory()?}))
}

fn watcher(directory: &std::path::Path, device: ledger::Device, expected: &Value) -> Result<()> {
    let map = ledger::Ledger::open_nonblocking(directory, device)?;
    let donor = ledger::RowId {
        slot: number(expected, "donor_slot")?,
        incarnation: expected["donor_incarnation"]
            .as_u64()
            .ok_or_else(|| io::Error::other("missing incarnation"))?,
    };
    let requester = ledger::RowId {
        slot: number(expected, "requester_slot")?,
        incarnation: expected["requester_incarnation"]
            .as_u64()
            .ok_or_else(|| io::Error::other("missing requester incarnation"))?,
    };
    let deadline = Instant::now() + Duration::from_millis(number(expected, "timeout_ms")? as u64);
    loop {
        match map.snapshot() {
            Ok(snapshot) => {
                if let Some(round) = snapshot
                    .round
                    .filter(|r| r.requester == requester && !r.aborted)
                    && let Some(donation) = round
                        .donations
                        .iter()
                        .find(|d| d.owner == donor && d.answer == ledger::Answer::Waiting)
                    && let Some(owner) = snapshot.rows.iter().find(|r| r.id == donor)
                {
                    emit(
                        json!({"event":"donor_waiting","round":round.generation,"donor":row(donor),"requester":row(requester),"pid":owner.pid,"start_ticks":owner.start_ticks,"completed":donation.completed,"pending":donation.pending,"libraries":libraries()?,"read_only":true}),
                    )?;
                    return Ok(());
                }
            }
            Err(ledger::Error::Busy) => {}
            Err(error) => return Err(error.into()),
        }
        if Instant::now() >= deadline {
            return Err(io::Error::other(
                "waiting-state observation deadline expired; no death inferred",
            )
            .into());
        }
        std::thread::sleep(Duration::from_millis(1));
    }
}

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 5 {
        return Err(io::Error::other(
            "usage: consumer MODE DIRECTORY GPU_UUID DEVICE [WATCH_JSON]",
        )
        .into());
    }
    ledger::initialize()?;
    let directory = PathBuf::from(&args[2]);
    let descriptor = ledger::Device::parse(&args[3])?;
    let device: i32 = args[4].parse()?;
    if args[1] == "watch" {
        return watcher(
            &directory,
            descriptor,
            &serde_json::from_str(
                args.get(5)
                    .ok_or_else(|| io::Error::other("missing watcher request"))?,
            )?,
        );
    }
    consumer(&args[1], &directory, descriptor, device)
}
