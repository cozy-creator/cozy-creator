//! The GPUs a machine reports in `StatusFrame.gpus[]`: NVIDIA's through NVML, which is opened at
//! run time (`libnvidia-ml.so.1`, `nvml.dll`), and an Apple Silicon Mac's through Metal. No driver
//! is "no GPU", never an error.
#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
mod apple;
mod cuda;
mod faults;

pub use cuda::open as open_cuda;
pub use faults::{Fault, watch_faults};

use crate::{Error, Result};
use nvml_wrapper::{
    Nvml,
    bitmasks::device::ThrottleReasons,
    enum_wrappers::device::{Clock, ComputeMode, TemperatureSensor},
    enums::device::UsedGpuMemory,
    error::NvmlError,
    struct_wrappers::device::{MemoryInfo, ProcessInfo},
};
use std::collections::BTreeMap;

/// One GPU: machine.proto `Gpu` fields 1-6, plus the memory class.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Gpu {
    pub index: u32,
    pub name: String,
    pub uuid: String,
    pub pci_bus_id: String,
    pub memory_bytes: u64,
    pub driver: String,
    /// The lane rule's row: an NVIDIA card's compute capability (`9.0` on a Hopper), an Apple
    /// GPU's newest Metal family (`apple9`); empty when the driver does not say.
    pub capability: String,
    /// [`open_cuda`] reads it from CUDA's device attributes; the probe leaves it `None`.
    pub memory_class: Option<MemoryClass>,
}

/// How a GPU reaches host memory (design §6.2), which picks Runtime's memory policy.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MemoryClass {
    /// Its own memory across PCIe or SXM: today's paging.
    Discrete,
    /// Its own memory plus coherent host memory (GH200, GB200, GB300).
    Coherent,
    /// One pool with the CPU (GB10, Jetson, Apple Silicon).
    Unified,
}

/// The oldest NVIDIA driver the stack's CUDA 13.0 runs on. The cozy-kernels wheel declares the same
/// (`cozy_kernels.DRIVER_FLOOR`), and cozy-runtime's `kernels.yaml` checks the two are equal.
pub const DRIVER_FLOOR: &str = if cfg!(windows) { "580.88" } else { "580.65.06" };

/// Why a GPU on `driver` cannot serve: it is older than [`DRIVER_FLOOR`]. `None` when it meets the
/// floor, or when the driver's version does not parse (CUDA then answers for itself).
pub fn below_floor(driver: &str) -> Option<String> {
    let version = |text: &str| {
        text.trim()
            .split('.')
            .map(|part| part.parse::<u32>().ok())
            .collect::<Option<Vec<u32>>>()
    };
    (version(driver)? < version(DRIVER_FLOOR)?).then(|| {
        format!(
            "NVIDIA driver {driver} is below {DRIVER_FLOOR}, the oldest CUDA 13.0 runs on: \
             update the driver to {DRIVER_FLOOR} or newer"
        )
    })
}

