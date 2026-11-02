//! Genuine same-device multi-arena Plan proof; never substitutes CUDA or allocator state.
use super::*;
use std::sync::Arc;
use varena::{Arena, Preparation, Region, plan::Plan};

fn fill(region: &Arc<Region>, byte: u8) -> Result<varena::Pin> {
    let mut fill = match region.prepare()? {
        Preparation::Fill(fill) => fill,
        _ => return Err(io::Error::other("expected cold real reservation").into()),
    };
    fill.copy_from_host(&vec![byte; region.len()])?;
    Ok(fill.commit()?)
}

fn selected(device: i32, page: usize, key: &str) -> Result<working::SplitSelection> {
    Ok(working::split_select(
        device,
        key,
        vec![("one".into(), vec![page])],
        "one",
        0,
    )?)
}

fn committed(device: i32, page: usize) -> Result<working::Execution> {
    Ok(working::Execution::commit(
        device,
        "multi-arena/u8/closed",
        "multi-arena-current",
        vec![("one".into(), vec![page])],
        "one",
        0,
    )?)
}

fn refuse_stale(execution: &working::Execution, reason: &str) -> Result<()> {
    require(
        matches!(execution.enter(), Err(varena::Error::Invalid(_))),
        reason,
    )
}

pub(super) fn same_device(cuda: &Cuda, peer: &cooperative::Peer, device: i32) -> Result<Value> {
    let a = Arena::new(device)?;
    let b = Arena::new(device)?;
    let outside = Arena::new(device)?;
    let page = a.granularity();
    let r1 = a.reserve(page)?;
    let r2 = b.reserve(2 * page)?;
    let cached = outside.reserve(page)?;
    let ids = [a.id(), b.id(), outside.id(), r1.id(), r2.id(), cached.id()];
    require(
        ids.iter()
            .copied()
            .collect::<std::collections::BTreeSet<_>>()
            .len()
            == ids.len(),
        "Arena and Region lifetime identities collided",
    )?;
    require(
        r1.arena_id() == a.id() && r2.arena_id() == b.id(),
        "Region owning arena identity",
    )?;
    let pin1 = fill(&r1, 43)?;
    let cached_pin = fill(&cached, 79)?;
    cuda.synchronize()?;
    drop(unsafe { cached_pin.into_completed_region() });

    let plan = Plan::new(
        a.clone(),
        vec![("both".into(), vec![r1.clone(), r2.clone(), r1.clone()])],
    )?;
    let facts = plan.facts();
    require(
        facts[0].regions == 2 && facts[0].bytes == 3 * page,
        "cross-arena duplicate counted twice",
    )?;
    plan.activate("both")?;
    let other = Plan::new(b.clone(), vec![("other".into(), vec![r2.clone()])])?;
    require(
        matches!(other.activate("other"), Err(varena::Error::Invalid(_))),
        "two active device plans admitted",
    )?;
    drop(other);
    let cold = selected(device, page, "multi-arena-cold")?;
    require(
        cold.phase_weight_bytes == 3 * page
            && cold.phase_missing_bytes == 2 * page
            && cold.phase_outside_reclaimable_bytes == page,
        "device-wide physical intent snapshot",
    )?;

    let pin2 = fill(&r2, 59)?;
    // Three pages of pinned phase weights plus one completed outside page fill the cap.
    // The chosen working page fits only after actual native outside-phase reclamation.
    varena::set_limit(device, Some(4 * page))?;
    let choice = selected(device, page, "multi-arena-scratch")?;
    require(
        choice.preserves_phase_weights
            && choice.phase_missing_bytes == 0
            && choice.phase_outside_reclaimable_bytes == page,
        "completed outside-phase capacity was not recognized",
    )?;
    let phase = working::phase_enter(
        device,
        Some("multi-arena-scratch".into()),
        working::PhaseMode::Eager,
    )?;
    let leaf = working::split_begin(
        device,
        "multi-arena-scratch",
        "one",
        Some(phase),
        0,
        Some(choice.plan_generation),
    )?;
    require(
        !cached.is_resident()? && r1.is_resident()? && r2.is_resident()?,
        "outside reclaim changed a phase weight",
    )?;
    let output = working::alloc(device, page, 0, None)?;
    cuda.fill(output, page, 97)?;
    cuda.verify(&Block {
        ptr: output,
        bytes: page,
        value: 97,
    })?;
    cuda.verify(&Block {
        ptr: r1.ptr(),
        bytes: page,
        value: 43,
    })?;
    cuda.verify(&Block {
        ptr: r2.ptr(),
        bytes: 2 * page,
        value: 59,
    })?;
    unsafe {
        working::free_completed(device, output)?;
    }
    working::split_end(device, leaf, true)?;
    let observed = working::phase_leave(device, phase, true, None)?;
    require(
        observed.published && observed.peak_bytes == page,
        "actual chosen working demand was not measured",
    )?;
    working::phase_forget(device, "multi-arena-scratch")?;
    working::trim(device)?;
    varena::set_limit(device, None)?;

    let mut stale = Vec::new();
    let execution = committed(device, page)?;
    let key = execution.key()?.to_owned();
    let old = varena::plan::inventory_generation(device)?;
    let temporary = b.reserve(page)?;
    require(
        varena::plan::inventory_generation(device)? > old,
        "Region publication did not advance inventory",
    )?;
    refuse_stale(
        &execution,
        "committed entry accepted a newly published region",
    )?;
    stale.push("region_publication");
    let first_id = temporary.id();
    let execution = committed(device, page)?;
    let old = varena::plan::inventory_generation(device)?;
    drop(temporary);
    require(
        varena::plan::inventory_generation(device)? > old,
        "Region retirement did not advance inventory",
    )?;
    refuse_stale(
        &execution,
        "committed entry accepted retired region inventory",
    )?;
    stale.push("region_retirement");
    let replacement = b.reserve(page)?;
    require(
        replacement.id() != first_id,
        "retired Region identity was reused",
    )?;
    let compatible = committed(device, page)?;
    require(
        compatible.key()? == key,
        "lifetime identity changed compatible compute fingerprint",
    )?;
    compatible.abort()?;
    drop(replacement);

    let execution = committed(device, page)?;
    let temporary_arena = Arena::new(device)?;
    refuse_stale(&execution, "committed entry accepted new arena inventory")?;
    stale.push("arena_publication");
    let execution = committed(device, page)?;
    let old_arena_id = temporary_arena.id();
    let before_retirement = varena::plan::inventory_generation(device)?;
    let retired = super::arena_retirement::request(temporary_arena)?;
    wait_retired(&retired)?;
    require(
        varena::plan::inventory_generation(device)? > before_retirement,
        "Arena retirement did not advance inventory",
    )?;
    refuse_stale(
        &execution,
        "committed entry accepted retired arena inventory",
    )?;
    stale.push("arena_retirement");

    // Safe Arc::try_unwrap creates the actual last-Arc/destructor gap deterministically.
    // No Peer publication has retained this Arena and no fake weak owner is inserted.
    let retiring = Arena::new(device)?;
    require(
        retiring.id() != old_arena_id,
        "retired Arena identity was reused",
    )?;
    let retiring_id = retiring.id();
    let unwrapped = Arc::try_unwrap(retiring)
        .map_err(|_| io::Error::other("unexpected Arena lifetime owner"))?;
    require(
        matches!(plan.activate("both"), Err(varena::Error::Busy)),
        "partial retiring inventory was accepted",
    )?;
    let before_drop = varena::plan::inventory_generation(device)?;
    drop(unwrapped);
    require(
        varena::plan::inventory_generation(device)? > before_drop,
        "unwrapped Arena retirement was lost",
    )?;
    plan.activate("both")?;

    // A copied Plan must retain every referenced arena, including one other than anchor.
    let held_a = Arena::new(device)?;
    let held_b = Arena::new(device)?;
    let held_region = held_b.reserve(page)?;
    let weak_a = Arc::downgrade(&held_a);
    let weak_b = Arc::downgrade(&held_b);
    let weak_region = Arc::downgrade(&held_region);
    let held_plan = Plan::new(
        held_a.clone(),
        vec![("held".into(), vec![held_region.clone()])],
    )?;
    let retire_a = super::arena_retirement::request(held_a)?;
    let retire_b = super::arena_retirement::request(held_b)?;
    drop(held_region);
    for ticket in [&retire_a, &retire_b] {
        let end = Instant::now() + Duration::from_secs(5);
        while !ticket.facts()?.observers_detached {
            require(Instant::now() < end, "observer detach did not settle")?;
            std::thread::sleep(Duration::from_millis(1));
        }
    }
    require(
        weak_a.upgrade().is_some() && weak_b.upgrade().is_some() && weak_region.upgrade().is_some(),
        "Plan did not retain all reservation owners",
    )?;
    drop(held_plan);
    wait_retired(&retire_a)?;
    wait_retired(&retire_b)?;
    require(
        weak_a.upgrade().is_none() && weak_b.upgrade().is_none() && weak_region.upgrade().is_none(),
        "Plan lifetime owners did not retire",
    )?;

    let concurrent = concurrent_inventory(device, page)?;
    let refill = fill(&cached, 79)?;
    cuda.verify(&Block {
        ptr: cached.ptr(),
        bytes: page,
        value: 79,
    })?;
    cuda.synchronize()?;
    drop(unsafe { refill.into_completed_region() });

    // Only now retain arenas for instrumentation; earlier lifetime cases require real
    // final owners, not a hidden observing Peer keeping their arenas alive.
    peer.publish(0)?;
    let capture = capture_refusal(cuda, peer, &plan, device, page)?;
    plan.deactivate()?;
    drop(plan);
    cuda.synchronize()?;
    drop(unsafe { pin1.into_completed_region() });
    drop(unsafe { pin2.into_completed_region() });
    for arena in [&a, &b, &outside] {
        arena.reclaim(usize::MAX)?;
        require(
            arena.stats()?.resident_bytes == 0,
            "multi-arena physical cleanup",
        )?;
    }
    drop(r1);
    drop(r2);
    drop(cached);
    let retired = [
        super::arena_retirement::request(a)?,
        super::arena_retirement::request(b)?,
        super::arena_retirement::request(outside)?,
    ];
    for ticket in &retired {
        wait_retired(ticket)?;
    }
    Ok(
        json!({"page":page,"identities":ids,"deduplicated_regions":facts[0].regions,
        "phase_bytes":facts[0].bytes,"cold_missing_bytes":cold.phase_missing_bytes,
        "outside_reclaimed_bytes":page,"full_readback_bytes":5*page,"outside_refill_readback":true,"concurrent_inventory":concurrent,
        "stale_entry_refused":stale,"retiring_weak_refused":retiring_id,
        "plan_retained_all_arenas":true,"compatible_compute_key":true,
        "capture":capture,"physical_baseline_restored":true,
        "mismatched_device_case":"separate two-GPU entrypoint required"}),
    )
}

