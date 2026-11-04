// Appended only to the output proof copy of cache_owner.rs. These are real
// native worker callbacks and requests; no CUDA owner or release is fabricated.
pub fn cold_recovery_host_proof() -> Result<serde_json::Value> {
    use std::sync::{mpsc, atomic::AtomicUsize};
    let device = 174;
    let (entered_send, entered) = mpsc::channel();
    let (release, released) = mpsc::channel();
    let first = CacheOwner::new(device, move |_| {
        // Callback recursion refuses before any CUDA initializer is reachable.
        assert!(matches!(crate::working::phase_enter_cold(device, "callback".into()),
            Err(Error::Invalid("cache callbacks cannot enter cold recovery"))));
        entered_send.send(()).unwrap();
        released.recv_timeout(Duration::from_secs(10)).unwrap();
        Ok(CallbackResult::Declined)
    })?;
    let second_calls = Arc::new(AtomicUsize::new(0));
    let count = second_calls.clone();
    let second = CacheOwner::new(device, move |_| {
        count.fetch_add(1, Ordering::AcqRel);
        Ok(CallbackResult::Declined)
    })?;
    let key = RequestKey { cause: Cause::Allocation, generation: next_generation()? };
    let request = request_shrink(device, key, 1)?.expect("real native request");
    entered.recv_timeout(Duration::from_secs(3)).unwrap();
    let joined = request_shrink(device, RequestKey {
        cause: Cause::Allocation, generation: next_generation()?,
    }, 2)?.expect("coalesced request");
    assert!(Arc::ptr_eq(&request.job, &joined.job));
    let since = crate::released_bytes(device)?;
    let started = Instant::now();
    let deadline = started + Duration::from_millis(150);
    assert!(!request.wait_allocation_release(since, deadline)?);
    assert!(Instant::now() >= deadline);
    let timeout_ms = started.elapsed().as_millis();
    let facts = request.facts()?;
    assert!(facts.running && !facts.cancelled && !facts.finished);
    assert_eq!(facts.requested_bytes, 2);
    drop(request); // This observer owns neither callback nor request lifetime.
    assert!(joined.control.lock()?.jobs.contains_key(&key));
    let (waiting_send, waiting) = mpsc::channel();
    let (done_send, done) = mpsc::channel();
    let observed = joined.clone();
    let waiter = std::thread::spawn(move || {
        waiting_send.send(()).unwrap();
        let deadline = Instant::now() + Duration::from_secs(3);
        let result = observed.wait_allocation_release(since, deadline);
        done_send.send((result, Instant::now() < deadline)).unwrap();
    });
    waiting.recv().unwrap();
    release.send(()).unwrap();
    // No physical release was invented: completion wakes the real observer with false.
    let (result, before_deadline) = done.recv_timeout(Duration::from_secs(1)).unwrap();
    assert!(!result? && before_deadline);
    waiter.join().unwrap();
    let facts = joined.facts()?;
    assert!(facts.finished && !facts.cancelled && !facts.running);
    assert_eq!(facts.returned, 2);
    assert_eq!(facts.declined, 2);
    assert_eq!(facts.observed_release_bytes, 0);
    assert_eq!(second_calls.load(Ordering::Acquire), 1);
    first.close()?;
    second.close()?;

    // The exact marker consumed by Working's cold permit is live across its scope.
    let marker = crate::enter_external_callback(device)?;
    {
        let captures = crate::capture_counts()?;
        assert_eq!(captures[&device].external_callbacks.load(Ordering::Acquire), 1);
    }
    drop(marker);
    assert_eq!(crate::capture_counts()?[&device].external_callbacks.load(Ordering::Acquire), 0);
    let maps = std::fs::read_to_string("/proc/self/maps").unwrap();
    assert!(!maps.contains("libcuda.so") && !maps.contains("libnvidia-ml.so"));
    Ok(serde_json::json!({
        "timeout_ms": timeout_ms,
        "timed_out_without_draining_or_cancelling": true,
        "coalesced_request_survives_observer": true,
        "actual_worker_terminal_before_deadline": true,
        "callback_cold_entry_refused": true,
        "external_callback_marker_lifetime": true,
        "fabricated_release_bytes": 0,
        "cuda_or_nvml_loaded": false,
        "gpu_qualified": false
    }))
}
