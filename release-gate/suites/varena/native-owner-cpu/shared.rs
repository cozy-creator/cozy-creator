// Appended only to the proof copy of owner.rs. Exercises the actual admission,
// client commands and lifetime controls with real threads/channels/ledger files.
// Ready rows below are REAL metadata participants, NOT CUDA Peers. Setting host
// Ready/retired facts here authorizes only this host lifecycle proof; no device
// initialization, NVML observation or GPU retirement is simulated or claimed.

fn host_ready(control: &Control, row: ledger::RowId) {
    let mut state = control.lock().unwrap();
    state.row = Some(row);
    state.facts.active = true;
}

fn host_retired(control: &Control) {
    let mut state = control.lock().unwrap();
    assert!(state.stopping);
    state.retired = true;
    control.changed.notify_all();
}

fn owner_from_host(device: i32, control: Arc<Control>, client: u64, row: ledger::RowId) -> Owner {
    Owner { device, control, client, id: row }
}

fn is_busy<T>(result: Result<T>) -> bool {
    matches!(result, Err(super::Error::Native(crate::Error::Busy)))
}

fn shared_clients_proof() {
    let root = PathBuf::from(std::env::var_os("VARENA_HOST_PROOF_DIRECTORY").unwrap());
    use std::os::unix::fs::DirBuilderExt;
    std::fs::DirBuilder::new().mode(0o700).create(&root).unwrap();
    let descriptor = ledger::Device::parse("GPU-00000000-0000-0000-0000-000000000077").unwrap();
    let identity = Identity::read(descriptor, &root).unwrap();
    let ledger = ledger::Ledger::open(&root, descriptor).unwrap();
    let device = 177; // Host registry key, never passed to a CUDA initializer.

    // Real simultaneous first reservations admit one creator, and never expose Opening
    // as Ready. Keep the successful Control alive through every racing attempt.
    let (send, receive) = mpsc::channel();
    let barrier = Arc::new(std::sync::Barrier::new(17));
    let mut threads = Vec::new();
    for _ in 0..16 {
        let send = send.clone();
        let barrier = barrier.clone();
        let identity = identity.clone();
        threads.push(std::thread::spawn(move || {
            barrier.wait();
            send.send(reserve(device, &identity, true)).unwrap();
        }));
    }
    barrier.wait();
    let mut created = None;
    for _ in 0..16 {
        match receive.recv_timeout(Duration::from_secs(2)).unwrap() {
            Ok((control, client, row)) => {
                assert!(created.is_none() && row.is_none());
                created = Some((control, client));
            }
            error => assert!(is_busy(error)),
        }
    }
    for thread in threads { thread.join().unwrap(); }
    let (control, first) = created.unwrap();
    let participant = ledger.register(ledger::Sharing::Dedicated, 37).unwrap();
    let row = participant.id();
    host_ready(&control, row);
    let original = owner_from_host(device, control.clone(), first, row);
    let held = control.lock().unwrap();
    assert!(is_busy(original.try_close()));
    assert!(!held.clients.get(&first).unwrap().released);
    drop(held);

    // Both named arrival orders use the same shared API. A caller's creation defaults
    // must not queue Configure or replace the actual participant's categorical settings.
    let public_after_runtime = Owner::get_or_open(device, descriptor, &root, ledger::Sharing::Shared, -4).unwrap();
    assert_eq!(original.id(), public_after_runtime.id());
    assert!(control.lock().unwrap().commands.is_empty());
    let snapshot = ledger.snapshot().unwrap();
    let actual = snapshot.rows.iter().find(|r| r.id == row).unwrap();
    assert_eq!(actual.sharing, ledger::Sharing::Dedicated);
    assert_eq!(actual.priority, 37);
    assert!(is_busy(Owner::open(device, descriptor, &root, ledger::Sharing::Shared, 0)));

    // Wrong ledger/device identities cannot alias a process/device service. An alias
    // of the SAME directory resolves to the same identity and is permitted.
    let other = root.join("other"); std::fs::create_dir(&other).unwrap();
    assert!(Owner::get_or_open(device, descriptor, &other, ledger::Sharing::Shared, 0).is_err());
    let other_device = ledger::Device::parse("GPU-00000000-0000-0000-0000-000000000078").unwrap();
    assert!(Owner::get_or_open(device, other_device, &root, ledger::Sharing::Shared, 0).is_err());
    let alias = root.join("alias"); std::os::unix::fs::symlink(&root, &alias).unwrap();
    let alias_client = Owner::get_or_open(device, descriptor, &alias, ledger::Sharing::Shared, 0).unwrap();
    assert!(alias_client.try_close().unwrap());
    assert!(alias_client.try_close().unwrap());
    assert!(original.facts().unwrap().active);

    let mut clients = Vec::new();
    let mut threads = Vec::new();
    for _ in 0..16 {
        let directory = root.clone();
        threads.push(std::thread::spawn(move || Owner::get_or_open(device, descriptor, &directory, ledger::Sharing::Shared, -99).unwrap()));
    }
    for thread in threads { clients.push(thread.join().unwrap()); }
    assert_eq!(original.facts().unwrap().clients, 18);
    original.close().unwrap();
    original.close().unwrap();
    assert!(!original.facts().unwrap().active && original.facts().unwrap().closed);
    assert!(original.wake(1).is_err());
    assert!(public_after_runtime.facts().unwrap().active);
    assert_eq!(public_after_runtime.facts().unwrap().clients, 17);
    for client in clients { client.close().unwrap(); }
    assert_eq!(public_after_runtime.facts().unwrap().clients, 1);

    // Final close has a retryable ticket under a REAL registered capture exclusion.
    // The real ledger row remains live; this does not stand in for CUDA retirement.
    let capture = crate::enter_capture(device).unwrap();
    let held_capture = crate::capture_counts().unwrap();
    assert!(!public_after_runtime.try_close().unwrap());
    assert!(!public_after_runtime.try_close().unwrap());
    drop(held_capture); // try_close never consults this deliberately held global mutex.
    assert!(matches!(public_after_runtime.close(), Err(super::Error::Native(crate::Error::CaptureActive))));
    assert_eq!(public_after_runtime.facts().unwrap().clients, 0);
    assert!(!public_after_runtime.facts().unwrap().retired);
    assert!(ledger.snapshot().unwrap().rows.iter().any(|r| r.id == row));
    assert!(is_busy(Owner::get_or_open(device, descriptor, &root, ledger::Sharing::Shared, 0)));
    drop(capture);
    let (closed, result) = mpsc::channel();
    let close_thread = std::thread::spawn(move || {
        closed.send(public_after_runtime.close()).unwrap();
        public_after_runtime
    });
    assert!(matches!(result.recv_timeout(Duration::from_millis(30)), Err(mpsc::RecvTimeoutError::Timeout)));
    drop(participant);
    host_retired(&control);
    result.recv_timeout(Duration::from_secs(2)).unwrap().unwrap();
    let final_old = close_thread.join().unwrap();
    assert!(!ledger.snapshot().unwrap().rows.iter().any(|r| r.id == row));

    // Public-first then Runtime-style shared acquisition. Old closed handles remain
    // alive and cannot prevent or stop a new, proved-retired service generation.
    let (next, client, absent) = reserve(device, &identity, true).unwrap();
    assert!(absent.is_none() && !Arc::ptr_eq(&next, &control));
    let participant = ledger.register(ledger::Sharing::Shared, -7).unwrap();
    let next_row = participant.id();
    assert_ne!(row, next_row);
    host_ready(&next, next_row);
    let public_first = owner_from_host(device, next.clone(), client, next_row);
    let runtime_second = Owner::get_or_open(device, descriptor, &root, ledger::Sharing::Dedicated, 91).unwrap();
    let new_capture = crate::enter_capture(device).unwrap();
    assert!(original.try_close().unwrap());
    assert!(final_old.try_close().unwrap());
    original.close().unwrap(); final_old.close().unwrap();
    drop(new_capture);
    drop(original); drop(final_old);
    assert_eq!(runtime_second.id(), next_row);
    assert!(runtime_second.facts().unwrap().active);
    runtime_second.close().unwrap();
    assert!(public_first.facts().unwrap().active);
    let snapshot = ledger.snapshot().unwrap();
    let actual = snapshot.rows.iter().find(|r| r.id == next_row).unwrap();
    assert_eq!(actual.sharing, ledger::Sharing::Shared);
    assert_eq!(actual.priority, -7);
    drop(public_first); // A held worker/metadata Arc must not count as a client.
    assert!(next.lock().unwrap().stopping);
    drop(participant); host_retired(&next);
    registry().unwrap().remove(&device);

    shared_admission_races(&identity);
    shared_boundaries_proof();
    shared_failure_proof(&identity);
    let maps = std::fs::read_to_string("/proc/self/maps").unwrap();
    assert!(!maps.contains("libcuda.so") && !maps.contains("libnvidia-ml.so"));
}