/// The GPUs this process may use, and why any it cannot read is left out: a lost or unreadable GPU
/// costs that GPU, never the machine. `visible` is `CUDA_VISIBLE_DEVICES` as the caller's config
/// read it: indices or UUIDs, and an empty value hides every GPU.
pub fn probe(visible: Option<&str>) -> (Vec<Gpu>, Vec<String>) {
    if visible.is_some_and(|visible| visible.trim().is_empty()) {
        return (vec![], vec![]);
    }
    if let Some(gpus) = apple() {
        return (gpus, vec![]);
    }
    let nvml = match Nvml::init() {
        Ok(nvml) => nvml,
        Err(
            NvmlError::LibloadingError(_) | NvmlError::DriverNotLoaded | NvmlError::LibraryNotFound,
        ) => {
            return (vec![], vec![]);
        }
        Err(error) => return (vec![], vec![format!("NVML did not start ({error}): no GPU is used")]),
    };
    let count = match nvml.device_count() {
        Ok(count) => count,
        Err(error) => return (vec![], vec![format!("NVML counts no GPUs ({error}): no GPU is used")]),
    };
    let driver = nvml.sys_driver_version().unwrap_or_default();
    let named: Option<Vec<&str>> = visible.map(|visible| visible.split(',').map(str::trim).collect());
    let read = |index: u32| -> Result<Gpu> {
        let device = nvml.device_by_index(index).map_err(failed("device"))?;
        Ok(Gpu {
            index,
            name: device.name().map_err(failed("name"))?,
            uuid: device.uuid().map_err(failed("uuid"))?,
            pci_bus_id: device.pci_info().map_err(failed("PCI info"))?.bus_id,
            memory_bytes: memory(device.memory_info())?.0,
            driver: driver.clone(),
            capability: device
                .cuda_compute_capability()
                .map_or(String::new(), |cc| format!("{}.{}", cc.major, cc.minor)),
            memory_class: None,
        })
    };
    let (mut gpus, mut problems) = (vec![], vec![]);
    for index in 0..count {
        match read(index) {
            Ok(gpu) => gpus.push(gpu),
            // One the caller hid by its index is none of its business.
            Err(_) if named.as_ref().is_some_and(|named| named.iter().all(|n| n.parse::<u32>().is_ok_and(|n| n != index))) => {}
            Err(error) => problems.push(format!("GPU {index} cannot be read ({error}): it is not used")),
        }
    }
    if let Some(named) = named {
        gpus.retain(|gpu| {
            named.contains(&gpu.index.to_string().as_str()) || named.contains(&gpu.uuid.as_str())
        });
    }
    (gpus, problems)
}

/// Whether every pair of these GPUs (`CUDA_VISIBLE_DEVICES` entries) reaches the other over
/// NVLink, as NVML reports peer access; false for fewer than two, or when NVML cannot say.
pub fn nvlinked(entries: &[&str]) -> bool {
    use nvml_wrapper::enum_wrappers::device::{P2pCapabilitiesIndex, P2pStatus};
    let Ok(nvml) = Nvml::init() else { return false };
    let devices: Option<Vec<_>> = entries
        .iter()
        .map(|entry| match entry.parse::<u32>() {
            Ok(index) => nvml.device_by_index(index).ok(),
            Err(_) => nvml.device_by_uuid(*entry).ok(),
        })
        .collect();
    let Some(devices) = devices.filter(|devices| devices.len() > 1) else { return false };
    devices.iter().enumerate().all(|(a, one)| {
        devices.iter().enumerate().filter(|(b, _)| *b != a).all(|(_, other)| {
            matches!(one.p2p_status(other, P2pCapabilitiesIndex::NvLink), Ok(P2pStatus::Ok))
        })
    })
}

/// An Apple Silicon Mac's GPU; `None` elsewhere, where NVML answers.
#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
fn apple() -> Option<Vec<Gpu>> {
    Some(apple::probe().into_iter().collect())
}
#[cfg(not(all(target_os = "macos", target_arch = "aarch64")))]
fn apple() -> Option<Vec<Gpu>> {
    None
}

/// One GPU opened for repeated reads (TensorD's memory sampler reads it every second). NVML
/// stays loaded while it lives.
pub struct Device {
    nvml: Nvml,
    index: u32,
}

/// One reading of a GPU's memory.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Reading {
    pub total: u64,
    pub free: u64,
    /// A desktop is initialized on it.
    pub display: bool,
    /// Device bytes the driver charges each process (pid -> bytes), compute and graphics (a
    /// desktop, a game) alike; `None` where it cannot account every one (a container whose pids
    /// are not the driver's, WDDM).
    pub processes: Option<BTreeMap<u32, u64>>,
}

