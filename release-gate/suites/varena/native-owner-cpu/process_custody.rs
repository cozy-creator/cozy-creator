// Output-only real host Control/ledger proof. A passive host hold is exercised
// directly; no Working/Driver is fabricated and GPU-bound attachment is not claimed.
pub fn process_custody_proof() {
    use std::os::unix::fs::DirBuilderExt;
    let directory = PathBuf::from(std::env::var_os("VARENA_HOST_PROOF_DIRECTORY").unwrap())
        .with_extension("custody");
    std::fs::DirBuilder::new()
        .mode(0o700)
        .create(&directory)
        .unwrap();
    let descriptor = ledger::Device::parse("GPU-00000000-0000-0000-0000-000000000097").unwrap();
    let identity = Identity::read(descriptor, &directory).unwrap();
    let device = 197; // Host registry key only; never initialized as a CUDA device.
    let (control, client, _) = reserve(device, &identity, true).unwrap();
    let ledger = ledger::Ledger::open(&directory, descriptor).unwrap();
    let participant = ledger.register(ledger::Sharing::Dedicated, 37).unwrap();
    let row = participant.id();
    host_ready(&control, row);
    let owner = owner_from_host(device, control.clone(), client, row);

    // The public attach method must not create a Working device. Its refused
    // candidate restores custody counts, including concurrent callers.
    std::thread::scope(|scope| {
        for _ in 0..16 {
            scope.spawn(|| assert!(owner.retain_working_lifetime().is_err()));
        }
    });
    assert_eq!(owner.facts().unwrap().clients, 1);
    assert_eq!(owner.facts().unwrap().passive_holds, 0);
    assert!(!crate::working::cooperation_bound(device).unwrap());
    assert!(crate::working::require_cooperation(device).is_err());
    for bytes in [0, 1, 2 << 20] {
        assert!(crate::working::alloc_bound(device, bytes, 0, None).is_err());
        assert!(crate::working::alloc_bound(device, bytes, 0, Some((1, 1))).is_err());
    }

    let mut pressure = super::pressure::proof_unobserved_pressure();
    let baseline = proof_control();
    let mut baseline_pressure = super::pressure::proof_unobserved_pressure();
    control
        .apply_boundary(&mut pressure, client, Boundary::RequestBegin)
        .unwrap();
    baseline
        .apply_boundary(&mut baseline_pressure, 1, Boundary::RequestBegin)
        .unwrap();
    let hold = owner.working_hold().unwrap();
    assert_eq!(owner.facts().unwrap().clients, 1);
    assert_eq!(owner.facts().unwrap().passive_holds, 1);
    {
        let state = control.lock().unwrap();
        assert!(!state.interrupted_window);
        assert_eq!(state.clients.len(), 1);
        assert_eq!(state.clients[&client].requests, 1);
        assert!(state.clients[&client].reported);
        assert_eq!(state.activity(), ledger::Activity::Active);
    }
    let duplicate = owner.working_hold().unwrap();
    assert!(hold.same_service(&duplicate));
    drop(duplicate);
    assert_eq!(
        control
            .apply_boundary(&mut pressure, client, Boundary::RequestEnd)
            .unwrap(),
        ledger::Activity::Idle
    );
    baseline
        .apply_boundary(&mut baseline_pressure, 1, Boundary::RequestEnd)
        .unwrap();
    assert_eq!(
        pressure.facts.measurement,
        baseline_pressure.facts.measurement
    );
    assert_eq!(
        pressure.facts.rejected_requests,
        baseline_pressure.facts.rejected_requests
    );
    assert_eq!(pressure.facts.measured_requests, 0); // No invented NVML attribution.
    assert_eq!(pressure.facts.local_reserve, None);
    assert!(!control.lock().unwrap().interrupted_window);

    // Execution-client closure is nonfinal because an independent passive hold
    // survives. Neither an extra Unknown client nor a false Idle report is added.
    assert!(owner.try_close().unwrap());
    let facts = owner.facts().unwrap();
    assert!(facts.closed && !facts.active && !facts.stopping && !facts.retired);
    assert_eq!((facts.clients, facts.passive_holds), (0, 1));
    assert_eq!(facts.activity, ledger::Activity::Unknown);
    assert!(
        ledger
            .snapshot()
            .unwrap()
            .rows
            .iter()
            .any(|entry| entry.id == row)
    );
    let command = control.lock().unwrap().commands.pop_front().unwrap();
    let Command::ReleaseClient(released) = command else {
        panic!("missing exact release command");
    };
    assert_eq!(released, client);
    assert_eq!(
        control.abandon_client(&mut pressure, released).unwrap(),
        ledger::Activity::Unknown
    );

    let next = Owner::join_existing(device).unwrap().unwrap();
    assert_eq!(next.id(), row);
    assert_eq!(next.facts().unwrap().passive_holds, 1);
    assert_eq!(next.facts().unwrap().clients, 1);
    assert!(control.lock().unwrap().facts.active);
    control
        .apply_boundary(&mut pressure, next.client, Boundary::RequestBegin)
        .unwrap();
    control
        .apply_boundary(&mut pressure, next.client, Boundary::RequestEnd)
        .unwrap();
    assert_eq!(next.facts().unwrap().activity, ledger::Activity::Idle);
    assert!(next.try_close().unwrap());
    assert!(!control.lock().unwrap().stopping);
    assert_eq!(
        control.lock().unwrap().activity(),
        ledger::Activity::Unknown
    );
    drop(hold);
    assert!(control.lock().unwrap().stopping);
    drop(participant);
    host_retired(&control);
    assert!(owner.try_close().unwrap() && next.try_close().unwrap());
    registry().unwrap().remove(&device);
    let maps = std::fs::read_to_string("/proc/self/maps").unwrap();
    assert!(!maps.contains("libcuda.so") && !maps.contains("libnvidia-ml.so"));
    eprintln!("process_custody_host_proof: passed; no synthetic GPU/Working or NVML samples");
}
