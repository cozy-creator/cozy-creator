//! Process-local working allocations and prepared graph pools, sharing the weight pager's
//! capture exclusion. Streams are native CUstream handles in the device's primary context.
//! A graph owner must call `pool_capture_complete` after CUDA capture actually ends and
//! `pool_record_stream` after every replay/consumer submission before dropping its reference.

use crate::{Error, GlobalCaptureGuard, Result, cuda};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock};

pub type PoolId = (u64, u64);

#[derive(Clone, Copy, Debug, Default)]
pub struct WorkingStats {
    pub allocated_bytes: usize,
    pub allocated_peak: usize,
    pub allocated_capacity: usize,
    pub mapped_bytes: usize,
    pub mapped_peak: usize,
    pub pending_bytes: usize,
    pub free_bytes: usize,
    pub unassigned_bytes: usize,
    pub pools_bytes: usize,
    pub pools: usize,
    pub event_handles: usize,
    pub allocations: u64,
    pub frees: u64,
    pub ooms: u64,
    pub driver_memory_calls: u64,
    pub event_queries: u64,
    pub event_records: u64,
    pub event_waits: u64,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Life {
    Live,
    Ready,
    Pending,
    Deferred,
    Captured,
}

struct Block {
    segment: u64,
    capacity: usize,
    requested: usize,
    stream: u64,
    origin_retired: bool,
    streams: BTreeSet<u64>,
    events: BTreeMap<u64, usize>,
    life: Life,
    // None means all previous work is complete. Some(stream) permits ordered reuse only there.
    reusable_on: Option<u64>,
}

struct Segment {
    bytes: usize,
    handle: u64,
    pool: Option<PoolId>,
    cursor: usize,
}

#[derive(Default)]
struct Pool {
    prepared: bool,
    references: usize,
    routing: bool,
    capture: Option<GlobalCaptureGuard>,
    streams: BTreeSet<u64>,
    events: BTreeMap<u64, usize>,
}

#[derive(Default)]
struct State {
    segments: BTreeMap<u64, Segment>,
    blocks: BTreeMap<u64, Block>,
    // (pool, size class, originating stream). Cross-stream candidates are promoted by poll.
    free: BTreeMap<(Option<PoolId>, usize, Option<u64>), Vec<u64>>,
    addresses: BTreeMap<u64, usize>,
    pools: BTreeMap<PoolId, Pool>,
    stats: WorkingStats,
    poison: Option<String>,
}

struct Working {
    driver: cuda::Driver,
    page: usize,
    state: Mutex<State>,
}

static DEVICES: OnceLock<Mutex<BTreeMap<i32, Arc<Working>>>> = OnceLock::new();

fn devices() -> Result<MutexGuard<'static, BTreeMap<i32, Arc<Working>>>> {
    crate::check_process()?;
    DEVICES
        .get_or_init(Mutex::default)
        .lock()
        .map_err(|_| Error::Poisoned("working allocator registry".into()))
}

fn existing(device: i32) -> Result<Option<Arc<Working>>> {
    Ok(devices()?.get(&device).cloned())
}

fn owner(device: i32) -> Result<Arc<Working>> {
    existing(device)?.ok_or(Error::Invalid(
        "working allocator is not initialized on this device",
    ))
}

fn rounded(bytes: usize, align: usize) -> Result<usize> {
    bytes
        .checked_add(align - 1)
        .map(|n| n / align * align)
        .ok_or(Error::OutOfMemory {
            requested_bytes: bytes,
        })
}

fn size_class(bytes: usize, page: usize) -> Result<usize> {
    if bytes > 1 << 20 {
        return rounded(bytes, page);
    }
    let power = bytes
        .max(512)
        .checked_next_power_of_two()
        .ok_or(Error::OutOfMemory {
            requested_bytes: bytes,
        })?;
    let middle = rounded(power / 2 + power / 4, 512)?;
    Ok(if middle >= bytes && middle >= 512 {
        middle
    } else {
        power
    })
}

