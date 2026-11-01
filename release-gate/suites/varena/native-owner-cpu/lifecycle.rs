// Appended by prove.py to an output-only copy of the exact owner source. This native
// concurrency harness exercises host control, not CUDA or a simulated GPU participant.
fn proof_control() -> Arc<Control> {
    let control = Arc::new(Control::new());
    control.lock().unwrap().facts.active = true;
    control
}

fn proof_queued(control: &Control, count: usize) {
    let deadline = std::time::Instant::now() + Duration::from_secs(2);
    while control.lock().unwrap().commands.len() != count {
        assert!(std::time::Instant::now() < deadline, "metadata callers did not queue");
        std::thread::yield_now();
    }
}

#[allow(unsafe_code)]
fn proof_inherited_control() {
    let control = proof_control();
    let owner = Owner { device: 0, id: ledger::RowId { slot: 0, incarnation: 1 }, control: Arc::clone(&control) };
    let held = control.lock().unwrap();
    // The child inherits a locked mutex. Every public control operation must reject by PID
    // before touching it. This is a real private-VM fork, with no CUDA or GPU participant.
    let child = unsafe { libc::fork() };
    assert!(child >= 0);
    if child == 0 {
        let inherited = |result: Result<()>| matches!(result, Err(super::Error::Ledger(ledger::Error::Inherited)));
        let passed = inherited(owner.facts().map(|_| ())) && inherited(owner.wake(0)) && inherited(owner.close());
        unsafe { libc::_exit(if passed { 0 } else { 1 }); }
    }
    let deadline = std::time::Instant::now() + Duration::from_secs(2);
    let mut status = 0;
    loop {
        let result = unsafe { libc::waitpid(child, &mut status, libc::WNOHANG) };
        if result == child { break; }
        assert!(result >= 0);
        if std::time::Instant::now() >= deadline {
            unsafe { libc::kill(child, libc::SIGKILL); libc::waitpid(child, &mut status, 0); }
            panic!("inherited control touched its locked parent mutex");
        }
        std::thread::sleep(Duration::from_millis(1));
    }
    drop(held);
    assert!(libc::WIFEXITED(status));
    assert_eq!(libc::WEXITSTATUS(status), 0);
}

