//! What slows this machine's runs, said on each run it slows and in its Status (cl-913): a GPU the driver holds back for heat
//! most of a run that loads it (a call's readings, `platform::gpu::Peak`), and a Runtime that serves its GPUs none of its native
//! kernels (its executors' Start). Each is a run warning by its code, once a run, on its root. Heat is also the machine's
//! (Status `heat`) until a later loaded run there runs cool, and the Hub's (`machine::speed`), which ranks a host that throttles
//! under load after healthy ones. Recorded readings (`machine.gpu_samples`) go through all of it but the Hub.
use crate::execution::Engine;
use api::machine::GpuHeat;
use platform::gpu::{Sample, Used};
use std::{
    collections::{BTreeMap, HashSet},
    path::Path,
    sync::{LazyLock, Mutex},
};

pub(crate) const GPU_THROTTLED: &str = "gpu_throttled";
pub(crate) const RUNTIME_WITHOUT_KERNELS: &str = "runtime_without_kernels";

/// The machine's hot GPUs, by index.
static HEAT: LazyLock<Mutex<BTreeMap<u32, GpuHeat>>> = LazyLock::new(Mutex::default);
/// Why its Runtime serves its GPUs no native kernels, as its latest executor on GPUs said.
static KERNELS: Mutex<String> = Mutex::new(String::new());
/// The roots already told, by code.
static WARNED: LazyLock<Mutex<HashSet<(String, &'static str)>>> = LazyLock::new(Mutex::default);

/// Its hot GPUs, for its Status.
pub(crate) fn heat() -> Vec<GpuHeat> {
    HEAT.lock().unwrap().values().cloned().collect()
}

/// Why its Runtime serves its GPUs none of its kernels; empty: it does.
pub(crate) fn runtime_kernels() -> String {
    KERNELS.lock().unwrap().clone()
}

/// An executor on GPUs started, saying why its Runtime serves them none of its kernels (empty: it does).
pub(crate) fn started(absent: &str) {
    absent.clone_into(&mut KERNELS.lock().unwrap());
}

/// Run `id` runs on GPUs whose Runtime said `absent`.
pub(crate) fn without_kernels(engine: &Engine, id: &str, absent: &str) {
    let text = format!(
        "this machine's Runtime serves its GPUs none of its native kernels ({absent}): GEMMs fall back to slower ones and \
         FlashAttention-3 doesn't load, so this run is slower. A Runtime release carries them: cozy machine update <machine> --runtime <release>"
    );
    warn(engine, id, RUNTIME_WITHOUT_KERNELS, &text);
}

/// Run `id`'s GPU readings, so far (`done` false: the call goes on) or whole. One held back for heat most of a loaded run is hot: the
/// run warns once and the machine is marked, and at the end the Hub is told; one loaded and cool is no longer hot.
pub(crate) fn ran(engine: &Engine, id: &str, used: &[Used], replayed: bool, done: bool) {
    for gpu in used.iter().filter(|gpu| gpu.loaded()) {
        let mut heat = HEAT.lock().unwrap();
        match gpu.hot() {
            true => {
                heat.insert(gpu.gpu, GpuHeat {
                    gpu: gpu.gpu,
                    share: gpu.heat_share,
                    busy_ms: gpu.busy_ms,
                    sm_mhz_mean: gpu.sm_mhz_mean,
                    sm_mhz_limit: gpu.sm_mhz_limit,
                    temperature_max_c: gpu.temperature_max_c,
                    throttles: gpu.throttles.iter().map(|name| (*name).to_owned()).collect(),
                    run: id.to_owned(),
                    at_unix_ms: crate::machine::lifecycle::now_ms(),
                    replayed,
                });
                drop(heat);
                let so_far = if done { "" } else { " so far" };
                let text = format!(
                    "GPU {} ran throttled for heat {:.0}% of this run{so_far} ({} °C, {} of {} MHz), so it is slower: its host runs too hot, \
                     and another machine runs it faster",
                    gpu.gpu,
                    gpu.heat_share * 100.0,
                    gpu.temperature_max_c,
                    gpu.sm_mhz_mean,
                    gpu.sm_mhz_limit
                );
                warn(engine, id, GPU_THROTTLED, &text);
            }
            false if done => {
                heat.remove(&gpu.gpu);
            }
            false => {}
        }
    }
    if !done || replayed {
        return;
    }
    if let Some(most) = used.iter().filter(|gpu| gpu.loaded()).max_by(|a, b| a.heat_share.total_cmp(&b.heat_share)) {
        crate::machine::speed::heat(most.heat_share, most.busy_ms as f64 / 1e3);
    }
}

/// Recorded readings (`machine.gpu_samples`): one JSON line per reading. None: unreadable, said once.
pub(crate) fn samples(path: &Path) -> Option<Vec<Sample>> {
    let read = std::fs::read_to_string(path).map_err(|error| error.to_string()).and_then(|text| {
        (text.lines().filter(|line| !line.trim().is_empty()))
            .map(|line| serde_json::from_str::<Sample>(line).map_err(|error| error.to_string()))
            .collect::<Result<Vec<_>, _>>()
    });
    match read {
        Ok(samples) if !samples.is_empty() => Some(samples),
        Ok(_) => None,
        Err(error) => {
            static SAID: std::sync::Once = std::sync::Once::new();
            SAID.call_once(|| tracing::warn!("machine.gpu_samples {}: {error}", path.display()));
            None
        }
    }
}

/// `text` on run `id`'s root, as the warning `code` its controller keeps among the run's warnings: once a run.
fn warn(engine: &Engine, id: &str, code: &'static str, text: &str) {
    let root = match engine.get(id) {
        Ok(record) if !record.invocation.root.is_empty() => record.invocation.root,
        _ => id.to_owned(),
    };
    {
        let mut warned = WARNED.lock().unwrap();
        if warned.len() > 10_000 {
            warned.clear();
        }
        if !warned.insert((root.clone(), code)) {
            return;
        }
    }
    tracing::warn!("run {root}: {text}");
    if let Err(error) = engine.append_coded_warning(&root, code, text) {
        tracing::warn!("run {root}: {code} not said: {error}");
    }
}
