//! What TensorFS copies are measured against: a GPU's pinned host→device bandwidth (best of 1,
//! 2 and 4 streams) and PCIe link, and a volume's O_DIRECT read (`probe`). Ceilings are kept per
//! boot (`probe::keep`), the GPU's by UUID and driver. Reads never measure one (`io::ceiling`
//! does); a run faster than its ceiling raises it. Every copy run lands in one log (`take`).

use std::fs::File;
use std::os::unix::fs::MetadataExt;
use std::sync::Mutex;

use super::probe;

#[cfg(feature = "cuda")]
mod gpu;
#[cfg(feature = "cuda")]
pub(crate) use gpu::{device_link, h2d};
#[cfg(feature = "cuda")]
pub(crate) use gpu::pci_address;
#[cfg(feature = "cuda")]
use gpu::h2d_key;

const LOG_KEEP: usize = 256;

/// One run of copies: a GPU's copy stream busy from its first job to its last landing
/// (`h2d`), or a host layout filled from a store's files (`disk`).
#[derive(Debug, Clone, Default)]
pub(crate) struct Copy {
    pub kind: &'static str,
    /// -1 for a disk fill.
    pub gpu: i32,
    pub bytes: u64,
    pub ns: u64,
    /// GB/s; 0 when unmeasured.
    pub ceiling: f64,
    /// The PCIe link as the run ended (`Gen4 x16 of Gen5 x16`), "" if unknown or a disk fill.
    pub link: String,
    /// h2d: GPU time in copies; the rest of `ns` the host left the stream idle.
    pub busy_ns: u64,
    /// h2d: host time page-locking source ranges.
    pub pin_ns: u64,
    /// h2d: bytes through the pinned staging buffers.
    pub staged: u64,
    /// disk: bytes the page cache held.
    pub cached: u64,
}

static LOG: Mutex<Vec<Copy>> = Mutex::new(Vec::new());

pub(crate) fn record(c: Copy) {
    let mut log = LOG.lock().unwrap();
    if log.len() >= LOG_KEEP {
        log.remove(0);
    }
    log.push(c);
}

/// A run of reads from `files`, logged against its volume's kept ceiling. Only a run the page
/// cache had no part in measures the volume: one of a probe's size seeds the ceiling, any that
/// beats it raises it. A run that read cached bytes is logged unmeasured (ceiling 0).
pub(crate) fn disk_run(files: &[&File], bytes: u64, ns: u64, direct: u64, cached: u64) {
    let Some(volume) = files.first().and_then(|f| f.metadata().ok()).map(|m| volume(m.dev())) else {
        return;
    };
    let rate = direct as f64 / ns.max(1) as f64;
    let kept = probe::disk_known(volume).map(|(gbps, _)| gbps);
    let measures = cached == 0 && (direct >= probe::KEEP || kept.is_some_and(|k| rate > k));
    if measures {
        probe::disk_keep(volume, (rate, true));
    }
    let ceiling = match (cached, kept) {
        (0, Some(k)) => k.max(rate),
        (0, None) if measures => rate,
        _ => 0.0,
    };
    record(Copy { kind: "disk", gpu: -1, bytes, ns, cached, ceiling, ..Default::default() });
}

/// A run of copies to GPU `c.gpu`, logged against its kept H2D ceiling (0 until `io::ceiling`
/// measures it); a run faster than that ceiling raises it.
#[cfg(feature = "cuda")]
pub(crate) fn h2d_run(mut c: Copy) {
    let key = h2d_key(c.gpu);
    if let Some(kept) = key.as_deref().and_then(probe::known) {
        let rate = c.bytes as f64 / c.ns.max(1) as f64;
        if rate > kept[0] {
            probe::keep(key.as_deref().unwrap(), &[rate, kept.get(1).copied().unwrap_or(1.0)]);
        }
        c.ceiling = kept[0].max(rate);
    }
    record(c);
}

/// A `st_dev` as `durable::device` names its volume.
#[cfg(target_os = "linux")]
fn volume(dev: u64) -> u64 {
    u64::from(libc::major(dev)) << 32 | u64::from(libc::minor(dev))
}

#[cfg(not(target_os = "linux"))]
fn volume(dev: u64) -> u64 {
    dev
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Link {
    pub gen: u32,
    pub width: u32,
    pub max_gen: u32,
    pub max_width: u32,
}

impl std::fmt::Display for Link {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "Gen{} x{} of Gen{} x{}",
            self.gen, self.width, self.max_gen, self.max_width
        )
    }
}

/// The link of PCI device `pci` (`0000:65:00.0`) from sysfs. An idle GPU trains its link down,
/// so read it right after a copy.
pub(crate) fn link(pci: &str) -> Option<Link> {
    let read = |f: &str| std::fs::read_to_string(format!("/sys/bus/pci/devices/{pci}/{f}")).ok();
    let gen = |f: &str| -> Option<u32> {
        let gts: f64 = read(f)?.split_whitespace().next()?.parse().ok()?;
        [2.5, 5.0, 8.0, 16.0, 32.0, 64.0]
            .iter()
            .position(|&g| g == gts)
            .map(|i| i as u32 + 1)
    };
    let width = |f: &str| read(f)?.trim().parse().ok();
    Some(Link {
        gen: gen("current_link_speed")?,
        width: width("current_link_width")?,
        max_gen: gen("max_link_speed")?,
        max_width: width("max_link_width")?,
    })
}
