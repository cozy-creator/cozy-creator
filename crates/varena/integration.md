# Varena v2 native integration

Owner: `/root`, xs-138. Branch `codex/xs138-native-integrated` starts from fetched
`origin/master` e28d759ce0440a5e0aa7c4f98620699b5029e5c5.

This candidate combines three separately exercised dependencies:

- physical shortage and native peer service: c3bf579ae82b42b254aa30fe54c609fc8e89afcb;
- phase-region Plan: 4dcf26183fafe6bd160a52856afd057eae43d184;
- normal-process guard: bacdb9db721dd4ceda3ffe279d1285bbea556708.

Integration preserves shortage classification and the finite recovery budget. The
Plan reader now filters the device-tagged Arena registry before upgrading weak
references; every acquired owner still predates and outlives capture/working
guards. Arena publication retains Plan generation invalidation. Linux libc stays
required by the ledger even when the optional kernel guard is disabled.

At source 3efea023dfde83b44d5a15bed6deb19c4ac96fc1, the full TensorFS Python library
passes single-job locked `cargo check` in 23.81 seconds. Default, strict-PID and
feature-off real host-process probes all pass. The default probe still records the
unsupported shared-VM/copied-TLS child acceptance; integration does not expand the
documented `one_process/1` contract.

These are source and CPU checks. Earlier GPU receipts qualify their named frozen
dependencies, not this combined binary. Coherent allocator, Plan, recovery, capture
and model GPU checks remain required. Cache and foreign-pressure work is still on
separate owned branches; this is neither P3 completion nor a supported P4 ABI.

CPU receipts are under
`release-gate/suites/varena/evidence/native-integrated-cpu/`.
