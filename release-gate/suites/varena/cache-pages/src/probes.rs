use super::*;
use std::sync::atomic::{AtomicBool, Ordering};

type StreamCreate = unsafe extern "C" fn(*mut Ctx, u32) -> i32;
type StreamOp = unsafe extern "C" fn(Ctx) -> i32;
type HostLaunch = unsafe extern "C" fn(Ctx, unsafe extern "C" fn(*mut c_void), *mut c_void) -> i32;

struct Gate {
    entered: AtomicBool,
    open: AtomicBool,
}
unsafe extern "C" fn blocked(data: *mut c_void) {
    // This callback performs no CUDA calls. The main thread owns its stable Box until sync.
    let gate = unsafe { &*(data as *const Gate) };
    gate.entered.store(true, Ordering::Release);
    while !gate.open.load(Ordering::Acquire) {
        std::thread::park_timeout(Duration::from_millis(1));
    }
}

struct Stream<'a> {
    _cuda: &'a Cuda,
    ptr: Ctx,
    sync: StreamOp,
    destroy: StreamOp,
    gate: Option<Box<Gate>>,
}
impl<'a> Stream<'a> {
    fn new(cuda: &'a Cuda) -> Result<Self> {
        let (create, sync, destroy) = unsafe {
            (
                *cuda._library.get::<StreamCreate>(b"cuStreamCreate\0")?,
                *cuda._library.get::<StreamOp>(b"cuStreamSynchronize\0")?,
                *cuda._library.get::<StreamOp>(b"cuStreamDestroy_v2\0")?,
            )
        };
        let mut ptr = std::ptr::null_mut();
        check("cuStreamCreate", unsafe { create(&mut ptr, 1) })?;
        Ok(Self {
            _cuda: cuda,
            ptr,
            sync,
            destroy,
            gate: None,
        })
    }
    fn block(&mut self) -> Result<()> {
        self.gate = Some(Box::new(Gate {
            entered: AtomicBool::new(false),
            open: AtomicBool::new(false),
        }));
        let gate = self.gate.as_deref().expect("owned gate");
        let launch = unsafe {
            *self
                ._cuda
                ._library
                .get::<HostLaunch>(b"cuLaunchHostFunc\0")?
        };
        check("cuLaunchHostFunc", unsafe {
            launch(self.ptr, blocked, (gate as *const Gate).cast_mut().cast())
        })?;
        let deadline = Instant::now() + Duration::from_secs(5);
        while !gate.entered.load(Ordering::Acquire) {
            require(
                Instant::now() < deadline,
                "actual submitted host gate did not start",
            )?;
            std::thread::sleep(Duration::from_millis(1));
        }
        Ok(())
    }
    fn finish(&self) -> Result<()> {
        if let Some(gate) = &self.gate {
            gate.open.store(true, Ordering::Release);
        }
        check("cuStreamSynchronize", unsafe { (self.sync)(self.ptr) })
    }
}
impl Drop for Stream<'_> {
    fn drop(&mut self) {
        // Error unwinding opens the owned gate too. Uncertain completion retains userdata.
        if self.finish().is_err() {
            if let Some(gate) = self.gate.take() {
                std::mem::forget(gate);
            }
        }
        unsafe {
            (self.destroy)(self.ptr);
        }
    }
}

fn sentinel(cuda: &Cuda, ptr: u64, bytes: usize, value: u8) -> Result<()> {
    cuda.fill(ptr, bytes, value)?;
    cuda.verify(&Block { ptr, bytes, value })
}
fn read(cuda: &Cuda, ptr: u64, bytes: usize, value: u8) -> Result<()> {
    cuda.verify(&Block { ptr, bytes, value })
}
fn caches_only(
    peer: &cooperative::Peer,
    live: usize,
    pending: usize,
    ready: usize,
) -> Result<Value> {
    let snapshot = accounting(peer)?;
    require(
        snapshot["cache_live"] == live
            && snapshot["cache_pending"] == pending
            && snapshot["cache_idle"] == ready,
        "exact native ledger Cache class accounting",
    )?;
    Ok(snapshot)
}