impl Device {
    /// `entry` as `CUDA_VISIBLE_DEVICES` spells it: an index, or a `GPU-…` UUID.
    pub fn open(entry: &str) -> Result<Self> {
        let nvml = Nvml::init().map_err(failed("init"))?;
        let device = match entry.parse::<u32>() {
            Ok(index) => nvml.device_by_index(index),
            Err(_) => nvml.device_by_uuid(entry),
        }
        .map_err(failed("open device"))?;
        let index = device.index().map_err(failed("device index"))?;
        Ok(Self { nvml, index })
    }

    pub fn read(&self) -> Result<Reading> {
        let device = self
            .nvml
            .device_by_index(self.index)
            .map_err(failed("open device"))?;
        let (total, free) = memory(device.memory_info())?;
        let charged = |rows: Vec<ProcessInfo>| -> Option<Vec<(u32, u64)>> {
            rows.into_iter()
                .map(|row| match row.used_gpu_memory {
                    UsedGpuMemory::Used(bytes) => Some((row.pid, bytes)),
                    UsedGpuMemory::Unavailable => None,
                })
                .collect()
        };
        // A process in both lists (a CUDA program that also draws) is charged once: its larger.
        let graphics = device
            .running_graphics_processes()
            .ok()
            .and_then(charged);
        let processes = device
            .running_compute_processes()
            .ok()
            .and_then(charged)
            .zip(graphics)
            .map(|(compute, graphics)| {
                compute
                    .into_iter()
                    .chain(graphics)
                    .fold(BTreeMap::new(), |mut by, (pid, bytes)| {
                        let held: &mut u64 = by.entry(pid).or_default();
                        *held = (*held).max(bytes);
                        by
                    })
            });
        Ok(Reading {
            total,
            free,
            display: device.is_display_active().unwrap_or(false),
            processes,
        })
    }

    /// Its free bytes now (an integrated GPU's: the host's available memory), read alone.
    pub fn free(&self) -> Result<u64> {
        let device = self.nvml.device_by_index(self.index).map_err(failed("open device"))?;
        memory(device.memory_info()).map(|(_, free)| free)
    }

    /// Its bytes in use now, every process's, as `nvidia-smi` shows them (not the driver's reserve;
    /// an integrated GPU's: total less free).
    pub(crate) fn used(&self) -> Result<u64> {
        let device = self.nvml.device_by_index(self.index).map_err(failed("open device"))?;
        match device.memory_info() {
            Ok(info) => Ok(info.used),
            answer => memory(answer).map(|(total, free)| total.saturating_sub(free)),
        }
    }

    /// Whether its compute mode admits one context at a time (exclusive-process) or none
    /// (prohibited): a second process holding a context locks every other out. A driver that
    /// does not say shares it.
    pub fn exclusive(&self) -> Result<bool> {
        match self.nvml.device_by_index(self.index).and_then(|device| device.compute_mode()) {
            Ok(mode) => Ok(mode != ComputeMode::Default),
            Err(NvmlError::NotSupported) => Ok(false),
            Err(error) => Err(failed("compute mode")(error)),
        }
    }

    /// What the display it drives uses now: its graphics processes' bytes (0 where the driver
    /// does not attribute them), or None when it drives none.
    pub fn display_use(&self) -> Result<Option<u64>> {
        let device = self.nvml.device_by_index(self.index).map_err(failed("open device"))?;
        if !device.is_display_active().map_err(failed("display active"))? {
            return Ok(None);
        }
        let graphics = device.running_graphics_processes().unwrap_or_default();
        Ok(Some(graphics.iter().map(|row| match row.used_gpu_memory {
            UsedGpuMemory::Used(bytes) => bytes,
            UsedGpuMemory::Unavailable => 0,
        }).sum()))
    }

    pub fn display_active(&self) -> Result<bool> {
        self.nvml
            .device_by_index(self.index)
            .and_then(|device| device.is_display_active())
            .map_err(failed("display active"))
    }
}

