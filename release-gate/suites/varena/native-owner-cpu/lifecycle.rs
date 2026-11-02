// Appended by prove.py to an output-only copy of the exact owner source. This native
// concurrency harness exercises host control, not CUDA or a simulated GPU participant.
fn proof_control() -> Arc<Control> {
    let control = Arc::new(Control::new());
    {
        let mut state = control.lock().unwrap();
        state.facts.active = true;
        assert_eq!(state.add_client().unwrap(), 1);
    }
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
    let owner = Owner { client: 1, device: 0, id: ledger::RowId { slot: 0, incarnation: 1 }, control: Arc::clone(&control) };
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
                done.send(waiting.call(1, Command::Snapshot).is_err()).unwrap();
            }));
        }
        proof_queued(&control, 2);
        control.stop();
        for _ in 0..2 {
            assert!(results.recv_timeout(Duration::from_secs(1)).unwrap());
        }
        for caller in callers { caller.join().unwrap(); }
        assert!(control.call(1, Command::Snapshot).is_err());
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
    drop(Owner { client: 1, device: 0, id: ledger::RowId { slot: 0, incarnation: 1 }, control });
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
        let owner = Owner { client: 1, device: 0, id: ledger::RowId { slot: 0, incarnation: 1 }, control: closing };
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
    recovery_lifetimes_proof();
    pressure_lifetimes_proof();
    cache_callback_lifetimes_proof();
    cache_service_epoch_proof();
    shared_clients_proof();
    format!("{{\"passed\":true,\"native_thread_cancellations\":{},\"admitted_vs_queued\":true,\"drop_did_not_join\":true,\"poison_cancellation\":true,\"permanent_fault_close_returned\":true,\"fault_not_marked_retired\":true,\"fork_rejected_before_inherited_mutex\":true,\"coordination_budget_ms\":{},\"final_retry_instruction\":true,\"deadline_not_reset\":true,\"control_remained_active_after_budget\":true,\"capture_mutex_free_while_waiting\":true,\"local_waiter_drop_preserves_other_demand\":true,\"withdrawal_latched_across_new_demand\":true,\"stopped_control_cancels_recovery\":true,\"pressure_withdrawal_preserves_allocation_owner\":true,\"queued_request_boundary_cancels\":true,\"no_nvml_or_cuda_loaded\":true,\"real_callback_cancellation_returned_busy\":true,\"cooperation_close_refused_dispatcher_self_wait\":true,\"callback_return_granted_no_bytes\":true,\"repeated_queue_did_not_advance_owner\":true,\"stale_service_epoch_did_not_advance_owner\":true,\"last_callback_required_native_service\":true,\"shared_client_lifetime\":true,\"shared_client_races\":256,\"failed_service_tombstone_survived_last_handle\":true,\"old_close_ignored_new_capture\":true,\"unreported_clients_remain_unknown\":true,\"client_abandonment_did_not_train_H\":true,\"gpu_qualified\":false}}", rounds * 2, budget_ms)
}

fn cache_service_epoch_proof() {
    use crate::cache_owner::{CacheOwner,CallbackResult,Cause,RequestKey,next_generation,request_shrink};
    let device=195;
    let (first,first_done)=mpsc::sync_channel(1);
    let (second,second_done)=mpsc::sync_channel(1);
    let a=CacheOwner::new(device,move |wanted| {first.send(wanted).unwrap();Ok(CallbackResult::Declined)}).unwrap();
    let b=CacheOwner::new(device,move |wanted| {second.send(wanted).unwrap();Ok(CallbackResult::Returned)}).unwrap();
    let key=RequestKey{cause:Cause::Cooperative,generation:next_generation().unwrap()};
    let request=request_shrink(device,key,32).unwrap().unwrap();
    assert_eq!(first_done.recv_timeout(Duration::from_secs(2)).unwrap(),32);
    let wait_epoch=|epoch| {
        let limit=Instant::now()+Duration::from_secs(2);
        loop {
            let facts=request.facts().unwrap();
            if facts.action_epoch>=epoch {return facts;}
            assert!(Instant::now()<limit);
            std::thread::yield_now();
        }
    };
    assert!(!wait_epoch(1).finished);
    // Actual worker observation intervals, not a process-death inference. Repeating the
    // queue request or supplying a pass that predates the action cannot dispatch B.
    request_shrink(device,key,32).unwrap();
    std::thread::sleep(Duration::from_millis(100));
    assert!(matches!(second_done.try_recv(),Err(mpsc::TryRecvError::Empty)));
    request.service_progress(0,0).unwrap();
    std::thread::sleep(Duration::from_millis(100));
    assert!(matches!(second_done.try_recv(),Err(mpsc::TryRecvError::Empty)));
    assert!(request.service_progress(33,1).is_err());
    assert!(request.service_progress(0,2).is_err());
    request.service_progress(0,1).unwrap();
    assert_eq!(second_done.recv_timeout(Duration::from_secs(2)).unwrap(),32);
    let pending=wait_epoch(2);
    assert!(!pending.finished && pending.observed_release_bytes==0 && pending.serviced_bytes==0);
    request.service_progress(0,1).unwrap();
    assert!(!request.facts().unwrap().finished);
    request.service_progress(0,2).unwrap();
    let complete=request.facts().unwrap();
    assert!(complete.finished && complete.returned==2 && complete.declined==1);
    assert_eq!(complete.serviced_bytes,0);
    a.close().unwrap();b.close().unwrap();
}

