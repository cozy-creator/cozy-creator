//! The GPUs a machine reports in `StatusFrame.gpus[]`: NVIDIA's through NVML, which is opened at
//! run time (`libnvidia-ml.so.1`, `nvml.dll`), and an Apple Silicon Mac's through Metal. No driver
//! is "no GPU", never an error.
#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
mod apple;
#[cfg(windows)]
mod budget;
mod cuda;
mod faults;

#[cfg(windows)]
pub use budget::{Adapter, Budget, adapters};
pub use cuda::memory_classes;
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
    /// [`memory_classes`] fills it from CUDA's device attributes; the probe leaves it `None`.
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

/// `visible` is `CUDA_VISIBLE_DEVICES` as the caller's config read it: indices or UUIDs, and an
/// empty value hides every GPU.
pub fn probe(visible: Option<&str>) -> Result<Vec<Gpu>> {
    if visible.is_some_and(|visible| visible.trim().is_empty()) {
        return Ok(vec![]);
    }
    if let Some(gpus) = apple() {
        return Ok(gpus);
    }
    let nvml = match Nvml::init() {
        Ok(nvml) => nvml,
        Err(
            NvmlError::LibloadingError(_) | NvmlError::DriverNotLoaded | NvmlError::LibraryNotFound,
        ) => {
            return Ok(vec![]);
        }
        Err(error) => return Err(failed("init")(error)),
    };
    let driver = nvml
        .sys_driver_version()
        .map_err(failed("driver version"))?;
    let mut gpus = vec![];
    for index in 0..nvml.device_count().map_err(failed("device count"))? {
        let device = nvml.device_by_index(index).map_err(failed("device"))?;
        gpus.push(Gpu {
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
        });
    }
    if let Some(visible) = visible {
        let named: Vec<&str> = visible.split(',').map(str::trim).collect();
        gpus.retain(|gpu| {
            named.contains(&gpu.index.to_string().as_str()) || named.contains(&gpu.uuid.as_str())
        });
    }
    Ok(gpus)
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
    key: String,
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
        let uuid = device.uuid().unwrap_or_else(|_| entry.into());
        let driver = nvml.sys_driver_version().unwrap_or_default();
        Ok(Self {
            key: format!("{uuid}/{driver}"),
            index,
            nvml,
        })
    }

    /// `<GPU UUID>/<driver version>`: what a learned context is valid for.
    pub fn key(&self) -> &str {
        &self.key
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
    pub fn used(&self) -> Result<u64> {
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
/// readings under one, the hottest temperature and the highest power draw.
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
    pub temperature_max_c: u32,
    pub power_max_w: u32,
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

/// A call's clock readings of one GPU, summed into [`Used`] at the end.
struct Clocks {
    busy: u32,
    mhz_sum: u64,
    low: u32,
    throttled: u32,
    seen: ThrottleReasons,
    temperature: u32,
    power_mw: u32,
}

impl Clocks {
    fn new() -> Self {
        Self { busy: 0, mhz_sum: 0, low: 0, throttled: 0, seen: ThrottleReasons::empty(), temperature: 0, power_mw: 0 }
    }

    fn read(&mut self, device: &Device) {
        let Ok(gpu) = device.nvml.device_by_index(device.index) else { return };
        self.temperature = self.temperature.max(gpu.temperature(TemperatureSensor::Gpu).unwrap_or(0));
        self.power_mw = self.power_mw.max(gpu.power_usage().unwrap_or(0));
        let reasons = gpu.current_throttle_reasons().unwrap_or(ThrottleReasons::empty());
        let Ok(mhz) = gpu.clock_info(Clock::SM) else { return };
        if reasons.contains(ThrottleReasons::GPU_IDLE) {
            return;
        }
        self.busy += 1;
        self.mhz_sum += u64::from(mhz);
        self.low = if self.busy == 1 { mhz } else { self.low.min(mhz) };
        let limited = THROTTLES.iter().fold(ThrottleReasons::empty(), |all, (r, _)| all | (reasons & *r));
        self.throttled += u32::from(!limited.is_empty());
        self.seen |= limited;
    }

    fn into(self, used: &mut Used, limit: u32) {
        used.sm_mhz_limit = limit;
        if self.busy > 0 {
            used.sm_mhz_mean = (self.mhz_sum / u64::from(self.busy)) as u32;
            used.sm_mhz_low = self.low;
            used.throttled_share = self.throttled as f32 / self.busy as f32;
        }
        used.throttles = THROTTLES.iter().filter(|(r, _)| self.seen.contains(*r)).map(|(_, n)| *n).collect();
        used.temperature_max_c = self.temperature;
        used.power_max_w = self.power_mw / 1000;
    }
}

/// A call's GPUs, read every 100 ms on a thread of its own from [`Peak::watch`] until
/// [`Peak::end`] (or its drop). A GPU NVML cannot open is left out.
pub struct Peak {
    stop: std::sync::mpsc::Sender<()>,
    thread: std::thread::JoinHandle<Vec<Used>>,
}

impl Peak {
    /// `entries` as `CUDA_VISIBLE_DEVICES` spells them; None for none.
    pub fn watch(entries: Vec<String>) -> Option<Peak> {
        if entries.is_empty() {
            return None;
        }
        let (stop, stopped) = std::sync::mpsc::channel::<()>();
        let thread = std::thread::Builder::new().name("gpu-peak".into()).spawn(move || {
            let devices: Vec<Device> = entries.iter().filter_map(|entry| Device::open(entry).ok()).collect();
            let mut rows: Vec<(Used, &Device, Clocks)> = devices.iter()
                .filter_map(|d| d.used().ok().map(|bytes| (Used { gpu: d.index, before_bytes: bytes, peak_bytes: bytes, ..Used::default() }, d, Clocks::new())))
                .collect();
            while let Err(std::sync::mpsc::RecvTimeoutError::Timeout) = stopped.recv_timeout(std::time::Duration::from_millis(100)) {
                for (row, device, clocks) in &mut rows {
                    row.peak_bytes = row.peak_bytes.max(device.used().unwrap_or(0));
                    clocks.read(device);
                }
            }
            rows.into_iter()
                .map(|(mut row, device, clocks)| {
                    let limit = device.nvml.device_by_index(device.index).and_then(|gpu| gpu.max_clock_info(Clock::SM)).unwrap_or(0);
                    clocks.into(&mut row, limit);
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
