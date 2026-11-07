//! Page-addressed owner storage. No callback, eviction decision or future release promise.
//! Released pages become reclaimable only after their actual submitted stream fence.
use super::*;
use crate::reclaim::{self, Protect, Wait};
use std::sync::atomic::AtomicBool;

#[derive(Clone, Copy, PartialEq, Eq)]
enum PageLife {
    Preparing,
    Live,
    Pending(u64, u64),
    Ready,
}

struct Page {
    handle: u64,
    life: PageLife,
}

pub(super) struct Storage {
    base: u64,
    bytes: usize,
    pages: BTreeMap<usize, Page>,
    abandoned: bool,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct CacheStats {
    pub reserved_bytes: usize,
    pub mapped_bytes: usize,
    pub live_bytes: usize,
    pub pending_bytes: usize,
    pub ready_bytes: usize,
}

impl Storage {
    fn stats(&self, page: usize) -> CacheStats {
        let mut stats = CacheStats {
            reserved_bytes: self.bytes,
            ..CacheStats::default()
        };
        for held in self.pages.values() {
            stats.mapped_bytes += page;
            match held.life {
                PageLife::Live | PageLife::Preparing => stats.live_bytes += page,
                PageLife::Pending(..) => stats.pending_bytes += page,
                PageLife::Ready => stats.ready_bytes += page,
            }
        }
        stats
    }
}

/// A fixed VA range whose owner chooses the exact pages it can discard.
///
/// `release` requires all readers/writers joined into its supplied stream. The raw base
/// address is not a lease: the caller must retain this owner and must never access a
/// released range. `poll` collects completed releases without waiting for GPU work.
/// Close refuses live pages; dropping without releasing them retains their backing
/// until process exit rather than inventing a completion proof.
pub struct Cache {
    device: i32,
    id: u64,
    base: u64,
    bytes: usize,
    page: usize,
    pid: u32,
    closed: AtomicBool,
}

impl Cache {
    pub fn new(device: i32, reserve: usize) -> Result<Arc<Self>> {
        crate::check_process()?;
        if reserve == 0 {
            return Err(Error::Invalid("cache reservation must be nonempty"));
        }
        let owner = initialize_owner(device)?;
        crate::gate(device)?.not_capturing()?;
        let mut state = owner.lock()?;
        let bytes = rounded(reserve, owner.page())?;
        let id = state
            .next_cache
            .checked_add(1)
            .ok_or(Error::Invalid("cache identity overflow"))?;
        let base = Device::address(&mut state, bytes)?;
        state.next_cache = id;
        state.caches.insert(
            id,
            Storage {
                base,
                bytes,
                pages: BTreeMap::new(),
                abandoned: false,
            },
        );
        Ok(Arc::new(Self {
            device,
            id,
            base,
            bytes,
            page: owner.page(),
            pid: std::process::id(),
            closed: AtomicBool::new(false),
        }))
    }

    fn check(&self) -> Result<()> {
        crate::check_process()?;
        if self.pid != std::process::id() {
            return Err(Error::Invalid("cache used after fork"));
        }
        if self.closed.load(Ordering::Acquire) {
            return Err(Error::Invalid("cache is closed"));
        }
        Ok(())
    }

    fn range(&self, offset: usize, bytes: usize) -> Result<std::ops::Range<usize>> {
        self.check()?;
        let end = offset
            .checked_add(bytes)
            .ok_or(Error::Invalid("cache range overflow"))?;
        if bytes == 0 || offset % self.page != 0 || bytes % self.page != 0 || end > self.bytes {
            return Err(Error::Invalid(
                "cache range must be nonempty, page-aligned and inside its reservation",
            ));
        }
        Ok(offset..end)
    }

    pub fn base(&self) -> Result<u64> {
        self.check()?;
        Ok(self.base)
    }
    pub fn granularity(&self) -> usize {
        self.page
    }
    pub fn reserved_bytes(&self) -> usize {
        self.bytes
    }

    /// Commit an entirely unmapped range. Capacity refusal rolls back every new page,
    /// leaving all previously committed ranges intact. Device/mapping faults quarantine
    /// the Working owner with its exact handles/reservation retained.
    pub fn commit(&self, offset: usize, bytes: usize) -> Result<()> {
        let range = self.range(offset, bytes)?;
        reclaim::acquire(self.device, bytes, Wait::Yes, Protect::default(), || {
            self.commit_local(range.clone())
        })
    }