impl Working {
    fn lock(&self) -> Result<MutexGuard<'_, State>> {
        let state = self
            .state
            .lock()
            .map_err(|_| Error::Poisoned("working allocator mutex".into()))?;
        if let Some(reason) = &state.poison {
            return Err(Error::Poisoned(reason.clone()));
        }
        Ok(state)
    }

    fn fail(&self, state: &mut State, error: Error) -> Error {
        if !matches!(
            error,
            Error::OutOfMemory { .. } | Error::Invalid(_) | Error::CaptureActive | Error::Busy
        ) {
            state.poison = Some(error.to_string());
        }
        error
    }

    fn record(&self, events: &mut BTreeMap<u64, usize>, stream: u64) -> Result<()> {
        let event = match events.get(&stream) {
            Some(event) => *event,
            None => {
                let event = self.driver.create_event()?;
                events.insert(stream, event);
                event
            }
        };
        self.driver.record(event, stream as usize)
    }

    fn record_free(&self, block: &mut Block) -> Result<()> {
        for stream in &block.streams {
            self.record(&mut block.events, *stream)?;
        }
        block.life = if block.streams.len() <= 1 {
            Life::Ready
        } else {
            Life::Pending
        };
        block.reusable_on = block.streams.first().copied();
        Ok(())
    }

    fn completed(&self, events: &BTreeMap<u64, usize>) -> Result<bool> {
        for event in events.values() {
            if !self.driver.query(*event)? {
                return Ok(false);
            }
        }
        Ok(true)
    }

    fn poll(&self, state: &mut State) -> Result<()> {
        let mut ready = Vec::new();
        for (ptr, block) in &mut state.blocks {
            if block.life == Life::Deferred {
                self.record_free(block)?;
                if block.life == Life::Ready {
                    ready.push((*ptr, block.capacity, block.reusable_on, block.segment));
                }
            }
            if matches!(block.life, Life::Ready | Life::Pending)
                && block.reusable_on.is_some()
                && self.completed(&block.events)?
            {
                block.life = Life::Ready;
                block.reusable_on = None;
                ready.push((*ptr, block.capacity, None, block.segment));
            }
        }
        for (ptr, bytes, stream, segment) in ready {
            state
                .free
                .entry((state.segments[&segment].pool, bytes, stream))
                .or_default()
                .push(ptr);
        }
        Ok(())
    }

    fn take(
        &self,
        state: &mut State,
        bytes: usize,
        stream: u64,
        pool: Option<PoolId>,
    ) -> Option<u64> {
        for origin in [Some(stream), None] {
            let slots = state.free.get_mut(&(pool, bytes, origin));
            if let Some(slots) = slots {
                while let Some(ptr) = slots.pop() {
                    let Some(block) = state.blocks.get_mut(&ptr) else {
                        continue;
                    };
                    let reusable = block.life == Life::Ready
                        || (block.life == Life::Captured && origin.is_some());
                    if !reusable || block.capacity != bytes || block.reusable_on != origin {
                        continue;
                    }
                    block.life = Life::Live;
                    block.stream = stream;
                    block.origin_retired = false;
                    block.streams.clear();
                    block.streams.insert(stream);
                    return Some(ptr);
                }
            }
        }
        None
    }

    fn activate(state: &mut State, ptr: u64, requested: usize) -> u64 {
        state
            .blocks
            .get_mut(&ptr)
            .expect("allocated block")
            .requested = requested;
        state.stats.allocated_bytes += requested;
        state.stats.allocated_peak = state.stats.allocated_peak.max(state.stats.allocated_bytes);
        state.stats.allocations += 1;
        ptr
    }

    fn address(state: &mut State, bytes: usize) -> Result<u64> {
        let (ptr, len) = state
            .addresses
            .iter()
            .filter(|(_, n)| **n >= bytes)
            .min_by_key(|(_, n)| **n)
            .map(|(p, n)| (*p, *n))
            .ok_or(Error::OutOfMemory {
                requested_bytes: bytes,
            })?;
        state.addresses.remove(&ptr);
        if len > bytes {
            state.addresses.insert(ptr + bytes as u64, len - bytes);
        }
        Ok(ptr)
    }

    fn return_address(state: &mut State, mut ptr: u64, mut bytes: usize) {
        if let Some((left, len)) = state
            .addresses
            .range(..ptr)
            .next_back()
            .map(|(p, n)| (*p, *n))
            && left + len as u64 == ptr
        {
            state.addresses.remove(&left);
            ptr = left;
            bytes += len;
        }
        if let Some(len) = state.addresses.remove(&(ptr + bytes as u64)) {
            bytes += len;
        }
        state.addresses.insert(ptr, bytes);
    }

    fn map(
        &self,
        state: &mut State,
        bytes: usize,
        pool: Option<PoolId>,
        class: usize,
    ) -> Result<u64> {
        let ptr = Self::address(state, bytes)?;
        let handle = match self.driver.create(bytes) {
            Ok(handle) => handle,
            Err(error) => {
                Self::return_address(state, ptr, bytes);
                return Err(error);
            }
        };
        // Track ownership before map: a partial mapping failure must retain the VA and handle.
        state.segments.insert(
            ptr,
            Segment {
                bytes,
                handle,
                pool,
                cursor: 0,
            },
        );
        state.stats.mapped_bytes += bytes;
        state.stats.mapped_peak = state.stats.mapped_peak.max(state.stats.mapped_bytes);
        self.driver.map(ptr, bytes, handle)?;
        if pool.is_none() {
            for offset in (0..bytes).step_by(class) {
                if bytes - offset < class {
                    break;
                }
                let address = ptr + offset as u64;
                state.blocks.insert(
                    address,
                    Block {
                        segment: ptr,
                        capacity: class,
                        requested: 0,
                        stream: 0,
                        origin_retired: false,
                        streams: BTreeSet::new(),
                        events: BTreeMap::new(),
                        life: Life::Ready,
                        reusable_on: None,
                    },
                );
                state
                    .free
                    .entry((None, class, None))
                    .or_default()
                    .push(address);
            }
        }
        Ok(ptr)
    }

    fn carve_pool(
        &self,
        state: &mut State,
        pool: PoolId,
        bytes: usize,
        stream: u64,
    ) -> Option<u64> {
        let (base, segment) = state
            .segments
            .iter_mut()
            .filter(|(_, segment)| {
                segment.pool == Some(pool) && segment.bytes - segment.cursor >= bytes
            })
            .min_by_key(|(_, segment)| segment.bytes - segment.cursor)?;
        let ptr = *base + segment.cursor as u64;
        segment.cursor += bytes;
        state.blocks.insert(
            ptr,
            Block {
                segment: *base,
                capacity: bytes,
                requested: 0,
                stream,
                origin_retired: false,
                streams: BTreeSet::from([stream]),
                events: BTreeMap::new(),
                life: Life::Live,
                reusable_on: None,
            },
        );
        Some(ptr)
    }

    fn trim(&self, state: &mut State, limit: usize, exclude: Option<PoolId>) -> Result<usize> {
        self.poll(state)?;
        let mut released = 0usize;
        let mut candidates = Vec::new();
        for (base, segment) in &state.segments {
            if exclude.is_some() && segment.pool == exclude {
                continue;
            }
            if let Some(id) = segment.pool {
                let pool = &state.pools[&id];
                if pool.prepared
                    || pool.references != 0
                    || pool.capture.is_some()
                    || !self.completed(&pool.events)?
                {
                    continue;
                }
            }
            if state
                .blocks
                .range(*base..(*base + segment.bytes as u64))
                .all(|(_, block)| block.life == Life::Ready && block.reusable_on.is_none())
            {
                candidates.push(*base);
            }
        }
        for base in candidates {
            if released >= limit {
                break;
            }
            let segment = &state.segments[&base];
            self.driver.unmap(base, segment.bytes)?;
            self.driver.release(segment.handle)?;
            let bytes = segment.bytes;
            let blocks: Vec<_> = state
                .blocks
                .range(base..base + bytes as u64)
                .map(|(p, _)| *p)
                .collect();
            for ptr in blocks {
                let block = state.blocks.remove(&ptr).expect("retired block");
                for event in block.events.values() {
                    self.driver.destroy_event(*event)?;
                }
            }
            state.segments.remove(&base);
            state.stats.mapped_bytes -= bytes;
            Self::return_address(state, base, bytes);
            released += bytes;
        }
        state.free.retain(|_, slots| {
            slots.retain(|ptr| state.blocks.contains_key(ptr));
            !slots.is_empty()
        });
        let mut retired = Vec::new();
        for (id, pool) in &state.pools {
            if !pool.prepared
                && pool.references == 0
                && pool.capture.is_none()
                && !state
                    .segments
                    .values()
                    .any(|segment| segment.pool == Some(*id))
                && self.completed(&pool.events)?
            {
                retired.push(*id);
            }
        }
        for id in retired {
            for event in state.pools[&id].events.values() {
                self.driver.destroy_event(*event)?;
            }
            state.pools.remove(&id);
        }
        Ok(released)
    }

    fn wait_reclaimable(
        &self,
        state: &mut State,
        limit: usize,
        exclude: Option<PoolId>,
    ) -> Result<usize> {
        self.poll(state)?;
        let mut candidates = Vec::new();
        let mut bytes = 0usize;
        for (base, segment) in &state.segments {
            if exclude.is_some() && segment.pool == exclude {
                continue;
            }
            if let Some(id) = segment.pool {
                let pool = &state.pools[&id];
                if pool.prepared || pool.references != 0 || pool.capture.is_some() {
                    continue;
                }
            }
            if state
                .blocks
                .range(*base..*base + segment.bytes as u64)
                .all(|(_, block)| matches!(block.life, Life::Ready | Life::Pending))
            {
                candidates.push(*base);
                bytes = bytes.saturating_add(segment.bytes);
                if bytes >= limit {
                    break;
                }
            }
        }
        // Every event below has already been submitted. No live allocation, graph reference,
        // callback or future owner action is waited on. Only the allocation slow path calls this.
        for base in candidates {
            let segment = &state.segments[&base];
            if let Some(id) = segment.pool {
                for event in state.pools[&id].events.values() {
                    if !self.driver.query(*event)? {
                        self.driver.wait_event(*event)?;
                    }
                }
            }
            for (_, block) in state.blocks.range(base..base + segment.bytes as u64) {
                for event in block.events.values() {
                    if !self.driver.query(*event)? {
                        self.driver.wait_event(*event)?;
                    }
                }
            }
        }
        self.trim(state, limit, exclude)
    }
}