fn shared_admission_races(identity: &Identity) {
    let device = 178;
    for round in 0..256 {
        let (control, first, _) = reserve(device, identity, true).unwrap();
        let row = ledger::RowId {slot:0, incarnation:round + 1};
        host_ready(&control, row); // Host lease stress only, not a ledger/GPU row claim.
        let barrier = Arc::new(std::sync::Barrier::new(3));
        let releasing = control.clone();
        let release_barrier = barrier.clone();
        let release = std::thread::spawn(move || {
            release_barrier.wait(); releasing.release_client(first)
        });
        let identity = identity.clone();
        let admit_barrier = barrier.clone();
        let admit = std::thread::spawn(move || {
            admit_barrier.wait(); reserve(device, &identity, true)
        });
        barrier.wait();
        let final_client = release.join().unwrap();
        match admit.join().unwrap() {
            Ok((joined, client, actual_row)) => {
                assert!(!final_client && Arc::ptr_eq(&joined, &control));
                assert_eq!(actual_row, Some(row));
                assert!(joined.release_client(client));
            }
            error => { assert!(final_client && is_busy(error)); }
        }
        assert!(control.lock().unwrap().stopping);
        assert!(!control.lock().unwrap().clients.values().any(|client| !client.released));
        registry().unwrap().remove(&device);
    }
}