fn filled(
    arena: &std::sync::Arc<varena::Arena>,
    bytes: usize,
    value: u8,
) -> Result<(std::sync::Arc<varena::Region>, varena::Pin)> {
    let region = arena.reserve(bytes)?;
    let mut fill = match region.prepare()? {
        varena::Preparation::Fill(fill) => fill,
        _ => return Err(io::Error::other("new region unexpectedly resident").into()),
    };
    fill.copy_from_host(&vec![value; bytes])?;
    let pin = fill.commit()?;
    Ok((region, pin))
}

fn capture(cuda: &Cuda, peer: &cooperative::Peer, cache: &Cache, device: i32) -> Result<Value> {
    type Begin = unsafe extern "C" fn(Ctx, i32) -> i32;
    type End = unsafe extern "C" fn(Ctx, *mut Ctx) -> i32;
    let (begin, end, destroy) = unsafe {
        (
            *cuda._library.get::<Begin>(b"cuStreamBeginCapture_v2\0")?,
            *cuda._library.get::<End>(b"cuStreamEndCapture\0")?,
            *cuda._library.get::<StreamOp>(b"cuGraphDestroy\0")?,
        )
    };
    let stream = Stream::new(cuda)?;
    let guard = varena::enter_capture(device)?;
    check("cuStreamBeginCapture_v2", unsafe { begin(stream.ptr, 0) })?;
    let observed = (|| -> Result<Value> {
        let before = peer.cuda_calls()?;
        let page = cache.granularity();
        require(
            matches!(
                cache.commit(4 * page, page),
                Err(varena::Error::CaptureActive)
            ),
            "commit refused in capture",
        )?;
        require(
            matches!(
                cache.release(0, page, stream.ptr as u64),
                Err(varena::Error::CaptureActive)
            ),
            "release refused in capture",
        )?;
        require(
            matches!(cache.poll(), Err(varena::Error::CaptureActive)),
            "poll refused in capture",
        )?;
        require(
            matches!(cache.close(), Err(varena::Error::CaptureActive)),
            "close refused in capture",
        )?;
        require(
            matches!(Cache::new(device, page), Err(varena::Error::CaptureActive)),
            "new refused in capture",
        )?;
        let after = peer.cuda_calls()?;
        require(
            before == after,
            "Cache capture refusals made native driver calls",
        )?;
        Ok(
            json!({"actual_cuda_capture":true,"native_working_driver_before":before,"native_working_driver_after":after,
            "commit_release_poll_close_new_refused":true,"cache":cache_stats(cache)?}),
        )
    })();
    let mut graph = std::ptr::null_mut();
    let ended = unsafe { end(stream.ptr, &mut graph) };
    if ended != 0 {
        let _ =
            emit(&json!({"event":"capture_end_failed","code":ended,"retirement_quarantined":true}));
        unsafe {
            libc::_exit(3);
        }
    }
    drop(guard);
    if !graph.is_null() {
        check("cuGraphDestroy", unsafe { destroy(graph) })?;
    }
    observed
}