/// Open native device state before any graph capture. Repeated initialization is harmless.
pub fn initialize(device: i32) -> Result<()> {
    if existing(device)?.is_some() {
        return Ok(());
    }
    let captures = crate::capture_counts()?;
    crate::ensure_recovery_allowed(&captures, device)?;
    let mut devices = devices()?;
    if devices.contains_key(&device) {
        return Ok(());
    }
    let driver = cuda::Driver::open(device)?;
    let page = rounded(2 << 20, driver.granularity)?;
    let (base, bytes) = {
        let _context = driver.enter()?;
        let (_, total) = driver.memory_info()?;
        let bytes = rounded(
            total
                .checked_mul(8)
                .ok_or(Error::Invalid("virtual arena size overflow"))?,
            page,
        )?;
        (driver.reserve(bytes)?, bytes)
    };
    let mut state = State::default();
    state.addresses.insert(base, bytes);
    devices.insert(
        device,
        Arc::new(Working {
            driver,
            page,
            state: Mutex::new(state),
        }),
    );
    Ok(())
}

fn valid_stream(stream: u64) -> Result<()> {
    if stream == 2 {
        return Err(Error::Invalid(
            "per-thread default CUDA streams are unsupported",
        ));
    }
    Ok(())
}

/// Allocate from a prepared pool during capture, or the device's working slabs otherwise.
pub fn alloc(device: i32, bytes: usize, stream: u64, pool: Option<PoolId>) -> Result<u64> {
    valid_stream(stream)?;
    if bytes == 0 {
        return Ok(0);
    }
    if pool.is_none() {
        initialize(device)?;
    }
    let owner = owner(device)?;
    let class = size_class(bytes, owner.page)?;
    {
        let mut state = owner.lock()?;
        if let Some(pool) = pool {
            let session = state
                .pools
                .get(&pool)
                .ok_or(Error::Invalid("unprepared graph pool"))?;
            if !session.routing || !session.streams.contains(&stream) {
                return Err(Error::Invalid(
                    "allocation stream is not routed to this capture",
                ));
            }
        }
        if let Some(ptr) = owner.take(&mut state, class, stream, pool) {
            return Ok(Working::activate(&mut state, ptr, bytes));
        }
        if let Some(id) = pool {
            if let Some(ptr) = owner.carve_pool(&mut state, id, class, stream) {
                return Ok(Working::activate(&mut state, ptr, bytes));
            }
            state.stats.ooms += 1;
            return Err(Error::OutOfMemory {
                requested_bytes: bytes,
            });
        }
    }
    // Never hold working state while asking the weight pager to reclaim: it has its own lock.
    for attempt in 0..3 {
        let result = {
            let captures = crate::capture_counts()?;
            crate::ensure_recovery_allowed(&captures, device)?;
            let mut state = owner.lock()?;
            let _context = owner.driver.enter()?;
            let result = (|| {
                owner.poll(&mut state)?;
                if let Some(ptr) = owner.take(&mut state, class, stream, None) {
                    return Ok(Working::activate(&mut state, ptr, bytes));
                }
                let mapped = if class <= 1 << 20 { owner.page } else { class };
                if attempt == 2 {
                    owner.wait_reclaimable(&mut state, mapped, None)?;
                } else if attempt != 0 {
                    owner.trim(&mut state, mapped, None)?;
                }
                owner.map(&mut state, mapped, None, class)?;
                let ptr = owner
                    .take(&mut state, class, stream, None)
                    .expect("new slab has a block");
                Ok(Working::activate(&mut state, ptr, bytes))
            })();
            result.map_err(|error| owner.fail(&mut state, error))
        };
        match result {
            Err(Error::OutOfMemory { .. }) if attempt == 0 => {
                trim(device)?;
                crate::reclaim(device, class)?;
            }
            Err(Error::OutOfMemory { .. }) if attempt == 1 => continue,
            Err(error @ Error::OutOfMemory { .. }) => {
                owner.lock()?.stats.ooms += 1;
                return Err(error);
            }
            result => return result,
        }
    }
    unreachable!("finite allocation attempts")
}

