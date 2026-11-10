//! What TensorFS copies are measured against: a GPU's pinned host→device bandwidth (best of 1,
//! 2 and 4 streams), and a volume's O_DIRECT read (`probe`). Ceilings are kept per
//! boot (`probe::keep`), the GPU's by UUID and driver. Reads never measure one (`io::ceiling`
//! does); a run faster than its ceiling raises it.

use std::fs::File;
use std::os::unix::fs::MetadataExt;

use super::probe;

#[cfg(feature = "cuda")]
mod gpu;
#[cfg(feature = "cuda")]
pub(crate) use gpu::h2d;
#[cfg(feature = "cuda")]
pub(crate) use gpu::pci_address;
#[cfg(feature = "cuda")]
use gpu::h2d_key;

/// A run of reads from `files`, held to its volume's kept ceiling. Only a run the page cache had
/// no part in measures the volume: one of a probe's size seeds the ceiling, any that beats it
/// raises it.
pub(crate) fn disk_run(files: &[&File], ns: u64, direct: u64, cached: u64) {
    let Some(volume) = files.first().and_then(|f| f.metadata().ok()).map(|m| volume(m.dev())) else {
        return;
    };
    let rate = direct as f64 / ns.max(1) as f64;
    let kept = probe::disk_known(volume).map(|(gbps, _)| gbps);
    let measures = cached == 0 && (direct >= probe::KEEP || kept.is_some_and(|k| rate > k));
    if measures {
        probe::disk_keep(volume, (rate, true));
    }
}

/// A run of `bytes` copied to GPU `gpu` in `ns`: one faster than its kept H2D ceiling (none
/// until `io::ceiling` measures it) raises it.
#[cfg(feature = "cuda")]
pub(crate) fn h2d_run(gpu: i32, bytes: u64, ns: u64) {
    let key = h2d_key(gpu);
    if let Some(kept) = key.as_deref().and_then(probe::known) {
        let rate = bytes as f64 / ns.max(1) as f64;
        if rate > kept[0] {
            probe::keep(key.as_deref().unwrap(), &[rate, kept.get(1).copied().unwrap_or(1.0)]);
        }
    }
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
