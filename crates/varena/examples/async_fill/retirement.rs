//! Real producer/consumer lifetime composition with native Arena retirement.
use super::*;

fn arena_facts(ticket: &varena::ArenaRetirement) -> Result<Value> {
    let facts = ticket.facts()?;
    Ok(json!({"arena_id":facts.arena_id,"device":facts.device,
        "state":format!("{:?}",facts.state),"observers_detached":facts.observers_detached,
        "remaining_physical_bytes":facts.remaining_physical_bytes,"failure":facts.failure}))
}

fn request_retirement(arena: Arc<Arena>) -> Result<varena::ArenaRetirement> {
    match arena.request_retirement() {
        Ok(ticket) => Ok(ticket),
        Err((owner, error)) => {
            // This isolated fixture is failing. Preserve the returned owner rather than
            // silently releasing it while reporting a refused ownership transfer.
            std::mem::forget(owner);
            Err(error.into())
        }
    }
}

fn useful_work(cuda: &Cuda, region: &Arc<varena::Region>, stream: u64, output: u64) -> Result<()> {
    let pin = region.pin()?;
    // SAFETY: the ready Region and output own complete BYTES ranges in this context.
    unsafe {
        check(
            "independent arena D2D",
            (cuda.d2d)(output, pin.ptr(), BYTES, stream as Obj),
        )?;
        drop(pin.into_fenced_region(&[stream]));
    }
    cuda.sync(stream)?;
    cuda.verify(output)
}

pub(super) fn run(cuda: Arc<Cuda>, device: i32) -> Result<Value> {
    let live = Arena::new(device)?;
    let live_region = live.reserve(BYTES)?;
    let independent = cuda.stream()?;
    let ready = fill(&live_region)?;
    cuda.initialize_pattern(ready.ptr(), independent)?;
    // SAFETY: the owned independent stream completed both whole-region writes.
    unsafe {
        drop(ready.commit_completed()?.into_completed_region());
    }

    let retiring = Arena::new(device)?;
    let weak = Arc::downgrade(&retiring);
    let region = retiring.reserve(BYTES)?;
    let physical = region.allocated_bytes();
    let resources = Resources::new(Arc::clone(&cuda))?;
    let dropped = Arc::clone(&resources.dropped);
    let errors = Arc::clone(&resources.errors);
    let control = cuda.stream()?;
    let consumer = cuda.stream()?;
    let producer_gate = working::alloc(device, 4, control, None)?;
    let consumer_gate = working::alloc(device, 4, control, None)?;
    let output = working::alloc(device, BYTES, consumer, None)?;
    let independent_output = working::alloc(device, BYTES, independent, None)?;
    cuda.initialize_gate(producer_gate, control)?;
    cuda.initialize_gate(consumer_gate, control)?;
    let released_before = varena::released_bytes(device)?;
    let mut session = owned(fill(&region)?, &resources);
    let destination = session.begin()?;
    submit(&cuda, &resources, destination.ptr, producer_gate, false)?;
    let fill_ticket = unsafe { session.publish()? };
    drop(session);
    drop(resources);
    let pin = match unsafe { region.prepare_on(consumer)? } {
        StreamPreparation::Ordered(pin) => pin,
        _ => return Err("retirement expected an in-flight ordered Pin".into()),
    };
    unsafe {
        check(
            "consumer lifetime gate",
            (cuda.wait)(consumer as Obj, consumer_gate, 1, 1),
        )?;
        check(
            "retiring arena D2D",
            (cuda.d2d)(output, pin.ptr(), BYTES, consumer as Obj),
        )?;
    }
    let ticket = request_retirement(retiring)?;
    until(|| Ok(ticket.facts()?.observers_detached))?;
    require(
        !ticket.try_complete()?,
        "retirement discarded a live Fill/consumer owner",
    )?;
    require(
        ticket.facts()?.remaining_physical_bytes == physical,
        "pending producer backing vanished",
    )?;
    require(
        !fill_ticket.facts()?.completed,
        "producer gate did not hold the Fill",
    )?;
    let while_producing = arena_facts(&ticket)?;
    useful_work(&cuda, &live_region, independent, independent_output)?;

    release_gate(&cuda, producer_gate, control)?;
    until(|| Ok(fill_ticket.facts()?.retirement_finished))?;
    require(
        dropped.load(Ordering::SeqCst) == 1 && errors.load(Ordering::SeqCst) == 0,
        "checked Fill resources did not retire exactly once",
    )?;
    require(
        cuda.pending(consumer)?,
        "consumer gate did not retain independent work",
    )?;
    require(
        !ticket.try_complete()? && ticket.facts()?.remaining_physical_bytes == physical,
        "producer completion reclaimed a live consumer",
    )?;
    let after_producer = arena_facts(&ticket)?;
    // SAFETY: this is the last submission on the consumer. The fence is after both gates
    // and the full D2D copy; the stream remains live through final Arena completion.
    unsafe {
        drop(pin.into_fenced_region(&[consumer]));
    }
    drop(region);
    // Observation only: native retirement makes the decision; this fixture never forces
    // Arc ownership or uses its count to authorize physical teardown.
    until(|| Ok(weak.strong_count() == 0))?;
    require(
        !ticket.try_complete()? && ticket.facts()?.remaining_physical_bytes == physical,
        "final Arena handoff treated a pending consumer fence as complete",
    )?;
    let finalizing = arena_facts(&ticket)?;
    useful_work(&cuda, &live_region, independent, independent_output)?;
    require(
        varena::released_bytes(device)? == released_before,
        "finalizing metadata credited bytes before consumer completion",
    )?;

    release_gate(&cuda, consumer_gate, control)?;
    cuda.sync(consumer)?;
    cuda.verify(output)?;
    until(|| ticket.try_complete().map_err(Into::into))?;
    require(
        ticket.facts()?.remaining_physical_bytes == 0,
        "completed Arena still has physical bytes",
    )?;
    require(
        varena::released_bytes(device)?.checked_sub(released_before) == Some(physical as u64),
        "Arena retirement did not credit the exact physical release",
    )?;
    require(
        live.stats()?.context_joins == 0,
        "independent Arena required a context join",
    )?;
    let completed = arena_facts(&ticket)?;
    let live_after = stats(&live)?;
    useful_work(&cuda, &live_region, independent, independent_output)?;
    finish_working(
        &cuda,
        device,
        &[producer_gate, consumer_gate, output, independent_output],
        &[control, consumer, independent],
    )?;
    drop(live_region);
    let live_ticket = request_retirement(live)?;
    until(|| live_ticket.try_complete().map_err(Into::into))?;
    Ok(
        json!({"while_producing":while_producing,"after_producer":after_producer,
        "finalizing":finalizing,"completed":completed,"fill":facts(fill_ticket.facts()?),
        "live_arena_after":live_after,"independent_full_readbacks":3,
        "ordered_consumer_full_readback":true,"exact_physical_release":physical,
        "live_arena_final":arena_facts(&live_ticket)?,
        "scope":"two Arenas on one GPU; no external peer service or multi-GPU claim"}),
    )
}