pub(super) fn pages(cuda: &Cuda, peer: &cooperative::Peer, device: i32) -> Result<Value> {
    let seed = Cache::new(device, 1)?;
    let page = seed.granularity();
    seed.close()?;
    let cache = Cache::new(device, 5 * page)?;
    require(
        cache.reserved_bytes() >= 5 * page,
        "fixture reservation covers five native pages",
    )?;
    let base = cache.base()?;
    cache.commit(0, 3 * page)?;
    for (at, value) in [(0, 17), (1, 29), (2, 41)] {
        sentinel(cuda, base + (at * page) as u64, page, value)?;
    }
    let initial = caches_only(peer, 3 * page, 0, 0)?;
    let token = working::phase_enter(
        device,
        Some("cache-pages-working-watermark".into()),
        working::PhaseMode::Eager,
    )?;
    cache.commit(3 * page, page)?;
    let activation = working::alloc(device, page, 0, None)?;
    sentinel(cuda, activation, page, 37)?;
    // Full readback synchronizes the only native working allocation's consumers.
    unsafe {
        working::free_completed(device, activation)?;
    }
    let phase = working::phase_leave(device, token, true, None)?;
    require(
        phase.published
            && phase.peak_bytes == page
            && phase.total_peak_bytes == page
            && phase.backing_peak_bytes == page,
        "Cache pages changed the actual working watermark",
    )?;
    let phase_cache = cache_stats(&cache)?;
    cache.release(3 * page, page, 0)?;
    cuda.synchronize()?;
    cache.poll()?;
    working::trim(device)?;
    require(
        working::phase_forget(device, "cache-pages-working-watermark")?,
        "temporary phase retired",
    )?;

    let pending = Cache::new(device, page)?;
    pending.commit(0, page)?;
    sentinel(cuda, pending.base()?, page, 53)?;
    let dropped = Cache::new(device, page)?;
    dropped.commit(0, page)?;
    sentinel(cuda, dropped.base()?, page, 67)?;
    let mut gate = Stream::new(cuda)?;
    gate.block()?;
    cache.release(page, page, gate.ptr as u64)?;
    pending.release(0, page, gate.ptr as u64)?;
    dropped.release(0, page, gate.ptr as u64)?;
    drop(dropped); // Native Drop occurred now; its submitted event still owns backing.
    require(
        cache.poll()?.pending_bytes == page,
        "middle page retains pending backing",
    )?;
    require(
        matches!(pending.close(), Err(varena::Error::Busy)),
        "pending-only close refused without waiting",
    )?;
    require(
        matches!(cache.commit(page, page), Err(varena::Error::Invalid(_))),
        "pending range cannot be recommitted",
    )?;
    let before_trim = working_stats(device)?;
    require(
        working::trim(device)? == 0,
        "trim released a range behind the blocked submitted fence",
    )?;
    let after_trim = working_stats(device)?;
    require(
        before_trim["memory_calls"] == after_trim["memory_calls"],
        "pending trim changed backing",
    )?;
    require(
        after_trim["caches"] == 3 && after_trim["cache_pending"] == 3 * page,
        "dropped pending owner retained exact pages",
    )?;
    let pending_accounting = caches_only(peer, 2 * page, 3 * page, 0)?;
    gate.finish()?;
    working::trim(device)?;
    require(
        working::stats(device)?.caches == 2,
        "dropped pending cache did not retire after actual completion",
    )?;
    pending.close()?;
    require(
        cache.base()? == base && cache.stats()?.mapped_bytes == 2 * page,
        "middle hole completed at fixed VA",
    )?;
    cache.commit(page, page)?;
    sentinel(cuda, base + page as u64, page, 73)?;
    read(cuda, base, page, 17)?;
    read(cuda, base + (2 * page) as u64, page, 41)?;
    let recommitted = cache_stats(&cache)?;
    require(
        matches!(cache.commit(0, page), Err(varena::Error::Invalid(_))),
        "overlap rejected",
    )?;
    require(
        matches!(cache.commit(1, page), Err(varena::Error::Invalid(_))),
        "unaligned offset rejected",
    )?;
    require(
        matches!(
            cache.release(0, page + 1, 0),
            Err(varena::Error::Invalid(_))
        ),
        "unaligned release rejected",
    )?;
    require(
        matches!(cache.close(), Err(varena::Error::Invalid(_))),
        "live close rejected",
    )?;

    varena::set_limit(device, Some(3 * page))?;
    let cap_before = working_stats(device)?;
    let cap_refusal = cache.commit(4 * page, page);
    varena::set_limit(device, None)?;
    require(
        matches!(cap_refusal, Err(varena::Error::OutOfMemory { .. })),
        "managed Cache capacity refusal",
    )?;
    let cap_after = working_stats(device)?;
    require(
        cap_before["physical_ooms"] == cap_after["physical_ooms"]
            && cache.stats()?.mapped_bytes == 3 * page,
        "local ceiling caused peer/physical exhaustion or changed old Cache ranges",
    )?;
    for (at, value) in [(0, 17), (1, 73), (2, 41)] {
        read(cuda, base + (at * page) as u64, page, value)?;
    }
    let captured = capture(cuda, peer, &cache, device)?;
    cache.release(0, 3 * page, 0)?;
    cuda.synchronize()?;
    cache.poll()?;
    cache.close()?;
    Ok(json!({"page":page,"base":base,"initial":initial,
        "phase":{"published":phase.published,"peak":phase.peak_bytes,"total_peak":phase.total_peak_bytes,
            "backing_peak":phase.backing_peak_bytes,"cache_during_phase":phase_cache},
        "pending":{"actual_host_gate_entered":true,"close_refused":true,"before_trim":before_trim,
            "after_trim":after_trim,"accounting":pending_accounting,"dropped_pending_retired":true},
        "interior_recommit":{"same_base":true,"neighbors_full_readback":true,"new_middle_value":73,"cache":recommitted},
        "local_cap":{"before":cap_before,"after":cap_after},"capture":captured}))
}