fn cache_callback_lifetimes_proof() {
    use crate::cache_owner::{CacheOwner, CallbackResult, Cause, RequestKey, next_generation, request_shrink};
    let device = 194; // Real host metadata only; no CUDA device is opened or substituted.
    let control = proof_control();
    let callback_owner = Owner { client: 1, device, id: ledger::RowId {slot:0,incarnation:1}, control: Arc::clone(&control) };
    let (entered, started) = mpsc::sync_channel(1);
    let (release, held) = mpsc::sync_channel(1);
    let (closed, close_result) = mpsc::sync_channel(1);
    let cache = CacheOwner::new(device, move |_| {
        assert!(crate::cache_owner::is_dispatcher_thread(device)?);
        entered.send(()).unwrap();
        held.recv_timeout(Duration::from_secs(5)).unwrap();
        // The metadata-only cooperation handle must refuse a self-wait on this actual
        // native callback thread; the fixture does not fabricate a Peer or GPU release.
        let busy = matches!(callback_owner.close(), Err(super::Error::Native(crate::Error::Busy)));
        closed.send(busy).unwrap();
        Ok(CallbackResult::Returned)
    }).unwrap();
    let key = RequestKey {cause:Cause::Cooperative,generation:next_generation().unwrap()};
    let request = request_shrink(device,key,32).unwrap().unwrap();
    started.recv_timeout(Duration::from_secs(2)).unwrap();
    let mut state = super::State::default();
    state.shrink = Some(super::Shrink {round:77,key,request:Some(request),queueing:false,start_completed:0,target:32});
    let (sent, returned) = mpsc::sync_channel(1);
    let cancellation = std::thread::spawn(move || {
        let busy = matches!(state.cancel_shrink(), Err(super::Error::Native(crate::Error::Busy)));
        sent.send((state,busy)).unwrap();
    });
    let (mut state,busy) = returned.recv_timeout(Duration::from_secs(2)).unwrap();
    cancellation.join().unwrap();
    assert!(busy, "cancellation must refuse, not wait for the blocked callback");
    let facts = state.shrink.as_ref().unwrap().request.as_ref().unwrap().facts().unwrap();
    assert!(facts.cancelled && facts.running && facts.observed_release_bytes == 0);
    release.send(()).unwrap();
    assert!(close_result.recv_timeout(Duration::from_secs(2)).unwrap());
    let limit = Instant::now()+Duration::from_secs(2);
    while cache.facts().unwrap().running {
        assert!(Instant::now()<limit);
        std::thread::yield_now();
    }
    state.cancel_shrink().unwrap();
    assert!(state.shrink.is_none());
    cache.close().unwrap();
    assert!(cache.facts().unwrap().retired);
    assert!(control.lock().unwrap().stopping);
    let maps = std::fs::read_to_string("/proc/self/maps").unwrap();
    assert!(!maps.contains("libcuda.so") && !maps.contains("libnvidia-ml.so"));
}

