//! First native adopter: raw Source only, with no Python/GIL owners or continuation.
use super::*;

pub(super) fn eligible(work: &Work) -> bool {
    work.decode.is_none()
        && work.prepared_decode.is_none()
        && work.continuation.is_none()
        && work.host.is_none()
        && !work.event_ordered
}

fn mode(work: &Work) -> Result<Mode, Failure> {
    let streamed = work
        .inputs
        .iter()
        .filter(|input| input.fill.is_some())
        .try_fold(false, |streamed, input| {
            input
                .region
                .is_preferred()
                .map(|preferred| streamed || !preferred)
        })
        .map_err(Failure::Native)?;
    Ok(if streamed {
        Mode::Buffered
    } else {
        Mode::Cache
    })
}

/// No raw producer ever used these still-owned Fills or ready Pins. Escaped views retain
/// independent Pins. The new path never falls through to the older engine-drain cleanup.
fn release_unsubmitted(work: &mut Work) {
    let mut aborted = Vec::new();
    for input in &mut work.inputs {
        if let Some(fill) = input.fill.take() {
            // SAFETY: raw submission has not transferred this Fill or issued a writer.
            drop(unsafe { fill.abort_completed() });
            aborted.push(Arc::clone(&input.region));
        }
        if let Some(pin) = input.pin.take() {
            // SAFETY: this raw-only Work has never submitted a consumer of ready input.
            drop(unsafe { pin.into_completed_region() });
        }
    }
    for region in aborted {
        Unready::release(region);
    }
}

pub(super) fn run(shared: Arc<Shared>, mut work: Work) {
    work.completed = false;
    let mut operation: Option<io::source::RawRead> = None;
    let outcome = catch_unwind(AssertUnwindSafe(|| {
        if shared.cancel.canceled() {
            return Err(Failure::Canceled);
        }
        let mode = mode(&work)?;
        let fills = work
            .inputs
            .iter_mut()
            .filter_map(|input| {
                input.fill.take().map(|fill| io::source::RawFill {
                    fill,
                    items: std::mem::take(&mut input.items),
                    origin: 0,
                })
            })
            .collect::<Vec<_>>();
        if fills.is_empty() {
            work.completed = true;
            shared
                .read_completed_ns
                .store(shared.elapsed(), Ordering::Release);
            return Ok(ReadStats::default());
        }
        // SAFETY: these Fills are this Work's exclusive prepared destinations. All raw
        // sources are owned native checkpoint/items; no earlier writer or Python hook exists.
        let prepared = unsafe { work.source.prepare_raw(fills, work.threads, mode) };
        operation = Some(match prepared {
            Ok(prepared) => prepared,
            Err((fills, error)) => {
                // Refusal returns the actual untouched owners; no destination was exposed.
                for input in fills {
                    // No producer was exposed. Keep the existing failed-fill immediate
                    // release behavior; escaped aliases still prevent physical reclaim.
                    Unready::release(unsafe { input.fill.abort_completed() });
                }
                return Err(io_failure(error));
            }
        });
        let operation = operation
            .as_mut()
            .expect("raw operation installed before submission");
        let submitted = operation.submit(&shared.cancel).map_err(io_failure);
        let facts = operation.facts().map_err(io_failure)?;
        if facts.submission_closed && facts.issued_bytes != 0 {
            shared
                .read_issued_ns
                .store(shared.elapsed(), Ordering::Release);
        }
        let stats = loop {
            if let Some(stats) = operation.poll_complete().map_err(io_failure)? {
                // A successful current PreparedRead can be followed immediately by capture.
                // Native ProducerGuards end after submitted Fill retirement, not merely
                // producer event completion. Failed requests retain native cleanup custody.
                let facts = operation.facts().map_err(io_failure)?;
                if submitted.is_err() || (facts.resources_retired && facts.submitted_fills_retired)
                {
                    break stats;
                }
            }
            // Only host metadata/event polling runs here; no stream/context join, callback
            // or Python interpreter participates in this native source worker.
            std::thread::sleep(std::time::Duration::from_millis(1));
        };
        work.completed = true;
        // Checked drain of a failed/partial source is not a completed logical read.
        // Preserve the existing read-byte/clock contract while separately certifying custody.
        submitted?;
        shared
            .read_completed_ns
            .store(shared.elapsed(), Ordering::Release);
        shared.read_bytes.store(stats.bytes, Ordering::Release);
        if shared.cancel.canceled() {
            return Err(Failure::Canceled);
        }
        Ok(stats)
    }));
    let result = outcome.unwrap_or_else(|_| {
        Err(Failure::Native(varena::Error::Poisoned(
            "native raw Source worker panicked".into(),
        )))
    });
    // An operation owns every transferred Fill; its native payload retains actual Source,
    // Checkpoint, engine and staging on any unproven completion. Never reclaim it via drain.
    release_unsubmitted(&mut work);
    let proven = work.completed || operation.is_none();
    drop(operation);
    drop(work.producer.take());
    crate::cleanup::defer(work);
    shared.completion_proven.store(proven, Ordering::Release);
    shared.finish(result);
}