pub(super) fn rollback(cuda: &Cuda, peer: &cooperative::Peer, device: i32) -> Result<Value> {
    let seed = Cache::new(device, 1)?;
    let page = seed.granularity();
    seed.close()?;
    let total = cuda.memory()?["total"]
        .as_u64()
        .ok_or_else(|| io::Error::other("driver total"))? as usize;
    let requested = total.div_ceil(page) * page;
    let cache = Cache::new(device, requested + 4 * page)?;
    let base = cache.base()?;
    for (at, value) in [(0, 11), (2, 23)] {
        cache.commit(at * page, page)?;
        sentinel(cuda, base + (at * page) as u64, page, value)?;
    }
    let free = cuda.memory()?["free"]
        .as_u64()
        .ok_or_else(|| io::Error::other("driver free"))? as usize;
    let margin = 128 << 20;
    require(
        free > margin + 4 * page,
        "real driver headroom for physical rollback fixture",
    )?;
    let filler_bytes = (free - margin) / page * page;
    let filler = working::alloc(device, filler_bytes, 0, None)?;
    sentinel(cuda, filler, filler_bytes, 89)?;
    let actual_free = cuda.memory()?["free"]
        .as_u64()
        .ok_or_else(|| io::Error::other("driver free after filler"))?
        as usize;
    require(
        actual_free >= 4 * page && actual_free < requested,
        "partial-map headroom observed before actual impossible commit",
    )?;
    let cache_before = cache_stats(&cache)?;
    let before = working_stats(device)?;
    let result = cache.commit(4 * page, requested);
    require(
        matches!(result, Err(varena::Error::OutOfMemory { .. })),
        "actual full-device commit must exhaust beside live filler",
    )?;
    let after = working_stats(device)?;
    let cache_after = cache_stats(&cache)?;
    let oom = after["physical_ooms"].as_u64().unwrap() - before["physical_ooms"].as_u64().unwrap();
    let calls = after["memory_calls"].as_u64().unwrap() - before["memory_calls"].as_u64().unwrap();
    // In this isolated call there are no free working blocks, callbacks, arenas or automatic
    // peer worker. Each successfully mapped then rolled-back page makes create/map/access/
    // unmap/release calls; each failed create contributes one real driver call and OOM.
    require(
        oom == 3 && calls > oom && (calls - oom).is_multiple_of(5),
        "real partial multi-page mapping and rollback counters",
    )?;
    require(
        cache_before == cache_after && cache.base()? == base,
        "failed commit changed prior exact Cache ranges",
    )?;
    read(cuda, base, page, 11)?;
    read(cuda, base + (2 * page) as u64, page, 23)?;
    let observed = caches_only(peer, 2 * page, 0, 0)?;
    working::free(device, filler, 0)?;
    cuda.synchronize()?;
    working::trim(device)?;
    for at in [0, 2] {
        cache.release(at * page, page, 0)?;
    }
    cuda.synchronize()?;
    cache.poll()?;
    cache.close()?;
    Ok(
        json!({"requested_commit":requested,"page":page,"filler_bytes":filler_bytes,
        "explicit_margin":margin,"observed_free_before_commit":actual_free,"filler_full_readback":true,
        "before":before,"after":after,"physical_ooms":oom,"memory_call_delta":calls,
        "observed_mapped_rollback_pages":(calls-oom)/5,"cache_before":cache_before,"cache_after":cache_after,
        "old_ranges_full_readback":true,"same_base":true,"accounting":observed}),
    )
}