fn pressure_lifetimes_proof() {
    // Real host-control race: registering allocation demand must queue only the pressure
    // generation, leaving a separately owned allocation generation alone. No Peer is faked.
    let device = 93;
    let control = proof_control();
    {
        let mut state = control.lock().unwrap();
        state.pressure_round = Some(101);
        state.owned_round = Some(102);
        state.demands.insert(0, Instant::now() + COORDINATION_BUDGET);
        state.next_demand = 1;
    }
    registry().unwrap().insert(device, Registered::new(&control));
    let (done, receive) = mpsc::sync_channel(1);
    let caller = std::thread::spawn(move || {
        let mut recovery = crate::Recovery::default();
        done.send(recovery.next(device, 2 << 20, true).is_err()).unwrap();
    });
    let limit = Instant::now() + Duration::from_secs(1);
    loop {
        let state = control.lock().unwrap();
        if state.demands.len() == 2 {
            assert!(state.pressure_round.is_none());
            assert!(state.withdrawals.contains(&101));
            assert!(!state.withdrawals.contains(&102));
            assert_eq!(state.owned_round, Some(102));
            break;
        }
        assert!(Instant::now() < limit);
        drop(state);
        std::thread::yield_now();
    }
    control.stop();
    assert!(receive.recv_timeout(Duration::from_secs(1)).unwrap());
    caller.join().unwrap();
    registry().unwrap().remove(&device);

    let control = proof_control();
    let waiting = Arc::clone(&control);
    let caller = std::thread::spawn(move || {
        let owner = Owner { client: 1, device: 0, id: ledger::RowId { slot: 0, incarnation: 1 }, control: waiting };
        assert!(owner.boundary(Boundary::RequestBegin).is_err());
    });
    proof_queued(&control, 1);
    control.stop();
    caller.join().unwrap();
    let maps = std::fs::read_to_string("/proc/self/maps").unwrap();
    assert!(!maps.contains("libcuda.so") && !maps.contains("libnvidia-ml.so"));
}

fn recovery_wait_proof() -> u128 {
    // Host control only: no Peer/driver is substituted and no release is fabricated. The
    // real coordinator has no responder; its explicit deadline requests one final retry.
    let device = 91;
    let control = proof_control();
    registry().unwrap().insert(device, Registered::new(&control));
    let (done, result) = mpsc::sync_channel(1);
    let caller = std::thread::spawn(move || {
        let mut recovery = crate::Recovery::default();
        let started = Instant::now();
        assert!(recovery.next(device, 2 << 20, true).unwrap());
        let elapsed = started.elapsed();
        let deadline = recovery.deadline;
        assert!(!recovery.next(device, 2 << 20, true).unwrap());
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
    assert!(facts.demands.is_empty());
    drop(facts);
    control.stop();
    registry().unwrap().remove(&device);
    assert!(!std::fs::read_to_string("/proc/self/maps").unwrap().contains("libcuda.so"));
    elapsed.as_millis()
}

fn recovery_lifetimes_proof() {
    // Host lifetime markers only: the separate three-process ledger proof checks actual
    // ownership/withdrawal publication. This creates no Peer and asserts no GPU release.
    let control = proof_control();
    let until = Instant::now() + COORDINATION_BUDGET;
    {
        let mut state = control.lock().unwrap();
        state.demands.insert(0, until);
        state.demands.insert(1, until);
        state.next_demand = 2;
        state.owned_round = Some(77);
        state.want_published = true;
    }
    let first = Participation { control: Arc::clone(&control), id: 0 };
    let second = Participation { control: Arc::clone(&control), id: 1 };
    std::thread::spawn(move || drop(first)).join().unwrap();
    {
        let state = control.lock().unwrap();
        assert_eq!(state.owned_round, Some(77));
        assert!(state.withdrawals.is_empty() && !state.clear_want);
    }
    drop(second);
    {
        let state = control.lock().unwrap();
        assert!(state.withdrawals.contains(&77) && state.clear_want);
        assert!(state.demands.is_empty() && state.owned_round.is_none());
    }
    let device = 92;
    registry().unwrap().insert(device, Registered::new(&control));
    let (done, result) = mpsc::sync_channel(1);
    let caller = std::thread::spawn(move || {
        let mut recovery = crate::Recovery::default();
        done.send(recovery.next(device, 2 << 20, true).is_err()).unwrap();
    });
    let limit = Instant::now() + Duration::from_secs(1);
    while control.lock().unwrap().demands.is_empty() {
        assert!(Instant::now() < limit, "new requester did not register");
        std::thread::yield_now();
    }
    {
        let state = control.lock().unwrap();
        assert!(state.withdrawals.contains(&77) && state.clear_want);
        assert!(state.owned_round.is_none());
    }
    control.stop();
    assert!(result.recv_timeout(Duration::from_secs(1)).unwrap());
    caller.join().unwrap();
    assert!(control.lock().unwrap().demands.is_empty());
    registry().unwrap().remove(&device);
}