    fn commit_local(&self, range: std::ops::Range<usize>) -> crate::AllocationResult<()> {
        let bytes = range.end - range.start;
        let owner = owner(self.device)?;
        let gate = crate::gate(self.device)?;
        gate.not_capturing()?;
        let mut state = owner.lock()?;
        self.check()?;
        let _context = owner.driver().enter()?;
        let result = (|| -> crate::AllocationResult<()> {
            owner.poll(&mut state)?;
            trim_ready(owner, &mut state, usize::MAX, usize::MAX, Some(self.id))?;
            let held = state
                .caches
                .get(&self.id)
                .ok_or(Error::Invalid("cache is closed"))?;
            if range
                .clone()
                .step_by(self.page)
                .any(|at| held.pages.contains_key(&at))
            {
                return Err(Error::Invalid(
                    "cache commit overlaps committed or pending pages",
                )
                .into());
            }
            owner.sample()?;
            if owner.over(bytes) != 0 {
                return Err(Error::OutOfMemory {
                    requested_bytes: bytes,
                }
                .into());
            }
            for at in range.clone().step_by(self.page) {
                let handle = match owner.driver().create(self.page) {
                    Ok(handle) => handle,
                    Err(failure) => {
                        // Every earlier page reached a successful map/access operation.
                        // Roll back before the allocator may request any outside help.
                        rollback(owner, &mut state, self.id, range.start..at)?;
                        return Err(match failure {
                            crate::AllocationFailure::DriverOutOfMemory(_) => {
                                crate::AllocationFailure::DriverOutOfMemory(bytes)
                            }
                            other => other,
                        });
                    }
                };
                // Record physical ownership before map, exactly as ordinary Working.
                state
                    .caches
                    .get_mut(&self.id)
                    .expect("cache held under working lock")
                    .pages
                    .insert(
                        at,
                        Page {
                            handle,
                            life: PageLife::Preparing,
                        },
                    );
                owner.working.fetch_add(self.page, Ordering::AcqRel);
                owner.driver().map(self.base + at as u64, self.page, handle)?;
            }
            let held = state
                .caches
                .get_mut(&self.id)
                .expect("cache held under working lock");
            for at in range.clone().step_by(self.page) {
                held.pages.get_mut(&at).expect("committed page").life = PageLife::Live;
            }
            Ok(())
        })();
        result.map_err(|failure| owner.fail_allocation(&mut state, failure))
    }

    /// The owner has stopped using these pages and joined every consumer into `stream`.
    pub fn release(&self, offset: usize, bytes: usize, stream: u64) -> Result<()> {
        let range = self.range(offset, bytes)?;
        valid_stream(stream)?;
        let owner = owner(self.device)?;
        if owner.captures.load(Ordering::Acquire) != 0 {
            return Err(Error::CaptureActive);
        }
        let mut state = owner.lock()?;
        self.check()?;
        let held = state
            .caches
            .get_mut(&self.id)
            .ok_or(Error::Invalid("cache is closed"))?;
        if range
            .clone()
            .step_by(self.page)
            .any(|at| held.pages.get(&at).is_none_or(|p| p.life != PageLife::Live))
        {
            return Err(Error::Invalid(
                "cache release requires entirely live committed pages",
            ));
        }
        let mut stamp = Waits::new();
        owner.fences().stamp(stream, &mut stamp);
        let (id, sequence) = stamp[0];
        for at in range.step_by(self.page) {
            held.pages.get_mut(&at).expect("validated cache page").life =
                PageLife::Pending(id, sequence);
        }
        Ok(())
    }

    /// Nonblocking completion collection. This is the only explicit release progress call;
    /// ordinary native pressure and participant ticks also observe these submitted events.
    pub fn poll(&self) -> Result<CacheStats> {
        self.check()?;
        let owner = owner(self.device)?;
        let gate = crate::gate(self.device)?;
        gate.not_capturing()?;
        let mut state = owner.lock()?;
        self.check()?;
        let _context = owner.driver().enter()?;
        let result = (|| {
            owner.poll(&mut state)?;
            trim_ready(owner, &mut state, usize::MAX, usize::MAX, Some(self.id))?;
            Ok(state
                .caches
                .get(&self.id)
                .ok_or(Error::Invalid("cache is closed"))?
                .stats(self.page))
        })();
        result.map_err(|error| owner.fail(&mut state, error))
    }

    /// Metadata-only: neither a snapshot nor a queued owner action proves GPU completion.
    pub fn stats(&self) -> Result<CacheStats> {
        self.check()?;
        let owner = owner(self.device)?;
        let state = owner.lock()?;
        Ok(state
            .caches
            .get(&self.id)
            .ok_or(Error::Invalid("cache is closed"))?
            .stats(self.page))
    }