pub(super) fn abandoned(cuda: &Cuda, peer: &cooperative::Peer, device: i32) -> Result<Value> {
    let cache = Cache::new(device, 1)?;
    let page = cache.granularity();
    let base = cache.base()?;
    cache.commit(0, page)?;
    sentinel(cuda, base, page, 101)?;
    drop(cache); // Exact native last owner, with no release or completion contract supplied.
    let before = working_stats(device)?;
    require(
        working::trim(device)? == 0,
        "abandoned live cache must not be inferred free",
    )?;
    let after = working_stats(device)?;
    require(
        before["memory_calls"] == after["memory_calls"]
            && after["cache_live"] == page
            && after["caches"] == 1,
        "unproven live backing changed after native owner drop",
    )?;
    // Deliberate fixture read of the retained unproven alias; no supported reuse is claimed.
    read(cuda, base, page, 101)?;
    let observed = caches_only(peer, page, 0, 0)?;
    Ok(
        json!({"page":page,"base":base,"before":before,"after":after,"full_retained_readback":true,
        "accounting":observed,"boundary":"unreleased native last-owner drop retains backing until this isolated process exits; no completion or free capacity is invented"}),
    )
}

pub(super) fn choice(cuda: &Cuda, peer: &cooperative::Peer, device: i32) -> Result<Value> {
    choice_with_plan(cuda, peer, device, false)
}

pub(super) fn choice_plan(cuda: &Cuda, peer: &cooperative::Peer, device: i32) -> Result<Value> {
    choice_with_plan(cuda, peer, device, true)
}

fn choice_with_plan(
    cuda: &Cuda,
    peer: &cooperative::Peer,
    device: i32,
    planned: bool,
) -> Result<Value> {
    let seed = Cache::new(device, 1)?;
    let page = seed.granularity();
    seed.close()?;
    let weight = if planned {
        let arena = varena::Arena::new(device)?;
        let (region, pin) = filled(&arena, page, 173)?;
        let plan = varena::plan::Plan::new(
            std::sync::Arc::clone(&arena),
            vec![(
                "cache-choice-weight".into(),
                vec![std::sync::Arc::clone(&region)],
            )],
        )?;
        plan.activate("cache-choice-weight")?;
        Some((arena, region, pin, plan))
    } else {
        None
    };
    let cache = Cache::new(device, 2 * page)?;
    cache.commit(0, 2 * page)?;
    sentinel(cuda, cache.base()?, 2 * page, 113)?;
    let limit = (4 + usize::from(planned)) * page;
    varena::set_limit(device, Some(limit))?;
    let key = "cache-ready-choice";
    let select = || {
        working::split_select(
            device,
            key,
            vec![
                ("whole".into(), vec![4 * page]),
                ("half".into(), vec![2 * page]),
            ],
            "whole",
            0,
        )
    };
    let live = select()?;
    require(
        live.choice_id == "half",
        "live Cache pages were credited to the larger choice",
    )?;
    let mut gate = Stream::new(cuda)?;
    gate.block()?;
    cache.release(0, 2 * page, gate.ptr as u64)?;
    let pending_accounting = caches_only(peer, 0, 2 * page, 0)?;
    let pending = select()?;
    require(
        pending.choice_id == "half",
        "pending Cache pages were credited before actual completion",
    )?;
    gate.finish()?;
    // Publication queries the actual fence but does not trim the newly Ready Cache pages.
    let ready_accounting = caches_only(peer, 0, 0, 2 * page)?;
    require(
        cache.stats()?.mapped_bytes == 2 * page && cache.stats()?.ready_bytes == 2 * page,
        "choice comparison requires physically mapped, completed owner releases",
    )?;
    let ready = select()?;
    let selections = json!({"event":"choices","page":page,"managed_limit":limit,
        "declared_phase_weight_bytes":if planned { page } else { 0 },
        "plan":{"generation":ready.plan_generation,"weight_bytes":ready.phase_weight_bytes,
                "live_preserves":live.preserves_phase_weights,"pending_preserves":pending.preserves_phase_weights,
                "ready_preserves":ready.preserves_phase_weights},
        "live":{"choice":live.choice_id,"required":live.required_bytes,"available":live.available_bytes},
        "pending":{"choice":pending.choice_id,"required":pending.required_bytes,"available":pending.available_bytes},
        "ready":{"choice":ready.choice_id,"required":ready.required_bytes,"available":ready.available_bytes},
        "pending_accounting":pending_accounting,"ready_accounting":ready_accounting});
    emit(&selections)?; // A before-fix refusal retains the actual comparison, not just an assertion.
    require(
        ready.choice_id == "whole" && ready.available_bytes == 4 * page,
        "completed Cache pages did not admit the larger declared choice",
    )?;
    if planned {
        require(
            ready.plan_generation != 0
                && ready.phase_weight_bytes == page
                && ready.preserves_phase_weights
                && live.preserves_phase_weights
                && pending.preserves_phase_weights,
            "Cache choice did not preserve the declared real phase weight",
        )?;
    }
    let token = working::phase_enter(device, Some(key.into()), working::PhaseMode::Eager)?;
    let site = working::split_begin(
        device,
        key,
        &ready.choice_id,
        Some(token),
        0,
        Some(ready.plan_generation),
    )?;
    let after_begin = cache_stats(&cache)?;
    require(
        after_begin["mapped"] == 0,
        "split begin did not reclaim actual completed Cache backing",
    )?;
    let output = working::alloc(device, 4 * page, 0, None)?;
    sentinel(cuda, output, 4 * page, 131)?;
    unsafe {
        working::free_completed(device, output)?;
    }
    let observed = working::split_end(device, site, true)?;
    let phase = working::phase_leave(device, token, true, None)?;
    require(
        observed.pending
            && phase.published
            && phase.peak_bytes == 4 * page
            && phase.backing_peak_bytes == 4 * page,
        "chosen whole operation did not publish actual working demand",
    )?;
    require(
        working::phase_forget(device, key)?,
        "choice/phase measurement retired",
    )?;
    working::trim(device)?;
    cache.close()?;
    varena::set_limit(device, None)?;
    if let Some((arena, region, pin, plan)) = weight {
        read(cuda, region.ptr(), page, 173)?;
        cuda.synchronize()?;
        plan.deactivate()?;
        drop(unsafe { pin.into_completed_region() });
        arena.reclaim(page)?;
        require(arena.stats()?.resident_bytes == 0, "planned weight cleanup")?;
    }
    Ok(
        json!({"selection":selections,"cache_after_begin":after_begin,"full_readback_bytes":4 * page,
        "full_weight_readback_bytes":if planned { page } else { 0 },
        "phase":{"published":phase.published,"peak":phase.peak_bytes,"backing_peak":phase.backing_peak_bytes}}),
    )
}

