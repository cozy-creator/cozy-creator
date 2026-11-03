// Appended only to the proof copy. This is the product's real host pressure state
// with an unavailable sampler. No invented NVML observations, GPU or Peer exists.
pub(super) fn proof_unobserved_pressure() -> Pressure {
    Pressure {
        nvml: None,
        uuid: String::new(),
        host_pids: false,
        previous: None,
        pending_processes: BTreeSet::new(),
        signal_deadline: None,
        requests: 0,
        window: None,
        last: None,
        largest_growth: None,
        facts: Facts::default(),
    }
}