/// One GPU's bytes in use as the driver reads them ([`Device::used`]): when a [`Peak`] began and
/// at its fullest. With its clocks while busy (readings NVML calls idle left out), so a slowed
/// host shows in every call: the SM clock's mean and lowest against the card's maximum, the
/// limits the driver applied (power cap, thermal, hardware slowdown) and the share of busy
/// readings under one, the share under a heat limit (cl-913), the busy time they cover, the
/// hottest temperature and the highest power draw.
#[derive(Clone, Debug, Default, serde::Serialize)]
pub struct Used {
    pub gpu: u32,
    pub before_bytes: u64,
    pub peak_bytes: u64,
    pub sm_mhz_limit: u32,
    pub sm_mhz_mean: u32,
    pub sm_mhz_low: u32,
    pub throttles: Vec<&'static str>,
    pub throttled_share: f32,
    pub heat_share: f32,
    pub busy_ms: u64,
    pub temperature_max_c: u32,
    pub power_max_w: u32,
}

/// A GPU busy this long in a call was under load: its readings judge its heat.
pub const LOADED_MS: u64 = 20_000;
/// Held back for heat for more than this share of its busy readings: hot.
pub const HOT_SHARE: f32 = 0.5;

impl Used {
    /// It was under load in the call.
    pub fn loaded(&self) -> bool {
        self.busy_ms >= LOADED_MS
    }

    /// Under load, the driver held it back for heat most of the time.
    pub fn hot(&self) -> bool {
        self.loaded() && self.heat_share > HOT_SHARE
    }
}

/// The driver's clock limits a busy reading can carry, by name.
const THROTTLES: [(ThrottleReasons, &str); 6] = [
    (ThrottleReasons::SW_POWER_CAP, "power_cap"),
    (ThrottleReasons::HW_SLOWDOWN, "hw_slowdown"),
    (ThrottleReasons::SW_THERMAL_SLOWDOWN, "sw_thermal"),
    (ThrottleReasons::HW_THERMAL_SLOWDOWN, "hw_thermal"),
    (ThrottleReasons::HW_POWER_BRAKE_SLOWDOWN, "power_brake"),
    (ThrottleReasons::APPLICATIONS_CLOCKS_SETTING, "app_clocks"),
];

/// The limits that are heat's: the driver slowing the card to cool it, and the hardware slowdown that heat raises. A power cap
/// is the card's own rating, not heat.
fn heat() -> ThrottleReasons {
    ThrottleReasons::SW_THERMAL_SLOWDOWN | ThrottleReasons::HW_THERMAL_SLOWDOWN | ThrottleReasons::HW_SLOWDOWN
}

/// How often a call's GPUs are read.
const EVERY_MS: u64 = 100;

/// A call's clock readings of one GPU, summed into [`Used`].
struct Clocks {
    busy: u32,
    mhz_sum: u64,
    low: u32,
    throttled: u32,
    hot: u32,
    seen: ThrottleReasons,
    temperature: u32,
    power_mw: u32,
}

impl Clocks {
    fn new() -> Self {
        Self { busy: 0, mhz_sum: 0, low: 0, throttled: 0, hot: 0, seen: ThrottleReasons::empty(), temperature: 0, power_mw: 0 }
    }

    fn read(&mut self, device: &Device) {
        let Ok(gpu) = device.nvml.device_by_index(device.index) else { return };
        let temperature = gpu.temperature(TemperatureSensor::Gpu).unwrap_or(0);
        let power_mw = gpu.power_usage().unwrap_or(0);
        let reasons = gpu.current_throttle_reasons().unwrap_or(ThrottleReasons::empty());
        match gpu.clock_info(Clock::SM) {
            Ok(mhz) => self.add(mhz, reasons, temperature, power_mw),
            Err(_) => self.add(0, ThrottleReasons::GPU_IDLE, temperature, power_mw),
        }
    }

