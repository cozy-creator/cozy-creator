//! Real CUDA memory cells. Built by native-memory.sh against the candidate's native crates.
//! The 64 MiB cap is local to each probe executor, not a machine-wide GPU quota.
use std::{sync::Arc, time::{Duration, Instant}};
use serde_json::{json, Value};
use varena::{Backend, CudaBackend, Error, Event, Fill, Fingerprint, Level, Memory, RegionKind, Result, SetSpec, Va};
use varena::layout::{Item, ItemSource, Layout, Region};
use tensorfs::io::source::Source;

const MIB: u64 = 1 << 20;
const CAP: u64 = 64 * MIB;
const WEIGHTS: u64 = 32 * MIB;

fn require(ok: bool, why: &str) -> Result<()> {
    if ok { Ok(()) } else { Err(Error::Invalid(why.into())) }
}

fn open(ordinal: i32) -> Result<Memory> {
    let backend = CudaBackend::open(ordinal)?;
    let memory = Memory::open_with_limit(backend.gpu()?, Box::new(backend), Some(CAP))?;
    memory.serve_standalone_torch()?;
    Ok(memory)
}

fn payload() -> Arc<[u8]> {
    (0..WEIGHTS as usize).map(|i| (i as u8).wrapping_mul(37).wrapping_add(11)).collect::<Vec<_>>().into()
}

fn drain_idle(memory: &Memory, transfers: &CudaBackend) -> Result<()> {
    transfers.synchronize(transfers.stream())?;
    let torch = memory.torch()?;
    let deadline = Instant::now() + Duration::from_secs(10);
    while torch.idle().1 > 0 {
        require(Instant::now() < deadline, "freed spans did not complete their stream epochs")?;
        std::thread::sleep(Duration::from_millis(1));
    }
    torch.reclaim(u64::MAX);
    require(torch.mapped() == 0, "probe-owned torch mappings remain after reclaim")
}

fn region_swap(ordinal: i32, transfers: &CudaBackend, concurrent_free: bool) -> Result<Value> {
    let memory = open(ordinal)?;
    let region = memory.region("native-memory-gate", RegionKind::Latent, &[], true)?;
    let bytes = payload();
    let ptr;
    let spare;
    {
        let _pin = region.allocating()?;
        ptr = memory.torch_alloc(WEIGHTS, transfers.stream(), None)?;
        // This probe owns the full live allocation, and copy_in completes before its pin ends.
        transfers.copy_in(Va { base: ptr, len: WEIGHTS }, 0, &bytes)?;
        spare = if concurrent_free {
            let p = memory.torch_alloc(WEIGHTS, transfers.stream(), None)?;
            transfers.copy_in(Va { base: p, len: WEIGHTS }, 0, &bytes)?;
            Some(p)
        } else { None };
    }
    let freer = spare.map(|p| {
        let memory = memory.clone();
        std::thread::spawn(move || memory.torch_free(p, WEIGHTS, transfers.stream()))
    });
    let reclaimed = memory.region_reclaim(region.id(), true)?;
    if let Some(freer) = freer {
        freer.join().map_err(|_| Error::Internal("concurrent free panicked".into()))?;
    }
    let swapped = match reclaimed {
        varena::Reclaimed::Swapped(n) => n,
        other => return Err(Error::Invalid(format!("expected a swapped region, got {other:?}"))),
    };
    require(swapped >= WEIGHTS && swapped <= CAP, "unexpected swapped region size")?;
    let mut actual = vec![0; WEIGHTS as usize];
    {
        let pin = region.pin()?;
        require(!pin.fresh(), "a swapped region repinned as discarded")?;
        // The region pin holds the restored mapping; the restore's copy stream has completed.
        transfers.copy_out(Va { base: ptr, len: WEIGHTS }, 0, &mut actual, Event::DONE)?;
        require(actual.as_slice() == bytes.as_ref(), "region bytes changed across swap and restore")?;
    }
    memory.torch_free(ptr, WEIGHTS, transfers.stream());
    region.close();
    drain_idle(&memory, transfers)?;
    memory.retire()?;
    Ok(json!({"swapped_bytes": swapped, "checked_bytes": WEIGHTS, "fresh": false,
        "sha256": tensorfs::sha256::hex_digest(&actual), "concurrent_free": concurrent_free}))
}