pub fn free(device: i32, ptr: u64, stream: u64) -> Result<()> {
    if ptr == 0 {
        return Ok(());
    }
    valid_stream(stream)?;
    let owner = owner(device)?;
    let captures = crate::capture_counts()?;
    let capturing = crate::ensure_not_capturing(&captures, device).is_err();
    let mut state = owner.lock()?;
    let block = state
        .blocks
        .get(&ptr)
        .ok_or(Error::Invalid("unknown working allocation"))?;
    if block.life != Life::Live {
        return Err(Error::Invalid("working allocation already freed"));
    }
    let pool = state.segments[&block.segment].pool;
    let in_pool_capture = pool.is_some_and(|id| state.pools[&id].routing);
    let block = state.blocks.get_mut(&ptr).expect("live block");
    // Torch's deleter passes the original allocation stream, even after its owner drained
    // and destroyed that stream. An explicit later record_stream still tracks new consumers.
    if !block.origin_retired || stream != block.stream {
        block.streams.insert(stream);
    }
    let requested = block.requested;
    block.requested = 0;
    let class = block.capacity;
    if in_pool_capture {
        block.life = Life::Captured;
        block.reusable_on = if block.streams.len() == 1 {
            block.streams.first().copied()
        } else {
            None
        };
    } else if capturing {
        block.life = Life::Deferred;
    } else {
        let _context = owner.driver.enter()?;
        if let Err(error) = owner.record_free(block) {
            return Err(owner.fail(&mut state, error));
        }
    }
    if state.blocks[&ptr].life == Life::Ready
        || (state.blocks[&ptr].life == Life::Captured && state.blocks[&ptr].reusable_on.is_some())
    {
        let reusable_on = state.blocks[&ptr].reusable_on;
        state
            .free
            .entry((pool, class, reusable_on))
            .or_default()
            .push(ptr);
    }
    state.stats.allocated_bytes -= requested;
    state.stats.frees += 1;
    Ok(())
}

