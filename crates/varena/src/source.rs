//! Where a Region's bytes come from, and the one path that puts them on the card. `use` on an
//! absent Region calls its Source on the caller's thread. One worker per GPU calls it ahead of
//! demand: what comes next in the observed order, else the Plan's. No lock crosses a Source.
use crate::{Contents, Device, Error, Fill, Pin, Region, Result, gpu, plan};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::thread::ThreadId;

/// A Region's bytes. The core alone decides when they are wanted.
pub trait Source: Send + Sync + 'static {
    /// Write every byte of `fill` and publish it. With `consumer`, the caller's thread, and that
    /// stream's next work is ordered behind the fill: the returned Pin is the caller's. Without
    /// one, the worker: no GIL, nothing that needs a caller, and `Busy` declines. On any error
    /// nothing is published. Never during capture, never under an allocator lock.
    fn fill(&self, fill: Fill, consumer: Option<u64>) -> Result<Option<Pin>>;

    /// Whether the worker could run `fill` now.
    fn ahead(&self) -> bool {
        false
    }

    /// Whether a demand `fill` now reads, so what runs ahead stops first. One that only copies
    /// on the card leaves it running: stopped, the next block's weights were read on demand.
    fn reads(&self) -> bool {
        true
    }

    /// Stop a fill the worker is running, soon, from any thread. Never waits.
    fn stop(&self) {}
}

#[derive(Default)]
struct Slot {
    /// The next Region to fill.
    wanted: Option<Arc<Region>>,
    running: Option<Arc<Region>>,
    worker: Option<ThreadId>,
}

/// One GPU's fills ahead of demand, one at a time.
#[derive(Default)]
pub(crate) struct Ahead {
    slot: Mutex<Slot>,
    settled: Condvar,
    wake: Condvar,
    /// The running fill was asked to stop: its wait for room ends.
    halt: AtomicBool,
}

impl Ahead {
    fn slot(&self) -> MutexGuard<'_, Slot> {
        self.slot.lock().unwrap_or_else(|error| error.into_inner())
    }

    /// Stop the running fill, unless it fills `keep`. Under the slot's lock, so it is that fill's.
    fn halt(&self, slot: &Slot, keep: Option<&Arc<Region>>) -> Option<Arc<Region>> {
        let running = slot.running.as_ref().filter(|running| keep.is_none_or(|keep| !Arc::ptr_eq(running, keep)))?;
        self.halt.store(true, Ordering::Release);
        Some(Arc::clone(running))
    }
}

/// Whether the running fill was asked to stop.
pub(crate) fn halted(device: &Device) -> bool {
    device.ahead.halt.load(Ordering::Acquire)
}

/// What comes after `region`: the Region observed next, else the Plan's next.
fn next(region: &Arc<Region>) -> Result<Option<Arc<Region>>> {
    let observed = {
        let state = region.gpu.lock()?;
        let next = state.entries.get(&region.id).and_then(|entry| entry.next);
        next.and_then(|id| state.entries.get(&id)).and_then(|entry| entry.region.upgrade())
    };
    match observed {
        Some(next) => Ok(Some(next)),
        None => plan::after(region.device(), region.id),
    }
}

/// How far past a use the core looks for what to fill: models that run between each other's
/// blocks (a LoRA's beside a DiT's) must not leave the one that matters a moment's notice.
const HORIZON: usize = 4;

/// The nearest absent Region after `region`, past resident ones, if it needs no caller. Never one
/// past it: that one's demand evicts a fill ahead first, so the guess is read twice (H3 Turbo at
/// 8 GiB on a 4090: 22 GB a call).
fn upcoming(region: &Arc<Region>) -> Result<Option<Arc<Region>>> {
    let mut at = Arc::clone(region);
    for _ in 0..HORIZON {
        let Some(next) = next(&at)? else { return Ok(None) };
        if next.uses.state() == Contents::Cold {
            return Ok((!next.uses.unused.load(Ordering::Relaxed) && next.source.ahead()).then_some(next));
        }
        at = next;
    }
    Ok(None)
}

/// A use of `region` happened, or its demand fill is about to run: fill what comes next, if absent.
pub(crate) fn after_use(region: &Arc<Region>) -> Result<()> {
    if let Some(next) = upcoming(region)? {
        want(region.gpu, next)?;
    }
    Ok(())
}

