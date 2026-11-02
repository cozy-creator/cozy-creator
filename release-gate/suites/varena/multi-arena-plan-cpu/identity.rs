// Appended to an output-only copy of lib.rs. Runs the real process identity allocator.
pub fn lifetime_inventory_host_proof() -> String {
    let mut threads = Vec::new();
    for _ in 0..16 {
        threads.push(std::thread::spawn(|| {
            (0..512).map(|_| next_identity().unwrap()).collect::<Vec<_>>()
        }));
    }
    let ids: Vec<_> = threads.into_iter().flat_map(|thread| thread.join().unwrap()).collect();
    let unique: std::collections::BTreeSet<_> = ids.iter().copied().collect();
    assert_eq!(ids.len(), 8192);
    assert_eq!(unique.len(), ids.len());
    assert!(!unique.contains(&0));
    plan::inventory_host_proof();
    // Exhaustion is last, in this disposable host process; the real atomic never resets.
    let previous = NEXT_IDENTITY.swap(u64::MAX - 1, Ordering::Relaxed);
    assert!(previous > *unique.last().unwrap());
    assert_eq!(next_identity().unwrap(), u64::MAX - 1);
    assert!(next_identity().is_err() && next_identity().is_err());
    assert_eq!(NEXT_IDENTITY.load(Ordering::Relaxed), u64::MAX);
    let maps = std::fs::read_to_string("/proc/self/maps").unwrap();
    assert!(!maps.contains("libcuda.so") && !maps.contains("libnvidia-ml.so"));
    format!("{{\"passed\":true,\"concurrent_identities\":{},\"identity_exhaustion_refused_without_wrap\":true,\"inactive_inventory_invalidated\":true,\"device_epochs_isolated\":true,\"concurrent_inventory_transitions\":512,\"ignored_retirement_overflow_latched\":true,\"intent_overflow_latched\":true,\"inherited_registry_rejected\":true,\"cuda_loaded\":false,\"nvml_loaded\":false,\"gpu_qualified\":false}}", ids.len())
}