fn shared_boundaries_proof() {
    let control = proof_control();
    let a = 1;
    let b = control.lock().unwrap().add_client().unwrap();
    let c = control.lock().unwrap().add_client().unwrap();
    let mut pressure = super::pressure::proof_unobserved_pressure();
    assert_eq!(control.lock().unwrap().activity(), ledger::Activity::Unknown);
    control.apply_boundary(&mut pressure, a, Boundary::RequestBegin).unwrap();
    control.apply_boundary(&mut pressure, a, Boundary::RequestBegin).unwrap();
    control.apply_boundary(&mut pressure, b, Boundary::RequestBegin).unwrap();
    assert!(control.apply_boundary(&mut pressure, c, Boundary::RequestEnd).is_err());
    assert_eq!(control.lock().unwrap().activity(), ledger::Activity::Active);
    assert!(!control.release_client(a));
    assert!(matches!(control.lock().unwrap().commands.pop_front(), Some(Command::ReleaseClient(id)) if id == a));
    assert_eq!(control.abandon_client(&mut pressure, a).unwrap(), ledger::Activity::Active);
    assert_eq!(control.apply_boundary(&mut pressure, b, Boundary::RequestEnd).unwrap(), ledger::Activity::Unknown); // c has not reported
    assert_eq!(pressure.facts.local_reserve, None);
    assert_eq!(pressure.facts.measured_requests, 0);
    assert_eq!(pressure.facts.rejected_requests, 2); // abandonment, then invalidated completion
    assert!(control.next_boundary(b, Boundary::RequestEnd).is_err());

    // Actual blocked caller/channel: close must enqueue abandonment AFTER a boundary
    // already admitted for this client, even if the caller hasn't received its reply.
    let calling = control.clone();
    let caller = std::thread::spawn(move || calling.call(c, |reply| Command::Boundary(c, Boundary::RequestBegin, reply)));
    proof_queued(&control, 1);
    assert!(!control.release_client(c));
    assert_eq!(control.lock().unwrap().commands.len(), 2);
    let admitted = control.lock().unwrap().commands.pop_front().unwrap();
    if let Command::Boundary(id, kind, reply) = admitted {
        reply.send(control.apply_boundary(&mut pressure, id, kind).map(|_| ())).unwrap();
    } else { panic!("boundary ordering changed"); }
    caller.join().unwrap().unwrap();
    let release = control.lock().unwrap().commands.pop_front().unwrap();
    if let Command::ReleaseClient(id) = release {
        assert_eq!(control.abandon_client(&mut pressure, id).unwrap(), ledger::Activity::Idle);
    } else { panic!("abandonment ordering changed"); }
    assert_eq!(pressure.facts.measurement, Some("abandoned_request_window"));
    assert_eq!(pressure.facts.local_reserve, None);
    assert_eq!(pressure.facts.measured_requests, 0);
    // A short-lived unknown client cannot vanish between two boundary samples without
    // invalidating the active request's attribution. No synthetic NVML values are used.
    control.apply_boundary(&mut pressure, b, Boundary::RequestBegin).unwrap();
    let late = control.lock().unwrap().add_client().unwrap();
    assert!(control.lock().unwrap().interrupted_window);
    assert!(!control.release_client(late));
    let release = control.lock().unwrap().commands.pop_front().unwrap();
    assert!(matches!(release, Command::ReleaseClient(id) if id == late));
    control.abandon_client(&mut pressure, late).unwrap();
    assert!(control.lock().unwrap().interrupted_window);
    assert_eq!(control.apply_boundary(&mut pressure, b, Boundary::RequestEnd).unwrap(), ledger::Activity::Idle);
    assert!(!control.lock().unwrap().interrupted_window);
    assert_eq!(pressure.facts.measured_requests, 0);
    assert!(control.release_client(b));
}