fn wait_retired(ticket: &varena::ArenaRetirement) -> Result<()> {
    let end = Instant::now() + Duration::from_secs(5);
    while !ticket.try_complete()? {
        require(
            Instant::now() < end,
            "actual native final resources did not retire",
        )?;
        std::thread::sleep(Duration::from_millis(1));
    }
    Ok(())
}

fn concurrent_inventory(device: i32, page: usize) -> Result<Value> {
    use std::sync::atomic::{AtomicBool, Ordering};
    let before = varena::plan::inventory_generation(device)?;
    let done = Arc::new(AtomicBool::new(false));
    let finished = done.clone();
    let worker = std::thread::spawn(move || {
        let result = (|| -> varena::Result<()> {
            for _ in 0..32 {
                let arena = Arena::new(device)?;
                let region = arena.reserve(page)?;
                drop(region);
                let ticket = arena.request_retirement().map_err(|(arena, error)| {
                    std::mem::forget(arena);
                    error
                })?;
                let end = Instant::now() + Duration::from_secs(5);
                while !ticket.try_complete()? {
                    if Instant::now() >= end {
                        return Err(varena::Error::Invalid(
                            "concurrent retirement did not settle",
                        ));
                    }
                    std::thread::sleep(Duration::from_millis(1));
                }
            }
            Ok(())
        })()
        .map_err(|error| error.to_string());
        finished.store(true, Ordering::Release);
        result
    });
    let deadline = Instant::now() + Duration::from_secs(15);
    let (mut accepted, mut deferred, mut tries) = (0, 0, 0);
    while !done.load(Ordering::Acquire) || tries < 64 {
        require(
            Instant::now() < deadline,
            "concurrent real arena lifecycle did not settle",
        )?;
        tries += 1;
        match selected(device, page, "multi-arena-concurrent") {
            Ok(choice) => {
                require(
                    choice.phase_weight_bytes == 3 * page && choice.phase_missing_bytes == 0,
                    "concurrent inventory changed the retained phase membership",
                )?;
                accepted += 1;
            }
            Err(error) => {
                let text = error.to_string();
                require(
                    text.contains("already being filled")
                        || text.contains("inventory changed")
                        || text.contains("intent or inventory changed"),
                    "unrelated concurrent Plan failure",
                )?;
                deferred += 1;
            }
        }
        std::thread::yield_now();
    }
    worker
        .join()
        .map_err(|_| io::Error::other("arena lifecycle thread panicked"))?
        .map_err(io::Error::other)?;
    let delta = varena::plan::inventory_generation(device)? - before;
    require(
        delta == 128,
        "actual Arena/Region publication and retirement epochs were lost",
    )?;
    require(
        selected(device, page, "multi-arena-concurrent")?.phase_weight_bytes == 3 * page,
        "stable selection did not recover after concurrent retirement",
    )?;
    Ok(
        json!({"actual_arenas":32,"actual_regions":32,"inventory_delta":delta,
        "accepted_snapshots":accepted,"typed_deferrals":deferred,"stable_after_join":true}),
    )
}