fn cold_weights(ordinal: i32, transfers: &CudaBackend) -> Result<Value> {
    let memory = open(ordinal)?;
    let bytes = payload();
    let layout = Arc::new(Layout {
        parts: vec![], names: vec![vec!["gate/weight".into()]], nbytes: WEIGHTS,
        digest: tensorfs::sha256::hex_digest(&bytes),
        regions: vec![Region { offset: 0, nbytes: WEIGHTS, span: WEIGHTS,
            items: vec![Item { offset: 0, len: WEIGHTS, source: ItemSource::Inline(bytes.clone()), part_end: true }] }],
    });
    let fingerprint = Fingerprint::new(&layout, &[], Fill::Stored, "native-memory-gate", "0/1", &transfers.gpu()?.uuid);
    let set = memory.load(SetSpec { name: "native-memory-gate/weight".into(), layout,
        source: Arc::new(Source::files(vec![], false, 0)), fingerprint }, Level::Gpu)?;
    let cold = varena::table::Class::N.index();
    let before = memory.classes().get(cold).copied().unwrap_or(0);
    require(before == WEIGHTS, "fixture weights are not wholly cold and resident")?;
    let activation_bytes = 33 * MIB;
    let ptr = memory.torch_alloc(activation_bytes, transfers.stream(), None)?;
    let after = memory.classes().get(cold).copied().unwrap_or(0);
    require(after == 0, "torch allocation did not reclaim the cold weight mapping")?;
    memory.torch_free(ptr, activation_bytes, transfers.stream());
    let pending_idle = memory.torch()?.idle().1;
    let mut actual = vec![0; WEIGHTS as usize];
    {
        // No pre-wait: weight placement must recover the just-freed torch float itself.
        let pin = memory.bind(set, 0, transfers.stream())?;
        transfers.copy_out(Va { base: pin.ptr, len: pin.len }, 0, &mut actual, pin.ready)?;
        require(actual.as_slice() == bytes.as_ref(), "weights changed after reclaiming the torch float")?;
    }
    memory.unload(set);
    memory.settle();
    drain_idle(&memory, transfers)?;
    memory.retire()?;
    Ok(json!({"fixture": "inline stored weight bytes", "weight_bytes": WEIGHTS,
        "activation_bytes": activation_bytes, "cold_before": before, "cold_after": after,
        "pending_idle_before_weight_refill": pending_idle, "checked_bytes": WEIGHTS,
        "sha256": tensorfs::sha256::hex_digest(&actual)}))
}

fn run(ordinal: i32) -> Result<Value> {
    let transfers = CudaBackend::open(ordinal)?;
    let gpu = transfers.gpu()?;
    let free_before = transfers.free()?;
    let mut cases = Vec::new();
    for (name, mode) in [("region_swap", 0), ("region_swap_with_concurrent_free", 1), ("cold_weights_and_reverse_torch_reclaim", 2)] {
        let result = match mode {
            0 => region_swap(ordinal, &transfers, false),
            1 => region_swap(ordinal, &transfers, true),
            _ => cold_weights(ordinal, &transfers),
        };
        match result {
            Ok(facts) => cases.push(json!({"case": name, "passed": true, "facts": facts})),
            Err(error) => {
                cases.push(json!({"case": name, "passed": false, "error_class": error.code(), "error": error.to_string()}));
                break;
            }
        }
    }
    let passed = cases.len() == 3 && cases.iter().all(|case| case["passed"] == true);
    Ok(json!({"schema": "varena-native-memory-v1", "passed": passed, "cuda_ordinal": ordinal,
        "device": {"uuid": gpu.uuid, "model": gpu.model, "capacity": gpu.memory},
        "local_cap_bytes": CAP, "cap_scope": "this probe executor only",
        "backend_headroom": transfers.headroom(), "driver_free_before": free_before,
        "driver_free_after": transfers.free()?, "cases": cases,
        "source": {"commit": env!("NATIVE_MEMORY_SOURCE_COMMIT"), "dirty": env!("NATIVE_MEMORY_SOURCE_DIRTY") == "true",
            "probe_sha256": env!("NATIVE_MEMORY_PROBE_SHA256"), "backend": "CudaBackend",
            "os": std::env::consts::OS, "arch": std::env::consts::ARCH}}))
}

fn main() {
    let ordinal = std::env::args().nth(1).and_then(|s| s.parse::<i32>().ok());
    let report = match ordinal {
        Some(ordinal) => run(ordinal).unwrap_or_else(|error| json!({"passed": false, "cuda_ordinal": ordinal,
            "error_class": error.code(), "error": error.to_string()})),
        None => json!({"passed": false, "error": "usage: native-memory CUDA_ORDINAL"}),
    };
    println!("{report}");
    if report["passed"] != true { std::process::exit(1); }
}