pub fn lifecycle_proof() -> String {
    let rounds = 256;
    for _ in 0..rounds {
        let control = proof_control();
        let mut callers = Vec::new();
        let (done, results) = mpsc::channel();
        for _ in 0..2 {
            let waiting = Arc::clone(&control);
            let done = done.clone();
            callers.push(std::thread::spawn(move || {
                done.send(waiting.call(Command::Snapshot).is_err()).unwrap();
            }));
        }
        proof_queued(&control, 2);
        control.stop();
        for _ in 0..2 {
            assert!(results.recv_timeout(Duration::from_secs(1)).unwrap());
        }
        for caller in callers { caller.join().unwrap(); }
        assert!(control.call(Command::Snapshot).is_err());
        let state = control.lock().unwrap();
        assert!(state.stopping && !state.retired && !state.facts.active);
    }

    // An admitted finite metadata operation may finish; all still queued calls cancel.
    let control = proof_control();
    let (first, admitted_reply) = mpsc::sync_channel(1);
    let (second, queued_reply) = mpsc::sync_channel(1);
    {
        let mut state = control.lock().unwrap();
        state.commands.push_back(Command::CudaCalls(first));
        state.commands.push_back(Command::CudaCalls(second));
    }
    let admitted = control.lock().unwrap().commands.pop_front().unwrap();
    control.stop();
    assert!(queued_reply.recv_timeout(Duration::from_secs(1)).is_err());
    if let Command::CudaCalls(reply) = admitted { reply.send(Ok(17)).unwrap(); }
    assert_eq!(admitted_reply.recv_timeout(Duration::from_secs(1)).unwrap().unwrap(), 17);

    // Drop cannot join a worker that still owns host control and awaits an external release.
    let control = proof_control();
    let worker_control = Arc::clone(&control);
    let (release, held) = mpsc::channel();
    let worker = std::thread::spawn(move || {
        held.recv_timeout(Duration::from_secs(2)).unwrap();
        assert!(worker_control.lock().unwrap().stopping);
    });
    drop(Owner { device: 0, id: ledger::RowId { slot: 0, incarnation: 1 }, control });
    release.send(()).unwrap();
    worker.join().unwrap();

    // Poison metadata in an actual native thread; cancellation must still drop reply owners.
    let control = proof_control();
    let (send, receive) = mpsc::sync_channel(1);
    control.lock().unwrap().commands.push_back(Command::Snapshot(send));
    let held = Arc::clone(&control);
    assert!(std::thread::spawn(move || {
        let _state = held.state.lock().unwrap();
        panic!("intentional native metadata unwind");
    }).join().is_err());
    control.stop();
    assert!(receive.recv_timeout(Duration::from_secs(1)).is_err());

    // An actual close caller must return a permanent metadata fault even though no GPU
    // retirement was authorized. Do not manufacture a successful release or retired flag.
    let control = proof_control();
    let closing = Arc::clone(&control);
    let (done, result) = mpsc::sync_channel(1);
    let caller = std::thread::spawn(move || {
        let owner = Owner { device: 0, id: ledger::RowId { slot: 0, incarnation: 1 }, control: closing };
        done.send(owner.close().map_err(|error| error.to_string())).unwrap();
    });
    {
        let mut state = control.lock().unwrap();
        state.facts.failed = Some("permanent_retirement_fault".into());
        control.changed.notify_all();
    }
    let error = result.recv_timeout(Duration::from_secs(2)).unwrap().unwrap_err();
    assert!(error.contains("permanent_retirement_fault"), "{error}");
    assert!(!control.lock().unwrap().retired);
    caller.join().unwrap();
    proof_inherited_control();
    let budget_ms = recovery_wait_proof();
    format!("{{\"passed\":true,\"native_thread_cancellations\":{},\"admitted_vs_queued\":true,\"drop_did_not_join\":true,\"poison_cancellation\":true,\"permanent_fault_close_returned\":true,\"fault_not_marked_retired\":true,\"fork_rejected_before_inherited_mutex\":true,\"coordination_budget_ms\":{},\"final_retry_instruction\":true,\"deadline_not_reset\":true,\"control_remained_active_after_budget\":true,\"capture_mutex_free_while_waiting\":true,\"gpu_qualified\":false}}", rounds * 2, budget_ms)
}

fn recovery_wait_proof() -> u128 {
    // Host control only: no Peer/driver is substituted and no release is fabricated. The
    // real coordinator has no responder; its explicit deadline requests one final retry.
    let device = 91;
    let control = proof_control();
    registry().unwrap().insert(device, Arc::downgrade(&control));
    let (done, result) = mpsc::sync_channel(1);
    let caller = std::thread::spawn(move || {
        let mut recovery = crate::Recovery::default();
        let started = Instant::now();
        assert!(recovery.next(device, 2 << 20).unwrap());
        let elapsed = started.elapsed();
        let deadline = recovery.deadline;
        assert!(!recovery.next(device, 2 << 20).unwrap());
        assert_eq!(recovery.deadline, deadline);
        done.send(elapsed).unwrap();
    });
    let limit = Instant::now() + Duration::from_secs(1);
    while control.lock().unwrap().attempt.is_none() {
        assert!(Instant::now() < limit, "request did not reach native coordination");
        std::thread::yield_now();
    }
    let expires = control.lock().unwrap().attempt.as_ref().unwrap().deadline;
    drop(crate::capture_counts().unwrap());
    assert!(Instant::now() < expires, "capture mutex stayed held until the wait expired");
    let elapsed = result.recv_timeout(Duration::from_secs(3)).unwrap();
    assert!(elapsed >= COORDINATION_BUDGET);
    caller.join().unwrap();
    let facts = control.lock().unwrap();
    assert!(facts.facts.active && !facts.stopping && !facts.retired);
    assert_eq!(facts.facts.last_recovery.unwrap().outcome.as_str(), "peer_wait_budget_exhausted");
    assert_eq!(facts.facts.recoveries, 1);
    drop(facts);
    control.stop();
    registry().unwrap().remove(&device);
    assert!(!std::fs::read_to_string("/proc/self/maps").unwrap().contains("libcuda.so"));
    elapsed.as_millis()
}
