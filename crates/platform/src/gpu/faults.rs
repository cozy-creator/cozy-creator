//! Driver faults on NVIDIA GPUs (NVML Xid events): one thread per GPU blocked in NVML's event wait,
//! woken by each fault. Linux only; elsewhere, or on a GPU that reports none, nothing is watched.
use std::sync::Arc;

/// A fault the driver reported on a GPU, or the GPU lost.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Fault {
    /// As nvidia-smi numbers it.
    pub index: u32,
    pub uuid: String,
    /// The Xid; `None` when NVML lost the GPU (fallen off the bus).
    pub xid: Option<u64>,
}

impl Fault {
    /// Whether the work on its GPU fails: every Xid but those that only record what already
    /// happened (45: cleanup after a process ended; 63: a row remap recorded; 92: a corrected-ECC
    /// rate).
    pub fn fails_work(&self) -> bool {
        !matches!(self.xid, Some(45 | 63 | 92))
    }
}

impl std::fmt::Display for Fault {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let Some(xid) = self.xid else {
            return write!(formatter, "GPU {} was lost (fallen off the bus)", self.index);
        };
        let what = match xid {
            13 => "graphics engine exception",
            31 => "GPU memory page fault",
            43 => "GPU stopped processing",
            48 => "double-bit ECC error",
            74 => "NVLink error",
            79 => "fallen off the bus",
            94 => "contained ECC error",
            95 => "uncontained ECC error",
            109 => "context switch timeout",
            119 | 120 => "GSP error",
            _ => "NVIDIA's Xid catalog names it",
        };
        write!(formatter, "GPU {} reported Xid {xid} ({what})", self.index)
    }
}

/// Calls `on` with every fault the driver reports on any GPU, from one thread per GPU, for as long
/// as this process lives. A GPU that cannot report faults is skipped with a warning.
#[cfg(target_os = "linux")]
pub fn watch_faults(on: Arc<dyn Fn(Fault) + Send + Sync>) {
    use nvml_wrapper::{
        Nvml,
        bitmasks::event::EventTypes,
        enums::event::XidError,
        error::{NvmlError, NvmlErrorWithSource},
    };
    let count = match Nvml::init().and_then(|nvml| nvml.device_count()) {
        Ok(count) => count,
        Err(error) => return tracing::warn!("GPU faults are not watched: {error}"),
    };
    for index in 0..count {
        let on = on.clone();
        let watched = std::thread::Builder::new().name(format!("gpu{index}-faults")).spawn(move || {
            let nvml = match Nvml::init() {
                Ok(nvml) => nvml,
                Err(error) => return tracing::warn!("GPU {index}'s faults are not watched: {error}"),
            };
            let registered = (|| -> Result<_, NvmlErrorWithSource> {
                let device = nvml.device_by_index(index)?;
                let set = nvml.create_event_set()?;
                Ok((device.uuid().unwrap_or_default(), device.register_events(EventTypes::CRITICAL_XID_ERROR, set)?))
            })();
            let (uuid, set) = match registered {
                Ok(watching) => watching,
                Err(error) => return tracing::warn!("GPU {index}'s faults are not watched: {error}"),
            };
            loop {
                // NVML's wait takes a limit; one that passes is waited again, never a verdict.
                match set.wait(i32::MAX as u32) {
                    Ok(event) => {
                        let xid = match event.event_data {
                            Some(XidError::Value(xid)) => xid,
                            Some(XidError::Unknown) | None => 0,
                        };
                        on(Fault { index, uuid: uuid.clone(), xid: Some(xid) });
                    }
                    Err(NvmlError::Timeout) => (),
                    Err(NvmlError::GpuLost) => return on(Fault { index, uuid, xid: None }),
                    Err(error) => return tracing::warn!("GPU {index}'s faults are no longer watched: {error}"),
                }
            }
        });
        if let Err(error) = watched {
            tracing::warn!("GPU {index}'s faults are not watched: {error}");
        }
    }
}

#[cfg(not(target_os = "linux"))]
pub fn watch_faults(_: Arc<dyn Fn(Fault) + Send + Sync>) {}