pub(super) fn owners(cuda: &Cuda, peer: &cooperative::Peer, device: i32) -> Result<Value> {
    use std::sync::{Arc, Mutex};
    use varena::cache_owner::{CallbackResult, Cause, RequestKey};
    let first = Cache::new(device, 1)?;
    let page = first.granularity();
    let second = Cache::new(device, page)?;
    first.commit(0, page)?;
    second.commit(0, page)?;
    sentinel(cuda, first.base()?, page, 137)?;
    sentinel(cuda, second.base()?, page, 149)?;
    let arena = varena::Arena::new(device)?;
    let (pending_weight, pending_pin) = filled(&arena, page, 151)?;
    let (unjoined_weight, unjoined_pin) = filled(&arena, page, 157)?;
    let unjoined_owner = unjoined_pin.into_unpinned_region();
    let cold_weight = arena.reserve(page)?;
    let mut cold_fill = match cold_weight.prepare()? {
        varena::Preparation::Fill(fill) => fill,
        _ => return Err(io::Error::other("new cold region unexpectedly resident").into()),
    };
    cold_fill.copy_from_host(&vec![163; page])?;
    drop(cold_fill); // Real unfinished publication: Cold backing, unannotated release.
    let mut gate = Stream::new(cuda)?;
    gate.block()?;
    let pending_owner = unsafe { pending_pin.into_fenced_region(&[gate.ptr as u64]) };
    let weights_before = arena.stats()?;
    let lock = Arc::new(Mutex::new(()));
    let held = lock
        .lock()
        .map_err(|_| io::Error::other("fixture owner lock"))?;
    let first_lock = Arc::clone(&lock);
    let first_cache = Arc::clone(&first);
    let first_owner = varena::CacheOwner::new(device, move |_| {
        let _owner = first_lock.try_lock().map_err(|_| varena::Error::Busy)?;
        first_cache.release(0, page, 0)?;
        Ok(CallbackResult::Returned)
    })?;
    let second_cache = Arc::clone(&second);
    let second_owner = varena::CacheOwner::new(device, move |_| {
        second_cache.release(0, page, 0)?;
        Ok(CallbackResult::Returned)
    })?;
    let key = RequestKey {
        cause: Cause::Allocation,
        generation: varena::cache_owner::next_generation()?,
    };
    let before = working_stats(device)?;
    let request = varena::cache_owner::request_shrink(device, key, page)?
        .ok_or_else(|| io::Error::other("registered native cache owners unavailable"))?;
    let deadline = Instant::now() + Duration::from_secs(5);
    let progress = loop {
        let facts = request.facts()?;
        if facts.finished {
            break facts;
        }
        require(
            Instant::now() < deadline,
            "two-owner completion protocol deadline; no release inferred",
        )?;
        std::thread::sleep(Duration::from_millis(1));
    };
    let first_facts = first_owner.facts()?;
    let second_facts = second_owner.facts()?;
    require(
        first_facts.callbacks == 1 && first_facts.declined == 1 && first_facts.failed.is_none(),
        "busy first owner was retried or treated as a device-wide deferral",
    )?;
    require(
        second_facts.callbacks == 1 && second_facts.returned == 1 && second_facts.failed.is_none(),
        "second available owner was starved",
    )?;
    require(
        progress.returned == 2
            && progress.declined == 1
            && progress.failed == 0
            && progress.observed_release_bytes >= page as u64,
        "owner return was confused with physical progress",
    )?;
    require(
        first.stats()?.live_bytes == page && second.stats()?.mapped_bytes == 0,
        "wrong native owner backing was reclaimed",
    )?;
    let weights_after = arena.stats()?;
    require(
        weights_after.resident_bytes == 3 * page
            && weights_after.context_joins == weights_before.context_joins
            && weights_after.event_waits == weights_before.event_waits,
        "completed-only owner collection waited on or credited Pending/Unjoined weights",
    )?;
    let observed = caches_only(peer, page, 0, 0)?;
    require(
        observed["weight_pending"] == 3 * page
            && observed["weight_idle"] == 0
            && observed["pending_release"] == page,
        "only the actual submitted weight fence may appear as pending release",
    )?;
    let after = working_stats(device)?;
    require(
        before["cache_mapped"].as_u64().unwrap() - after["cache_mapped"].as_u64().unwrap()
            == page as u64,
        "actual mapped Cache reduction differs from the successful owner",
    )?;
    gate.finish()?;
    read(cuda, first.base()?, page, 137)?;
    read(cuda, pending_weight.ptr(), page, 151)?;
    read(cuda, unjoined_weight.ptr(), page, 157)?;
    read(cuda, cold_weight.ptr(), page, 163)?;
    // Explicit cleanup may join unannotated weights, after the real gate opens.
    arena.reclaim(usize::MAX)?;
    require(
        arena.stats()?.resident_bytes == 0,
        "explicit weight cleanup incomplete",
    )?;
    drop((
        pending_owner,
        unjoined_owner,
        pending_weight,
        unjoined_weight,
        cold_weight,
    ));
    drop(held);
    first_owner.close()?;
    second_owner.close()?;
    require(
        first_owner.facts()?.retired
            && second_owner.facts()?.retired
            && varena::cache_owner::requestable(device)? == 0,
        "explicit native callback retirement incomplete",
    )?;
    first.release(0, page, 0)?;
    cuda.synchronize()?;
    first.poll()?;
    first.close()?;
    second.close()?;
    Ok(
        json!({"page":page,"request_generation":key.generation,"real_owner_lock_held":true,
        "first":{"callbacks":first_facts.callbacks,"declined":first_facts.declined},
        "second":{"callbacks":second_facts.callbacks,"returned":second_facts.returned},
        "request":{"returned":progress.returned,"declined":progress.declined,"failed":progress.failed,
            "observed_physical_release":progress.observed_release_bytes},
        "before":before,"after":after,"accounting":observed,"first_full_readback":true,
        "weights":{"resident_before":weights_before.resident_bytes,"resident_after":weights_after.resident_bytes,
            "context_joins_before":weights_before.context_joins,"context_joins_after":weights_after.context_joins,
            "event_waits_before":weights_before.event_waits,"event_waits_after":weights_after.event_waits,
            "pending_and_unjoined_full_readback":true},
        "both_callback_owners_retired":true,
        "scope":"actual native owner requests and physical release; allocator trigger and Python/GIL integration are separate"}),
    )
}