pub fn record_stream(device: i32, ptr: u64, stream: u64) -> Result<()> {
    if ptr == 0 {
        return Ok(());
    }
    valid_stream(stream)?;
    let owner = owner(device)?;
    let mut state = owner.lock()?;
    let block = state
        .blocks
        .get_mut(&ptr)
        .ok_or(Error::Invalid("unknown working allocation"))?;
    if block.life != Life::Live {
        return Err(Error::Invalid("record_stream after free"));
    }
    block.streams.insert(stream);
    Ok(())
}

pub fn record_stream_any(ptr: u64, stream: u64) -> Result<()> {
    if ptr == 0 {
        return Ok(());
    }
    valid_stream(stream)?;
    let candidates: Vec<_> = devices()?.values().cloned().collect();
    for owner in candidates {
        let mut state = owner.lock()?;
        if let Some((base, block)) = state.blocks.range_mut(..=ptr).next_back()
            && ptr - *base < block.capacity as u64
        {
            if block.life != Life::Live || ptr - *base >= block.requested as u64 {
                return Err(Error::Invalid(
                    "record_stream outside a live working allocation",
                ));
            }
            block.streams.insert(stream);
            return Ok(());
        }
        let mapped = state
            .segments
            .range(..=ptr)
            .next_back()
            .is_some_and(|(base, segment)| ptr - *base < segment.bytes as u64);
        let reserved = state
            .addresses
            .range(..=ptr)
            .next_back()
            .is_some_and(|(base, bytes)| ptr - *base < *bytes as u64);
        if mapped || reserved {
            return Err(Error::Invalid(
                "record_stream on unallocated working address space",
            ));
        }
    }
    // Native Torch ignores foreign DataPtr deleters. Their storage owner (for example a
    // TensorFS Region Pin) retains its own consumer/lifetime contract.
    Ok(())
}