    fn add(&mut self, mhz: u32, reasons: ThrottleReasons, temperature: u32, power_mw: u32) {
        self.temperature = self.temperature.max(temperature);
        self.power_mw = self.power_mw.max(power_mw);
        if reasons.contains(ThrottleReasons::GPU_IDLE) {
            return;
        }
        self.busy += 1;
        self.mhz_sum += u64::from(mhz);
        self.low = if self.busy == 1 { mhz } else { self.low.min(mhz) };
        let limited = THROTTLES.iter().fold(ThrottleReasons::empty(), |all, (r, _)| all | (reasons & *r));
        self.throttled += u32::from(!limited.is_empty());
        self.hot += u32::from(reasons.intersects(heat()));
        self.seen |= limited;
    }

    fn fill(&self, used: &mut Used, limit: u32) {
        used.sm_mhz_limit = limit;
        if self.busy > 0 {
            used.sm_mhz_mean = (self.mhz_sum / u64::from(self.busy)) as u32;
            used.sm_mhz_low = self.low;
            used.throttled_share = self.throttled as f32 / self.busy as f32;
            used.heat_share = self.hot as f32 / self.busy as f32;
        }
        used.busy_ms = u64::from(self.busy) * EVERY_MS;
        used.throttles = THROTTLES.iter().filter(|(r, _)| self.seen.contains(*r)).map(|(_, n)| *n).collect();
        used.temperature_max_c = self.temperature;
        used.power_max_w = self.power_mw / 1000;
    }
}

/// One recorded reading of a GPU, as a call's thread takes one every 100 ms: what a replay feeds in place of NVML's
/// (`machine.gpu_samples`, cl-913), so a throttling host's readings go through the real path. `reasons` are the driver's limits
/// in force by [`THROTTLES`]' names, and `idle`.
#[derive(Clone, Debug, Default, serde::Deserialize)]
#[serde(default)]
pub struct Sample {
    pub gpu: u32,
    pub used_bytes: u64,
    pub sm_mhz: u32,
    pub sm_mhz_limit: u32,
    pub reasons: Vec<String>,
    pub temperature_c: u32,
    pub power_w: u32,
}

impl Sample {
    fn reasons(&self) -> ThrottleReasons {
        self.reasons.iter().fold(ThrottleReasons::empty(), |all, name| match name.as_str() {
            "idle" => all | ThrottleReasons::GPU_IDLE,
            name => all | THROTTLES.iter().find(|(_, n)| *n == name).map_or(ThrottleReasons::empty(), |(r, _)| *r),
        })
    }
}

/// Told, once per GPU, that the call has held it back for heat most of its load so far ([`Used::hot`]): what it read until then.
pub type Hot = Box<dyn FnMut(&Used) + Send>;

/// Where a call's readings come from: its GPUs through NVML, or recorded ones, each GPU's in turn, from the first again
/// once they run out.
enum Source {
    Live(Vec<Device>),
    Recorded(Vec<Vec<Sample>>),
}

/// A call's GPUs, read every 100 ms on a thread of its own from [`Peak::watch`] until
/// [`Peak::end`] (or its drop). A GPU NVML cannot open is left out.
pub struct Peak {
    stop: std::sync::mpsc::Sender<()>,
    thread: std::thread::JoinHandle<Vec<Used>>,
}

impl Peak {
    /// `entries` as `CUDA_VISIBLE_DEVICES` spells them; None for none.
    pub fn watch(entries: Vec<String>, hot: Option<Hot>) -> Option<Peak> {
        if entries.is_empty() {
            return None;
        }
        Self::spawn(move || Source::Live(entries.iter().filter_map(|entry| Device::open(entry).ok()).collect()), hot)
    }

    /// Recorded readings in place of the GPUs' own, by GPU; None for none.
    pub fn replay(samples: Vec<Sample>, hot: Option<Hot>) -> Option<Peak> {
        let mut gpus: BTreeMap<u32, Vec<Sample>> = BTreeMap::new();
        for sample in samples {
            gpus.entry(sample.gpu).or_default().push(sample);
        }
        if gpus.is_empty() {
            return None;
        }
        Self::spawn(move || Source::Recorded(gpus.into_values().collect()), hot)
    }