fn shared_failure_proof(identity: &Identity) {
    let device = 179;
    let (control, client, _) = reserve(device, identity, true).unwrap();
    let row = ledger::RowId { slot: 0, incarnation: 1 };
    host_ready(&control, row);
    let owner = owner_from_host(device, control.clone(), client, row);
    let failed = control.clone();
    // An actual native panic poisons the actual host mutex. This does not fabricate
    // a CUDA fault or claim that any GPU resource was released/quarantined.
    assert!(std::thread::spawn(move || {
        let mut state = failed.lock().unwrap();
        state.facts.failed = Some("original_native_retirement_failure".into());
        panic!("intentional shared-service metadata retirement fault");
    }).join().is_err());
    assert!(owner.close().is_err());
    assert!(owner.try_close().unwrap_err().to_string().contains("original_native_retirement_failure"));
    finish_retirement(device, &control, false);
    assert!(!control.state.lock().unwrap_or_else(|e| e.into_inner()).retired);
    let weak = Arc::downgrade(&control);
    drop(owner); drop(control);
    assert!(weak.upgrade().is_none(), "tombstone retained more than host failure text");
    let error = match reserve(device, identity, true) { Err(error) => error.to_string(), Ok(_) => panic!("failed service restarted") };
    assert!(error.contains("original_native_retirement_failure"), "{error}");
    assert!(registered(device).is_err());
    // Fixture removes only its synthetic device-key tombstone after proving persistence.
    registry().unwrap().remove(&device);
}