/// Forget a stream only after its owner stopped submissions and synchronized it, before
/// destroying its handle. Live tensors may outlive the stream; new uses need record_stream.
pub fn retire_stream(device: i32, stream: u64) -> Result<()> {
    valid_stream(stream)?;
    if stream == 0 || stream == 1 {
        return Err(Error::Invalid(
            "the legacy default stream cannot be retired",
        ));
    }
    let Some(owner) = existing(device)? else {
        return Ok(());
    };
    let captures = crate::capture_counts()?;
    crate::ensure_not_capturing(&captures, device)?;
    let mut state = owner.lock()?;
    let _context = owner.driver.enter()?;
    if owner.driver.is_capturing(stream as usize)? {
        return Err(Error::CaptureActive);
    }
    if !owner.driver.query_stream(stream as usize)? {
        return Err(Error::Busy);
    }
    let result = (|| {
        for block in state.blocks.values_mut() {
            let origin = block.stream == stream && !block.origin_retired;
            if !origin
                && !block.streams.contains(&stream)
                && !block.events.contains_key(&stream)
                && block.reusable_on != Some(stream)
            {
                continue;
            }
            if origin {
                block.origin_retired = true;
            }
            block.streams.remove(&stream);
            if let Some(event) = block.events.remove(&stream) {
                owner.driver.destroy_event(event)?;
            }
            if block.life == Life::Live || block.life == Life::Captured {
                continue;
            }
            if block.life == Life::Deferred {
                owner.record_free(block)?;
            }
            if owner.completed(&block.events)? {
                block.life = Life::Ready;
                block.reusable_on = None;
            } else {
                block.life = if block.streams.len() == 1 {
                    Life::Ready
                } else {
                    Life::Pending
                };
                block.reusable_on = Some(block.streams.first().copied().unwrap_or(block.stream));
            }
        }
        for pool in state.pools.values_mut() {
            pool.streams.remove(&stream);
            if let Some(event) = pool.events.remove(&stream) {
                owner.driver.destroy_event(event)?;
            }
        }
        state.free.clear();
        let ready: Vec<_> = state
            .blocks
            .iter()
            .filter(|(_, block)| block.life == Life::Ready)
            .map(|(ptr, block)| (*ptr, block.capacity, block.reusable_on, block.segment))
            .collect();
        for (ptr, bytes, stream, segment) in ready {
            let pool = state.segments[&segment].pool;
            state
                .free
                .entry((pool, bytes, stream))
                .or_default()
                .push(ptr);
        }
        Ok(())
    })();
    result.map_err(|error| owner.fail(&mut state, error))
}

pub fn trim(device: i32) -> Result<usize> {
    let Some(owner) = existing(device)? else {
        return Ok(0);
    };
    let captures = crate::capture_counts()?;
    if crate::ensure_recovery_allowed(&captures, device).is_err() {
        return Ok(0);
    }
    let mut state = owner.lock()?;
    let _context = owner.driver.enter()?;
    owner
        .trim(&mut state, usize::MAX, None)
        .map_err(|error| owner.fail(&mut state, error))
}

pub fn trim_all() -> Result<usize> {
    let devices: Vec<_> = devices()?.keys().copied().collect();
    devices.into_iter().try_fold(0usize, |total, device| {
        Ok(total.saturating_add(trim(device)?))
    })
}

/// Weight fills call this while holding capture exclusion, before asking the driver for pages.
pub(crate) fn reclaim_for_weights(device: i32, bytes: usize) -> Result<usize> {
    let Some(owner) = existing(device)? else {
        return Ok(0);
    };
    let mut state = owner.lock()?;
    let _context = owner.driver.enter()?;
    let reclaimed = (|| {
        let completed = owner.trim(&mut state, bytes, None)?;
        if completed >= bytes {
            return Ok(completed);
        }
        let pending = owner.wait_reclaimable(&mut state, bytes - completed, None)?;
        Ok(completed.saturating_add(pending))
    })();
    reclaimed.map_err(|error| owner.fail(&mut state, error))
}