    fn spawn(source: impl FnOnce() -> Source + Send + 'static, mut hot: Option<Hot>) -> Option<Peak> {
        let (stop, stopped) = std::sync::mpsc::channel::<()>();
        let thread = std::thread::Builder::new().name("gpu-peak".into()).spawn(move || {
            let source = source();
            // Each GPU's row, readings, card maximum, place in its recording, and whether it was said hot.
            let mut rows: Vec<(Used, Clocks, u32, usize, bool)> = match &source {
                Source::Live(devices) => devices.iter()
                    .filter_map(|d| {
                        let bytes = d.used().ok()?;
                        let limit = d.nvml.device_by_index(d.index).and_then(|gpu| gpu.max_clock_info(Clock::SM)).unwrap_or(0);
                        Some((Used { gpu: d.index, before_bytes: bytes, peak_bytes: bytes, ..Used::default() }, Clocks::new(), limit, 0, false))
                    })
                    .collect(),
                Source::Recorded(gpus) => gpus.iter()
                    .map(|samples| {
                        let first = &samples[0];
                        (Used { gpu: first.gpu, before_bytes: first.used_bytes, peak_bytes: first.used_bytes, ..Used::default() }, Clocks::new(), first.sm_mhz_limit, 0, false)
                    })
                    .collect(),
            };
            while let Err(std::sync::mpsc::RecvTimeoutError::Timeout) = stopped.recv_timeout(std::time::Duration::from_millis(EVERY_MS)) {
                for (at, (row, clocks, limit, next, said)) in rows.iter_mut().enumerate() {
                    match &source {
                        Source::Live(devices) => {
                            row.peak_bytes = row.peak_bytes.max(devices[at].used().unwrap_or(0));
                            clocks.read(&devices[at]);
                        }
                        Source::Recorded(gpus) => {
                            let sample = &gpus[at][*next % gpus[at].len()];
                            *next += 1;
                            row.peak_bytes = row.peak_bytes.max(sample.used_bytes);
                            *limit = (*limit).max(sample.sm_mhz_limit);
                            clocks.add(sample.sm_mhz, sample.reasons(), sample.temperature_c, sample.power_w * 1000);
                        }
                    }
                    if let Some(hot) = hot.as_mut().filter(|_| !*said) {
                        let mut now = row.clone();
                        clocks.fill(&mut now, *limit);
                        if now.hot() {
                            *said = true;
                            hot(&now);
                        }
                    }
                }
            }
            rows.into_iter()
                .map(|(mut row, clocks, limit, ..)| {
                    clocks.fill(&mut row, limit);
                    row
                })
                .collect()
        });
        thread.ok().map(|thread| Peak { stop, thread })
    }

    pub fn end(self) -> Vec<Used> {
        drop(self.stop);
        self.thread.join().unwrap_or_default()
    }
}

/// A GPU's (total, free) memory from NVML's answer. An integrated GPU (GB10) has no
/// framebuffer, so NVML answers NotSupported: its memory is what this process may hold of the
/// host's (`host_memory::limit`, and the live budget under it).
fn memory(answer: std::result::Result<MemoryInfo, NvmlError>) -> Result<(u64, u64)> {
    match answer {
        Ok(memory) => Ok((memory.total, memory.free)),
        Err(NvmlError::NotSupported) => {
            let total = crate::contain::host_memory::limit();
            let free = u64::try_from(crate::contain::host_memory::read().available).ok();
            match total.zip(free) {
                Some((total, free)) => Ok((total, free.min(total))),
                None => Err(failed("memory info")(NvmlError::NotSupported)),
            }
        }
        Err(error) => Err(failed("memory info")(error)),
    }
}

fn failed(op: &'static str) -> impl FnOnce(NvmlError) -> Error {
    move |error| Error::Gpu {
        op,
        detail: error.to_string(),
    }
}