    /// Release the VA only after the owner released every page and its fences completed.
    pub fn close(&self) -> Result<()> {
        crate::check_process()?;
        if self.closed.load(Ordering::Acquire) {
            return Ok(());
        }
        let stats = self.poll()?;
        if stats.live_bytes != 0 {
            return Err(Error::Invalid("cache still owns live pages"));
        }
        if stats.pending_bytes != 0 {
            return Err(Error::Busy);
        }
        let owner = owner(self.device)?;
        let mut state = owner.lock()?;
        let held = state
            .caches
            .get(&self.id)
            .ok_or(Error::Invalid("cache is closed"))?;
        if !held.pages.is_empty() {
            return Err(Error::Busy);
        }
        let held = state
            .caches
            .remove(&self.id)
            .expect("empty cache held under working lock");
        Device::return_address(&mut state, held.base, held.bytes);
        self.closed.store(true, Ordering::Release);
        Ok(())
    }
}

impl Drop for Cache {
    fn drop(&mut self) {
        if self.pid != std::process::id() || self.closed.load(Ordering::Acquire) {
            return;
        }
        // No CUDA or GPU waits in finalization. Live pages have no completion proof.
        if let Ok(owner) = owner(self.device)
            && let Ok(mut state) = owner.lock()
            && let Some(held) = state.caches.get_mut(&self.id)
        {
            held.abandoned = true;
            retire_empty(&mut state);
        }
    }
}

fn rollback(
    owner: &Device,
    state: &mut State,
    id: u64,
    range: std::ops::Range<usize>,
) -> Result<()> {
    let held = state
        .caches
        .get_mut(&id)
        .expect("cache held under working lock");
    for at in range.step_by(owner.page()) {
        let page = &held.pages[&at];
        debug_assert!(page.life == PageLife::Preparing);
        owner.driver().unmap(held.base + at as u64, owner.page())?;
        owner.driver().release(page.handle)?;
        held.pages.remove(&at);
        owner.working.fetch_sub(owner.page(), Ordering::AcqRel);
        owner
            .released
            .fetch_add(owner.page() as u64, Ordering::AcqRel);
    }
    Ok(())
}

pub(super) fn poll(state: &mut State, fences: &crate::fence::Fences) {
    for held in state.caches.values_mut() {
        for page in held.pages.values_mut() {
            if let PageLife::Pending(id, sequence) = page.life
                && fences.complete(&[(id, sequence)])
            {
                page.life = PageLife::Ready;
            }
        }
    }
    retire_empty(state);
}

/// Only already-submitted owner releases, never a callback or future preemption.
pub(super) fn pending_events(
    state: &State,
    fences: &crate::fence::Fences,
    page: usize,
    wanted: usize,
    events: &mut Vec<usize>,
) {
    let mut selected = 0usize;
    for held in state.caches.values() {
        for held_page in held.pages.values() {
            if selected >= wanted {
                return;
            }
            if let PageLife::Pending(id, sequence) = held_page.life {
                fences.events(&[(id, sequence)], events);
                selected += page;
            }
        }
    }
}

pub(super) fn trim_ready(
    owner: &Device,
    state: &mut State,
    wanted: usize,
    maximum: usize,
    only: Option<u64>,
) -> Result<usize> {
    let mut released = 0;
    for (id, held) in &mut state.caches {
        if only.is_some_and(|only| only != *id) {
            continue;
        }
        let ready = held
            .pages
            .iter()
            .filter(|(_, page)| page.life == PageLife::Ready)
            .map(|(at, _)| *at)
            .collect::<Vec<_>>();
        for at in ready {
            if released >= wanted || owner.page() > maximum.saturating_sub(released) {
                break;
            }
            owner.driver().unmap(held.base + at as u64, owner.page())?;
            owner.driver().release(held.pages[&at].handle)?;
            held.pages.remove(&at);
            owner.working.fetch_sub(owner.page(), Ordering::AcqRel);
            owner
                .released
                .fetch_add(owner.page() as u64, Ordering::AcqRel);
            released += owner.page();
        }
    }
    retire_empty(state);
    Ok(released)
}

fn retire_empty(state: &mut State) {
    let retired = state
        .caches
        .iter()
        .filter(|(_, held)| held.abandoned && held.pages.is_empty())
        .map(|(id, _)| *id)
        .collect::<Vec<_>>();
    for id in retired {
        let held = state.caches.remove(&id).expect("retiring empty cache");
        Device::return_address(state, held.base, held.bytes);
    }
}

pub(super) fn working_stats(state: &State, page: usize, stats: &mut WorkingStats) {
    stats.caches = state.caches.len();
    for held in state.caches.values() {
        let cache = held.stats(page);
        stats.cache_reserved_bytes += cache.reserved_bytes;
        stats.cache_mapped_bytes += cache.mapped_bytes;
        stats.cache_live_bytes += cache.live_bytes;
        stats.cache_pending_bytes += cache.pending_bytes;
        stats.cache_ready_bytes += cache.ready_bytes;
    }
}

pub(super) fn ready_bytes(state: &State, page: usize) -> usize {
    state
        .caches
        .values()
        .map(|held| held.stats(page).ready_bytes)
        .sum()
}