pub(super) fn arena_concurrency(peer: &cooperative::Peer, device: i32) -> Result<Value> {
    use std::sync::{Arc, Barrier};
    struct Creating(Arc<AtomicBool>);
    impl Drop for Creating {
        fn drop(&mut self) {
            self.0.store(false, Ordering::Release);
        }
    }
    let anchor = varena::Arena::new(device)?; // Every recovery traverses a real live Arena.
    let start = Arc::new(Barrier::new(4));
    let creating = Arc::new(AtomicBool::new(true));
    let result = std::thread::scope(|scope| -> std::result::Result<Value, String> {
        let create_start = Arc::clone(&start);
        let create_active = Arc::clone(&creating);
        let creators = scope.spawn(move || -> std::result::Result<usize, String> {
            let _creating = Creating(create_active); // errors also end the observation window
            let cuda = Cuda::open(device).map_err(|error| error.to_string());
            create_start.wait();
            let cuda = cuda?;
            for _ in 0..128 {
                let arena = varena::Arena::new(device).map_err(|error| error.to_string())?;
                let (region, pin) = filled(&arena, 2 << 20, 173).map_err(|error| error.to_string())?;
                read(&cuda, pin.ptr(), pin.len(), 173).map_err(|error| error.to_string())?;
                // Full readback proves every consumer completed before this exact pin ends.
                let done = unsafe { pin.into_completed_region() };
                drop((done, region, arena));
            }
            Ok(128)
        });
        let reclaim_start = Arc::clone(&start);
        let reclaim_active = Arc::clone(&creating);
        let reclaimers = scope.spawn(move || -> std::result::Result<(usize, usize), String> {
            reclaim_start.wait();
            let (mut passes, mut overlapping) = (0, 0);
            while passes < 512 || reclaim_active.load(Ordering::Acquire) {
                overlapping += usize::from(reclaim_active.load(Ordering::Acquire));
                varena::reclaim(device, 2 << 20).map_err(|error| error.to_string())?;
                varena::resident_bytes(device).map_err(|error| error.to_string())?;
                passes += 1;
                std::thread::yield_now();
            }
            Ok((passes, overlapping))
        });
        let publish_start = Arc::clone(&start);
        let publish_active = Arc::clone(&creating);
        let publishers = scope.spawn(move || -> std::result::Result<(usize, usize, usize), String> {
            publish_start.wait();
            let (mut refreshed, mut deferred, mut overlapping) = (0, 0, 0);
            while refreshed + deferred < 512 || publish_active.load(Ordering::Acquire) {
                overlapping += usize::from(publish_active.load(Ordering::Acquire));
                match peer.publish(0) {
                    Ok(_) => refreshed += 1,
                    Err(cooperative::Error::Native(varena::Error::Busy | varena::Error::CaptureActive)
                        | cooperative::Error::Ledger(ledger::Error::Busy | ledger::Error::Stale | ledger::Error::StaleSample)) => deferred += 1,
                    Err(error) => return Err(error.to_string()),
                }
                std::thread::yield_now();
            }
            Ok((refreshed, deferred, overlapping))
        });
        start.wait();
        let made = creators.join().map_err(|_| "arena creator panicked")??;
        let (reclaimed, reclaim_overlap) = reclaimers.join().map_err(|_| "arena reclaimer panicked")??;
        let (published, deferred, publish_overlap) = publishers.join().map_err(|_| "arena publisher panicked")??;
        if reclaim_overlap == 0 || publish_overlap == 0 { return Err("no actual concurrent creation observation".into()); }
        Ok(json!({"actual_creation_fill_readback_drop_passes":made,
            "actual_reclaim_and_resident_snapshot_passes":reclaimed,"reclaim_passes_while_creating":reclaim_overlap,
            "actual_peer_publications":published,"typed_deferred_publications":deferred,
            "publication_passes_while_creating":publish_overlap,
            "simultaneous_start_barrier":true,"production_probe_hooks":false}))
    }).map_err(io::Error::other)?;
    varena::reclaim(device, usize::MAX)?;
    require(
        varena::resident_bytes(device)? == 0,
        "concurrent native arenas retained physical backing",
    )?;
    require(
        accounting(peer)?["physical"] == 0,
        "concurrent arena final physical accounting",
    )?;
    drop(anchor);
    Ok(result)
}