fn want(device: &'static Device, region: Arc<Region>) -> Result<()> {
    let mut slot = device.ahead.slot();
    if slot.running.as_ref().is_some_and(|running| Arc::ptr_eq(running, &region)) {
        return Ok(());
    }
    if slot.worker.is_none() {
        let worker = std::thread::Builder::new().name(format!("varena-ahead-{}", device.ordinal()))
            .spawn(move || run(device))
            .map_err(|error| Error::Unsupported(format!("fill worker could not start: {error}")))?;
        slot.worker = Some(worker.thread().id());
    }
    let replaced = slot.wanted.replace(region);
    device.ahead.wake.notify_all();
    drop(slot);
    drop(replaced); // A Region's last drop needs the gate.
    Ok(())
}

fn run(device: &'static Device) {
    loop {
        let region = {
            let mut slot = device.ahead.slot();
            loop {
                if let Some(wanted) = slot.wanted.take() {
                    slot.running = Some(Arc::clone(&wanted));
                    device.ahead.halt.store(false, Ordering::Release);
                    break wanted;
                }
                slot = device.ahead.wake.wait(slot).unwrap_or_else(|error| error.into_inner());
            }
        };
        let filled = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| fill_ahead(&region)));
        // Then what comes after it: the room rule keeps the lead to about one Region.
        let chained = match filled {
            Ok(Ok(true)) => upcoming(&region).unwrap_or(None),
            Ok(_) => None,
            Err(_) => {
                region.poison("fill worker panicked");
                None
            }
        };
        {
            let mut slot = device.ahead.slot();
            slot.running = None;
            device.ahead.settled.notify_all();
            if let Some(next) = chained
                && slot.wanted.is_none()
            {
                slot.wanted = Some(next);
            }
        }
    }
}

fn fill_ahead(region: &Arc<Region>) -> Result<bool> {
    let hold = match crate::enter_producer(region.device()) {
        Ok(hold) => hold,
        Err(Error::CaptureActive) => return Ok(false),
        Err(error) => return Err(error),
    };
    let Some(fill) = region.prepare_ahead()? else { return Ok(false) };
    let filled = region.source.fill(fill, None);
    drop(hold);
    match filled {
        Ok(_) => Ok(true),
        Err(Error::Poisoned(reason)) => Err(Error::Poisoned(reason)),
        Err(_) => Ok(false), // A failed guess costs its read. Demand tries again and reports.
    }
}

/// A demand fill of `region` goes first: what runs ahead of it stops, unless it is that Region.
pub(crate) fn yield_to(region: &Arc<Region>) {
    let mut slot = region.gpu.ahead.slot();
    let wanted = slot.wanted.take();
    let running = region.gpu.ahead.halt(&slot, Some(region));
    drop(slot);
    if let Some(running) = running {
        running.source.stop();
    }
    drop(wanted);
}

/// Wait for a fill the worker has of `region`. False: it has none.
pub(crate) fn wait_for(region: &Arc<Region>) -> bool {
    let mut slot = region.gpu.ahead.slot();
    let running = |slot: &Slot| slot.running.as_ref().is_some_and(|running| Arc::ptr_eq(running, region));
    if !running(&slot) {
        return false;
    }
    while running(&slot) {
        slot = region.gpu.ahead.settled.wait(slot).unwrap_or_else(|error| error.into_inner());
    }
    true
}

/// Stop what runs ahead of demand, and wait for it: its pages go first. False: nothing did.
/// Never from the worker, which is what would be waited for.
pub(crate) fn stop_and_wait(device: &Device) -> bool {
    let mut slot = device.ahead.slot();
    let wanted = slot.wanted.take();
    let on_worker = slot.worker == Some(std::thread::current().id());
    let running = if on_worker { None } else { device.ahead.halt(&slot, None) };
    drop(slot);
    drop(wanted);
    let Some(running) = running else { return false };
    running.source.stop();
    wait_for(&running);
    true
}

/// A capture is about to start: nothing may stay behind it.
pub(crate) fn stop(device: i32) -> Result<()> {
    let device = gpu(device)?;
    let mut slot = device.ahead.slot();
    let wanted = slot.wanted.take();
    let running = device.ahead.halt(&slot, None);
    drop(slot);
    drop(wanted);
    if let Some(running) = running {
        running.source.stop();
    }
    Ok(())
}
