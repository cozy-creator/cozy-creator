// Exact product registry/control, actual tmpfs participants and real threads. Host
// Ready/retired markers exercise client admission, never a fabricated CUDA Peer.
#[allow(unsafe_code)] // Isolated real fork/owned-child wait, never compiled into product.
pub fn default_service_proof() {
    use std::os::unix::fs::DirBuilderExt;
    let root = PathBuf::from(std::env::var_os("VARENA_HOST_PROOF_DIRECTORY").unwrap())
        .with_extension("defaults");
    std::fs::DirBuilder::new()
        .mode(0o700)
        .create(&root)
        .unwrap();
    super::domain::default_domain_proof(&root);
    let directory = root.join("custom");
    std::fs::DirBuilder::new()
        .mode(0o700)
        .create(&directory)
        .unwrap();
    let descriptor = ledger::Device::parse("GPU-00000000-0000-0000-0000-000000000091").unwrap();
    let identity = Identity::read(descriptor, &directory).unwrap();
    let device = 191; // Registry key only. Never passed to CUDA discovery.
    assert!(Owner::join_existing(device).unwrap().is_none());
    let (control, first, _) = reserve(device, &identity, true).unwrap();
    assert!(is_busy(Owner::join_existing(device)));
    let ledger = ledger::Ledger::open(&directory, descriptor).unwrap();
    let participant = ledger.register(ledger::Sharing::Dedicated, 37).unwrap();
    let row = participant.id();
    host_ready(&control, row);
    let original = owner_from_host(device, control.clone(), first, row);
    // A child inherits the deliberately held registry mutex. Both public entry
    // paths must reject before that mutex, not discover a new default service.
    let held_registry = registry().unwrap();
    let child = unsafe { libc::fork() };
    assert!(child >= 0);
    if child == 0 {
        let rejected =
            Owner::join_existing(device).is_err() && Owner::get_or_open_default(device).is_err();
        unsafe {
            libc::_exit(if rejected { 0 } else { 1 });
        }
    }
    let deadline = Instant::now() + Duration::from_secs(2);
    let mut status = 0;
    loop {
        let result = unsafe { libc::waitpid(child, &mut status, libc::WNOHANG) };
        if result == child {
            break;
        }
        assert_eq!(result, 0);
        if Instant::now() >= deadline {
            unsafe {
                libc::kill(child, libc::SIGKILL);
                libc::waitpid(child, &mut status, 0);
            }
            panic!("inherited default join touched the held registry");
        }
        std::thread::yield_now();
    }
    assert!(libc::WIFEXITED(status) && libc::WEXITSTATUS(status) == 0);
    drop(held_registry);
    let inherited = Owner::get_or_open_default(device).unwrap();
    assert_eq!(inherited.id(), row);
    assert!(control.lock().unwrap().commands.is_empty());
    assert_eq!(
        control.lock().unwrap().activity(),
        ledger::Activity::Unknown
    );
    let snapshot = ledger.snapshot().unwrap();
    let actual = snapshot.rows.iter().find(|entry| entry.id == row).unwrap();
    assert_eq!(actual.sharing, ledger::Sharing::Dedicated);
    assert_eq!(actual.priority, 37);
    let mut threads = Vec::new();
    for _ in 0..16 {
        threads.push(std::thread::spawn(move || {
            Owner::join_existing(device).unwrap().unwrap()
        }));
    }
    let clients = threads
        .into_iter()
        .map(|thread| thread.join().unwrap())
        .collect::<Vec<_>>();
    assert_eq!(original.facts().unwrap().clients, 18);
    for client in clients {
        client.close().unwrap();
    }
    inherited.close().unwrap();

    // The stored directory path now names a different inode: no extra client may
    // be admitted. Restore only this fixture-owned empty replacement afterward.
    let displaced = root.join("displaced");
    std::fs::rename(&directory, &displaced).unwrap();
    std::fs::DirBuilder::new()
        .mode(0o700)
        .create(&directory)
        .unwrap();
    assert!(Owner::join_existing(device).is_err());
    assert_eq!(original.facts().unwrap().clients, 1);
    std::fs::remove_dir(&directory).unwrap();
    std::fs::rename(&displaced, &directory).unwrap();

    // Keep the actual old closed handle and Control reachable after metadata
    // retirement; neither may prevent a new service in a different domain.
    let capture = crate::enter_capture(device).unwrap();
    assert!(matches!(
        original.close(),
        Err(super::Error::Native(crate::Error::CaptureActive))
    ));
    assert!(is_busy(Owner::join_existing(device)));
    drop(participant);
    host_retired(&control);
    drop(capture);
    assert!(Owner::join_existing(device).unwrap().is_none());
    let other = root.join("other");
    std::fs::DirBuilder::new()
        .mode(0o700)
        .create(&other)
        .unwrap();
    let next_identity = Identity::read(descriptor, &other).unwrap();
    let (next, client, absent) = reserve_mode(device, &next_identity, true, true).unwrap();
    assert!(absent.is_none() && !Arc::ptr_eq(&control, &next));
    let next_ledger = ledger::Ledger::open(&other, descriptor).unwrap();
    let next_participant = next_ledger.register(ledger::Sharing::Shared, -7).unwrap();
    let next_row = next_participant.id();
    host_ready(&next, next_row);
    let next_owner = owner_from_host(device, next.clone(), client, next_row);
    assert!(is_busy(Owner::join_checked(
        device,
        control.clone(),
        identity.clone()
    )));
    assert!(is_busy(reserve_mode(device, &identity, true, true)));
    assert_eq!(next_owner.facts().unwrap().clients, 1);
    let next_capture = crate::enter_capture(device).unwrap();
    original.close().unwrap();
    drop(original);
    drop(next_capture);
    let default = Owner::get_or_open_default(device).unwrap();
    assert_eq!(default.id(), next_row);
    default.close().unwrap();
    assert!(next_owner.facts().unwrap().active);

    // A real metadata panic records the original fault, followed by the actual
    // finish_retirement tombstone path. It is not a simulated CUDA release.
    let failing = next.clone();
    assert!(
        std::thread::spawn(move || {
            let mut state = failing.lock().unwrap();
            state.facts.failed = Some("default_service_original_failure".into());
            panic!("intentional default-service metadata fault");
        })
        .join()
        .is_err()
    );
    finish_retirement(device, &next, false);
    for failed in [
        Owner::join_existing(device).map(|_| ()),
        Owner::get_or_open_default(device).map(|_| ()),
        reserve_mode(device, &identity, true, true).map(|_| ()),
    ] {
        assert!(
            failed
                .unwrap_err()
                .to_string()
                .contains("default_service_original_failure")
        );
    }
    let weak = Arc::downgrade(&next);
    drop(default);
    drop(next_owner);
    drop(next);
    assert!(weak.upgrade().is_none());
    let failure = match Owner::get_or_open_default(device) {
        Err(error) => error.to_string(),
        Ok(_) => panic!("failed default service reopened"),
    };
    assert!(failure.contains("default_service_original_failure"));
    drop(next_participant);
    registry().unwrap().remove(&device); // Fixture-owned synthetic registry key only.
    let maps = std::fs::read_to_string("/proc/self/maps").unwrap();
    assert!(!maps.contains("libcuda.so") && !maps.contains("libnvidia-ml.so"));
    eprintln!(
        "default_service_host_proof: passed; actual files/registry; no CUDA or NVML; GPU discovery unrun"
    );
}
