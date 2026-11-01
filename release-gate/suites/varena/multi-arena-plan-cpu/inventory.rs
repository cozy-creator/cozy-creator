// Appended only to plan.rs's output proof copy. Actual metadata, locks and epochs;
// no Arena/Region is fabricated and no GPU publication/retirement is claimed.
pub(crate) fn inventory_host_proof() {
    let device = 181;
    let other = 182;
    let other_lease = Lease::acquire(other).unwrap();
    for _ in 0..4 {
        let lease = Lease::acquire(device).unwrap();
        let before = inventory_generation(device).unwrap();
        {
            let _capture = crate::capture_counts().unwrap();
            inventory_changed(device).unwrap();
        }
        assert_eq!(inventory_generation(device).unwrap(), before + 1);
        assert!(lease.validate().is_err());
        assert!(other_lease.validate().is_ok());
    }
    let before = inventory_generation(device).unwrap();
    let mut threads = Vec::new();
    for _ in 0..16 {
        threads.push(std::thread::spawn(move || {
            for _ in 0..32 {
                let _capture = crate::capture_counts().unwrap();
                inventory_changed(device).unwrap();
            }
        }));
    }
    for thread in threads { thread.join().unwrap(); }
    assert_eq!(inventory_generation(device).unwrap(), before + 512);
    assert!(other_lease.validate().is_ok());

    // Both counters must fail closed when the caller cannot return an overflow
    // (as in a destructor). Do not wrap or leave a previously accepted epoch valid.
    for (device, inventory) in [(183, true), (184, false)] {
        {
            let mut registry = registry().unwrap();
            let entry = registry.entry(device).or_default();
            if inventory { entry.inventory_generation = u64::MAX - 1; }
            else { entry.generation = u64::MAX - 1; }
        }
        let lease = Lease::acquire(device).unwrap();
        {
            let _capture = crate::capture_counts().unwrap();
            if inventory { let _ = inventory_changed(device); }
            else { let _ = registry().unwrap().get_mut(&device).unwrap().advance(false); }
        }
        assert!(lease.validate().is_err());
        assert!(generation(device).is_err());
        assert!(inventory_generation(device).is_err());
        assert!(inventory_changed(device).is_err());
    }
    inherited_registry_proof();
}

#[allow(unsafe_code)]
fn inherited_registry_proof() {
    let held = registry().unwrap();
    let pid = unsafe { libc::fork() };
    assert!(pid >= 0);
    if pid == 0 {
        let refused = inventory_generation(181).is_err();
        unsafe { libc::_exit(if refused { 0 } else { 1 }); }
    }
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
    let mut status = 0;
    loop {
        let result = unsafe { libc::waitpid(pid, &mut status, libc::WNOHANG) };
        if result == pid { break; }
        assert!(result >= 0);
        if std::time::Instant::now() >= deadline {
            unsafe { libc::kill(pid, libc::SIGKILL); libc::waitpid(pid, &mut status, 0); }
            panic!("inherited plan registry mutex was touched");
        }
        std::thread::sleep(std::time::Duration::from_millis(1));
    }
    drop(held);
    assert!(libc::WIFEXITED(status) && libc::WEXITSTATUS(status) == 0);
}
