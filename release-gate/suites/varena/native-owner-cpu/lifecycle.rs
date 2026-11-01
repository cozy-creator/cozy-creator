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
    format!("{{\"passed\":true,\"native_thread_cancellations\":{},\"admitted_vs_queued\":true,\"drop_did_not_join\":true,\"poison_cancellation\":true,\"gpu_qualified\":false}}", rounds * 2)
}