fn capture_refusal(
    cuda: &Cuda,
    peer: &cooperative::Peer,
    plan: &Arc<Plan>,
    device: i32,
    page: usize,
) -> Result<Value> {
    type Create = unsafe extern "C" fn(*mut Ctx, u32) -> i32;
    type Begin = unsafe extern "C" fn(Ctx, i32) -> i32;
    type End = unsafe extern "C" fn(Ctx, *mut Ctx) -> i32;
    type Destroy = unsafe extern "C" fn(Ctx) -> i32;
    let (create, begin, end, destroy_stream, destroy_graph) = unsafe {
        (
            *cuda._library.get::<Create>(b"cuStreamCreate\0")?,
            *cuda._library.get::<Begin>(b"cuStreamBeginCapture_v2\0")?,
            *cuda._library.get::<End>(b"cuStreamEndCapture\0")?,
            *cuda._library.get::<Destroy>(b"cuStreamDestroy_v2\0")?,
            *cuda._library.get::<Destroy>(b"cuGraphDestroy\0")?,
        )
    };
    let mut stream = std::ptr::null_mut();
    check("cuStreamCreate", unsafe { create(&mut stream, 1) })?;
    let guard = varena::enter_capture(device)?;
    check("cuStreamBeginCapture", unsafe { begin(stream, 0) })?;
    let observed = (|| -> Result<Value> {
        let before = peer.cuda_calls()?;
        require(
            matches!(plan.activate("both"), Err(varena::Error::CaptureActive)),
            "Plan activated during actual capture",
        )?;
        require(
            selected(device, page, "multi-arena-capture").is_err(),
            "selection entered actual capture",
        )?;
        require(
            matches!(Arena::new(device), Err(varena::Error::CaptureActive)),
            "Arena published during actual capture",
        )?;
        let after = peer.cuda_calls()?;
        require(before == after, "capture refusal called the native driver")?;
        Ok(json!({"actual_cuda_capture":true,"driver_before":before,"driver_after":after}))
    })();
    let mut graph = std::ptr::null_mut();
    let ended = unsafe { end(stream, &mut graph) };
    if ended != 0 {
        let _ = emit(&json!({"event":"capture_end_failed","cuda_status":ended,
            "boundary":"process exits without GPU-owning Rust teardown"}));
        std::process::exit(3);
    }
    drop(guard);
    if !graph.is_null() {
        check("cuGraphDestroy", unsafe { destroy_graph(graph) })?;
    }
    check("cuStreamDestroy", unsafe { destroy_stream(stream) })?;
    observed
}

pub(super) fn mismatched_device(device: i32) -> Result<Value> {
    let other = if device == 0 { 1 } else { 0 };
    let anchor = Arena::new(device)?;
    let before = varena::plan::inventory_generation(device)?;
    let foreign = Arena::new(other)?; // Requires the separately declared second actual GPU.
    let region = foreign.reserve(foreign.granularity())?;
    require(
        varena::plan::inventory_generation(device)? == before,
        "other-device publication changed anchor inventory",
    )?;
    let ids = [anchor.id(), foreign.id(), region.id()];
    require(
        ids.iter()
            .copied()
            .collect::<std::collections::BTreeSet<_>>()
            .len()
            == 3,
        "cross-device lifetime identity collision",
    )?;
    require(
        matches!(
            Plan::new(anchor, vec![("invalid".into(), vec![region.clone()])]),
            Err(varena::Error::Invalid(_))
        ),
        "foreign-device region admitted into Plan",
    )?;
    drop(region);
    drop(foreign);
    require(
        varena::resident_bytes(other)? == 0,
        "foreign-device physical cleanup",
    )?;
    Ok(
        json!({"anchor_device":device,"foreign_device":other,"identities":ids,
        "foreign_region_rejected":true,"device_inventory_isolated":true}),
    )
}