pub fn stats(device: i32) -> Result<WorkingStats> {
    let Some(owner) = existing(device)? else {
        return Ok(WorkingStats::default());
    };
    let state = owner.lock()?;
    let mut stats = state.stats;
    stats.allocated_capacity = state
        .blocks
        .values()
        .filter(|b| b.life == Life::Live)
        .map(|b| b.capacity)
        .sum();
    stats.pending_bytes = state
        .blocks
        .values()
        .filter(|b| b.life != Life::Live && (b.life != Life::Ready || b.reusable_on.is_some()))
        .map(|b| b.capacity)
        .sum();
    stats.free_bytes = state
        .blocks
        .values()
        .filter(|b| b.life == Life::Ready && b.reusable_on.is_none())
        .map(|b| b.capacity)
        .sum();
    stats.unassigned_bytes =
        stats.mapped_bytes - stats.allocated_capacity - stats.pending_bytes - stats.free_bytes;
    stats.pools_bytes = state
        .segments
        .values()
        .filter(|s| s.pool.is_some())
        .map(|s| s.bytes)
        .sum();
    stats.pools = state.pools.len();
    stats.event_handles = state
        .blocks
        .values()
        .map(|block| block.events.len())
        .sum::<usize>()
        + state
            .pools
            .values()
            .map(|pool| pool.events.len())
            .sum::<usize>();
    (
        stats.driver_memory_calls,
        stats.event_queries,
        stats.event_records,
        stats.event_waits,
    ) = owner.driver.calls();
    Ok(stats)
}

pub fn reset_peak(device: i32) -> Result<()> {
    let Some(owner) = existing(device)? else {
        return Ok(());
    };
    let mut state = owner.lock()?;
    state.stats.allocated_peak = state.stats.allocated_bytes;
    state.stats.mapped_peak = state.stats.mapped_bytes;
    Ok(())
}

/// Abandon unused preparation without dropping any graph's reference to a shared pool.
pub fn pool_cancel_preparation(device: i32, pool: PoolId) -> Result<()> {
    let Some(owner) = existing(device)? else {
        return Ok(());
    };
    let mut state = owner.lock()?;
    if let Some(pool) = state.pools.get_mut(&pool) {
        pool.prepared = false;
    }
    Ok(())
}
pub fn pool_reserve(device: i32, pool: PoolId, bytes: usize) -> Result<usize> {
    initialize(device)?;
    let owner = owner(device)?;
    let wanted = rounded(bytes, owner.page)?;
    for attempt in 0..3 {
        let result = {
            let captures = crate::capture_counts()?;
            crate::ensure_recovery_allowed(&captures, device)?;
            let mut state = owner.lock()?;
            let session = state.pools.entry(pool).or_default();
            if session.references == 0 {
                session.prepared = true;
            }
            let have: usize = state
                .segments
                .values()
                .filter(|segment| segment.pool == Some(pool))
                .map(|segment| segment.bytes)
                .sum();
            if have >= wanted {
                return Ok(have);
            }
            let _context = owner.driver.enter()?;
            let result = (|| {
                if attempt == 2 {
                    owner.wait_reclaimable(&mut state, wanted - have, Some(pool))?;
                } else {
                    owner.trim(&mut state, wanted - have, Some(pool))?;
                }
                owner.map(&mut state, wanted - have, Some(pool), 0)?;
                Ok(wanted)
            })();
            result.map_err(|error| owner.fail(&mut state, error))
        };
        match result {
            Err(Error::OutOfMemory { .. }) if attempt == 0 => {
                crate::reclaim(device, wanted)?;
            }
            Err(Error::OutOfMemory { .. }) if attempt == 1 => continue,
            other => return other,
        }
    }
    unreachable!("finite pool reserve attempts")
}

/// Register routing only. Repeated begin calls add streams to the same capture session.
pub fn pool_begin(device: i32, pool: PoolId, stream: u64) -> Result<()> {
    valid_stream(stream)?;
    let owner = owner(device)?;
    let guard = crate::enter_capture(device)?;
    let mut state = owner.lock()?;
    if state
        .pools
        .iter()
        .any(|(id, session)| *id != pool && session.capture.is_some())
    {
        return Err(Error::Invalid(
            "P0 capture sessions on one device must be sequential",
        ));
    }
    let pool = state.pools.entry(pool).or_default();
    if pool.capture.is_some() {
        if !pool.routing {
            return Err(Error::Invalid("previous CUDA capture has not completed"));
        }
    } else {
        pool.references = pool
            .references
            .checked_add(1)
            .ok_or(Error::Invalid("graph reference overflow"))?;
        pool.capture = Some(guard);
        pool.prepared = false;
        pool.routing = true;
        pool.streams.clear();
    }
    pool.streams.insert(stream);
    Ok(())
}

/// End allocation routing. This hook alone never proves CUDA capture has ended.
pub fn pool_end(device: i32, id: PoolId) -> Result<()> {
    let owner = owner(device)?;
    let mut state = owner.lock()?;
    let pool = state
        .pools
        .get_mut(&id)
        .ok_or(Error::Invalid("unknown graph pool"))?;
    if !pool.routing {
        return Err(Error::Invalid("graph pool routing is not active"));
    }
    pool.routing = false;
    Ok(())
}

