//! The GPU half of `metrics`: a device's PCI address and its pinned host→device ceiling.

use std::ffi::c_void;
use std::time::Instant;

use super::super::{cuda, probe};
use cuda::Result;

const H2D_PROBE: usize = 128 << 20;

/// CUDA device `device`'s PCI address as sysfs names it (`0000:65:00.0`).
pub(crate) fn pci_address(device: i32) -> Option<String> {
    let dev = cuda::device_at(device).ok()?;
    let attr = |a| cuda::attribute(dev, a).ok();
    Some(format!(
        "{:04x}:{:02x}:{:02x}.0",
        attr(cuda::ATTR_PCI_DOMAIN_ID)?,
        attr(cuda::ATTR_PCI_BUS_ID)?,
        attr(cuda::ATTR_PCI_DEVICE_ID)?
    ))
}

/// Pinned host→device GB/s on CUDA device `device`, and the streams that reached it: kept this
/// boot, else measured on its primary context.
pub(crate) fn h2d(device: i32) -> Option<(f64, u32)> {
    let key = h2d_key(device)?;
    if let Some(&[gbps, streams]) = probe::known(&key).as_deref() {
        return Some((gbps, streams as u32));
    }
    let (dev, ctx) = cuda::primary_context(device).ok()?;
    let retained = PrimaryContext(dev);
    let _c = cuda::CtxGuard::enter(ctx).ok()?;
    let (gbps, streams) = measure_h2d(device, ctx, retained)
        .inspect_err(|e| tracing::warn!("tensorfs: no H2D ceiling: {e}"))
        .ok()?;
    probe::keep(&key, &[gbps, streams as f64]);
    Some((gbps, streams))
}

/// A GPU's ceiling key: its UUID and the driver.
pub(super) fn h2d_key(device: i32) -> Option<String> {
    let dev = cuda::device_at(device).ok()?;
    let driver = std::fs::read_to_string("/sys/module/nvidia/version").unwrap_or_default();
    Some(format!("h2d {} {}", cuda::uuid(dev).ok()?, driver.trim()))
}

struct PrimaryContext(i32);
impl Drop for PrimaryContext {
    fn drop(&mut self) {
        if let Ok(driver) = cuda::driver() {
            // Balances this probe's successful primary-context retain.
            // SAFETY: balances this probe's successful primary-context retain.
            unsafe { (driver.primary_ctx_release)(self.0) };
        }
    }
}

/// Actual probe owners, including the context lease. A failed completion check retains the
/// entire owner box for process lifetime; no failed stream or allocation is ever reused.
struct ProbeResources {
    _producer: varena::ProducerGuard,
    _context: PrimaryContext,
    ctx: cuda::CUcontext,
    device: i32,
    host: *mut c_void,
    dev: u64,
    streams: Vec<cuda::Stream>,
}

impl ProbeResources {
    fn drain(&self) -> Result<()> {
        let mut first = None;
        for stream in &self.streams {
            if let Err(error) = stream.drain() {
                first.get_or_insert(error);
            }
        }
        first.map_or(Ok(()), Err)
    }
}

struct Probe(Option<Box<ProbeResources>>);
impl Drop for Probe {
    fn drop(&mut self) {
        let Some(owners) = self.0.take() else { return };
        let drained = owners
            .drain()
            .and_then(|()| cuda::CtxGuard::enter(owners.ctx));
        let Ok(_context) = drained else {
            // Keep the owned streams, both allocations AND the primary-context lease.
            // Dropping raw pointer identities alone would not preserve their ownership.
            std::mem::forget(owners);
            return;
        };
        let Ok(driver) = cuda::driver() else {
            std::mem::forget(owners);
            return;
        };
        // Every private stream has a checked completion proof before either allocation frees.
        // SAFETY: the probe allocated both buffers in its context, and every stream completed (checked above).
        unsafe {
            if owners.dev != 0 && varena::working::free_completed(owners.device, owners.dev).is_err() {
                std::mem::forget(owners);
                return;
            }
            if !owners.host.is_null() {
                (driver.mem_free_host)(owners.host);
            }
        }
        // Stream owners and the retained primary context release after the buffers.
        drop(owners);
    }
}

fn measure_h2d(device: i32, ctx: cuda::CUcontext, retained: PrimaryContext) -> Result<(f64, u32)> {
    let driver = cuda::driver()?;
    let mut probe = Probe(Some(Box::new(ProbeResources {
        _producer: varena::enter_producer(device)?,
        _context: retained,
        ctx,
        device,
        host: std::ptr::null_mut(),
        dev: 0,
        streams: Vec::new(),
    })));
    let p = probe.0.as_mut().expect("probe owners live");
    // SAFETY: out-pointers into the probe's owned record; its context is current.
    unsafe {
        cuda::check(
            "cuMemHostAlloc",
            (driver.mem_host_alloc)(&mut p.host, H2D_PROBE, 0),
        )?;
        p.dev = cuda::allocate_ready(device, H2D_PROBE)?;
    }
    for _ in 0..4 {
        p.streams.push(cuda::Stream::new(device)?);
    }
    let mut best = (0.0, 1);
    for n in [1, 2, 4, 1, 2, 4, 1, 2, 4] {
        let piece = H2D_PROBE / n;
        let t = Instant::now();
        for (i, stream) in p.streams[..n].iter().enumerate() {
            // Both ranges lie inside the owned probe allocations.
            // SAFETY: both ranges lie inside the probe's own allocations, which outlive the synced copies.
            cuda::check("cuMemcpyHtoDAsync", unsafe {
                (driver.memcpy_htod_async)(
                    p.dev + (i * piece) as u64,
                    (p.host as *const u8).add(i * piece).cast(),
                    piece,
                    stream.raw() as _,
                )
            })?;
        }
        for stream in &p.streams[..n] {
            stream.drain()?;
        }
        let gbps = H2D_PROBE as f64 / t.elapsed().as_nanos() as f64;
        if gbps > best.0 {
            best = (gbps, n as u32);
        }
    }
    Ok(best)
}
