use super::*;

pub(super) fn active_cancel(cuda: &Cuda, root: &Path, device: i32) -> Result<Value> {
    let recipe = checkpoint(root)?;
    let arena = Arena::new(device)?;
    let length = 6 * PAYLOAD;
    let region = arena.reserve(length)?;
    // Three separate file pieces, one reader, two staging slots. The third piece waits
    // on real prior DMA while the main fixture thread requests cancellation and opens it.
    let items = (0..3)
        .map(|index| Item {
            offset: (index * 2 * PAYLOAD) as u64,
            len: PAYLOAD as u64,
            source: recipe.items[0].source.clone(),
            part_end: true,
        })
        .collect();
    let input = fill(&region)?;
    let alias = input.pin()?;
    let mut read = prepare(&recipe.source, input, items, 1)?;
    let producer_streams = read.facts()?.producer_streams;
    let control_stream = cuda.stream()?;
    let gate = working::alloc(device, 4, control_stream, None)?;
    cuda.initialize_gate(control_stream, gate)?;
    unsafe {
        for &stream in &producer_streams {
            check("gate active Source", (cuda.wait)(stream as Obj, gate, 1, 1))?;
        }
    }
    let control = ReadControl::default();
    let threshold = (5 * PAYLOAD) as u64; // Three padding pieces plus two real file copies.
    let (submitted, before_cancel) = std::thread::scope(|scope| -> Result<_> {
        let runner = scope.spawn(|| read.submit(&control));
        let progress = until(|| Ok(control.issued_bytes() >= threshold));
        let before_cancel = control.issued_bytes();
        control.cancel();
        // Also release on a protocol-refusal path, so scope teardown can join its real
        // submitter. No completion or publication is inferred from opening this gate.
        cuda.open_gate(control_stream, gate)?;
        let submitted = runner
            .join()
            .map_err(|_| "actual Source submitter panicked")?;
        progress?;
        Ok((submitted, before_cancel))
    })?;
    require(
        submitted.is_err(),
        "mid-submission cancellation returned success",
    )?;
    until(|| Ok(read.poll_complete()?.is_some()))?;
    require(
        read.facts()?.published_regions == 0 && !region.is_resident()?,
        "canceled read published contents",
    )?;
    require(
        cuda.bytes(alias.ptr(), PAYLOAD)? == recipe.payload
            && cuda.bytes(alias.ptr() + (2 * PAYLOAD) as u64, PAYLOAD)? == recipe.payload,
        "the two proven pre-cancel file copies changed",
    )?;
    require(
        matches!(region.prepare(), Err(varena::Error::Busy)),
        "cancellation overwrote an escaped alias",
    )?;
    let canceled = observed(&read)?;
    unsafe {
        drop(alias.into_completed_region());
    }
    drop(read);
    until(|| Ok(arena.stats()?.pinned_bytes == 0))?;
    arena.reclaim(length)?;
    require(
        arena.stats()?.resident_bytes == 0 && arena.stats()?.context_joins == 0,
        "active cancellation kept backing or joined context",
    )?;
    let final_stats = arena_stats(&arena)?;
    finish(cuda, device, &[gate], &[control_stream])?;
    drop(region);
    retire(arena)?;
    Ok(
        json!({"before_cancel_issued_bytes":before_cancel,"threshold":threshold,
        "actual_mid_submission_cancel":true,"first_two_file_copies_exact":true,
        "canceled":canceled,"final":final_stats}),
    )
}

pub(super) fn multi_fill(cuda: &Cuda, root: &Path, device: i32) -> Result<Value> {
    let recipe = checkpoint(root)?;
    let a = Arena::new(device)?;
    let b = Arena::new(device)?;
    let first = a.reserve(LENGTH)?;
    let second = b.reserve(LENGTH)?;
    let inputs = vec![
        RawFill {
            fill: fill(&first)?,
            items: recipe.items.clone(),
            origin: 0,
        },
        RawFill {
            fill: fill(&second)?,
            items: recipe.items,
            origin: 0,
        },
    ];
    let mut read = match unsafe { recipe.source.prepare_raw(inputs, 2, Mode::Buffered) } {
        Ok(read) => read,
        Err((owners, error)) => {
            std::mem::forget(owners);
            return Err(error.into());
        }
    };
    drop(recipe.checkpoint);
    drop(recipe.source);
    read.submit(&ReadControl::default())?;
    let stream = cuda.stream()?;
    let out = working::alloc(device, LENGTH, stream, None)?;
    for region in [&first, &second] {
        let pin = match unsafe { region.prepare_on(stream)? } {
            StreamPreparation::Ordered(pin) | StreamPreparation::Ready(pin) => pin,
            _ => return Err("shared Source returned an uninitialized Fill".into()),
        };
        unsafe {
            check(
                "multi-Fill consumer",
                (cuda.copy)(out, pin.ptr(), LENGTH, stream as Obj),
            )?;
        }
        cuda.sync(stream)?;
        require(
            cuda.bytes(out, LENGTH)? == recipe.expected,
            "multi-Fill full readback differs",
        )?;
        unsafe {
            drop(pin.into_completed_region());
        }
    }
    until(|| {
        Ok(read.poll_complete()?.is_some()
            && read.facts()?.resources_retired
            && read.facts()?.submitted_fills_retired)
    })?;
    let facts = read.facts()?;
    require(
        facts.published_regions == 2
            && facts.requested_bytes == 2 * (PAYLOAD + 17) as u64
            && facts.issued_bytes == 2 * LENGTH as u64,
        "shared Source counters differ",
    )?;
    // Both actual Fill retirement payloads must have released producer exclusion; this is
    // the current PreparedRead success/capture boundary, not just data completion.
    let guard = varena::enter_capture(device)?;
    let mut graph = std::ptr::null_mut();
    unsafe {
        check(
            "capture after multi-Fill retirement",
            (cuda.capture_begin)(stream as Obj, 0),
        )?;
        check(
            "end post-retirement capture",
            (cuda.capture_end)(stream as Obj, &mut graph),
        )?;
    }
    drop(guard);
    unsafe {
        check("destroy post-retirement graph", (cuda.graph_destroy)(graph))?;
    }
    let observed = observed(&read)?;
    drop(read);
    a.reclaim(LENGTH)?;
    b.reclaim(LENGTH)?;
    require(
        a.stats()?.resident_bytes == 0
            && b.stats()?.resident_bytes == 0
            && a.stats()?.context_joins == 0
            && b.stats()?.context_joins == 0,
        "shared Source final accounting differs",
    )?;
    finish(cuda, device, &[out], &[stream])?;
    drop(first);
    drop(second);
    retire(a)?;
    retire(b)?;
    Ok(json!({"read":observed,"two_arenas_full_readback":true,
        "actual_capture_after_all_fill_retirements":true,"final_physical_bytes":0}))
}