/// The graph owner calls this only after cudaStreamEndCapture, including failed captures.
pub fn pool_capture_complete(device: i32, id: PoolId) -> Result<()> {
    let owner = owner(device)?;
    let _captures = crate::capture_counts()?;
    let mut state = owner.lock()?;
    let pool = state
        .pools
        .get_mut(&id)
        .ok_or(Error::Invalid("unknown graph pool"))?;
    if pool.routing {
        return Err(Error::Invalid("graph pool routing is still active"));
    }
    if pool.capture.is_none() {
        return Err(Error::Invalid("graph capture is already complete"));
    }
    // Auxiliary streams (Torch RNG setup, for example) execute eagerly during capture.
    // Their frees are metadata-only until now, so fence them before permitting pool retirement.
    let completion = (|| {
        let _context = owner.driver.enter()?;
        for stream in &pool.streams {
            if owner.driver.is_capturing(*stream as usize)? {
                return Err(Error::CaptureActive);
            }
        }
        for stream in &pool.streams {
            owner.record(&mut pool.events, *stream)?;
        }
        Ok(())
    })();
    if let Err(error) = completion {
        return Err(owner.fail(&mut state, error));
    }
    let guard = pool
        .capture
        .take()
        .ok_or(Error::Invalid("graph capture is already complete"))?;
    // Cross-stream capture frees may serve only a later, sequential capture/replay, never an
    // unordered stream in this capture. Live outputs remain live blocks.
    let mut ready = Vec::new();
    let segments: BTreeSet<_> = state
        .segments
        .iter()
        .filter(|(_, segment)| segment.pool == Some(id))
        .map(|(base, _)| *base)
        .collect();
    for (ptr, block) in &mut state.blocks {
        if segments.contains(&block.segment) && block.life == Life::Captured {
            block.life = Life::Ready;
            block.reusable_on = None;
            ready.push((*ptr, block.capacity));
        }
    }
    for (ptr, bytes) in ready {
        state
            .free
            .entry((Some(id), bytes, None))
            .or_default()
            .push(ptr);
    }
    drop(guard);
    Ok(())
}

/// Hold backing across a replay call whose end hooks may reset the original graph owner.
pub fn pool_retain(device: i32, id: PoolId) -> Result<()> {
    let owner = owner(device)?;
    let captures = crate::capture_counts()?;
    crate::ensure_not_capturing(&captures, device)?;
    let mut state = owner.lock()?;
    let pool = state
        .pools
        .get_mut(&id)
        .ok_or(Error::Invalid("unknown graph pool"))?;
    if pool.references == 0 {
        return Err(Error::Invalid("graph pool has no owner"));
    }
    pool.references = pool
        .references
        .checked_add(1)
        .ok_or(Error::Invalid("graph reference overflow"))?;
    Ok(())
}

pub fn pool_record_stream(device: i32, id: PoolId, stream: u64) -> Result<()> {
    valid_stream(stream)?;
    let owner = owner(device)?;
    let captures = crate::capture_counts()?;
    crate::ensure_not_capturing(&captures, device)?;
    let mut state = owner.lock()?;
    let _context = owner.driver.enter()?;
    let pool = state
        .pools
        .get_mut(&id)
        .ok_or(Error::Invalid("unknown graph pool"))?;
    if pool.references == 0 {
        return Err(Error::Invalid("graph pool has no owner"));
    }
    let result = owner.record(&mut pool.events, stream);
    result.map_err(|error| {
        // A replay may already be running. Even event-allocation OOM cannot release backing
        // without a completion fence; retain the arena until its process is retired.
        let reason = format!("graph completion fence failed: {error}");
        state.poison = Some(reason.clone());
        Error::Poisoned(reason)
    })
}

/// Release one graph reference. Physical reclamation is deferred to trim/pressure.
pub fn pool_release(device: i32, id: PoolId) -> Result<()> {
    let owner = owner(device)?;
    let mut state = owner.lock()?;
    let pool = state
        .pools
        .get_mut(&id)
        .ok_or(Error::Invalid("unknown graph pool"))?;
    if pool.references == 0 && pool.prepared {
        pool.prepared = false;
        return Ok(());
    }
    pool.references = pool
        .references
        .checked_sub(1)
        .ok_or(Error::Invalid("graph pool reference underflow"))?;
    Ok(())
}
