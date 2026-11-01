# Eager prefetch is structurally unqualified in this pair

Runtime `fe5a783da` and API-only control `1aca1d6cd` share
`src/cozy_runtime/internal/execution/phase.py:131`: `observing()` enters with
`vt.phase_enter(device, None, mode=mode)`. Actual adaptive choices accumulate in
`Observation.choice`; `Observation.complete` computes the final fingerprint only
after execution. The final key is passed to `phase_leave`, not installed as the
active key while the operation runs.

Native `9ccb36a24`, `crates/varena/src/working.rs:2143`, requires
`state.phases.samples.get(active.key.as_ref()?)` before reserving the measured
backing gap and admitting optional eager prefetch. Active None therefore declines
without IO regardless of whether a previous final fingerprint might happen to be
stable. `_prefetch_next` reaches `Region.prepare_prefetch`; it does not provide a
separately committed execution plan or key.

Control25112 recorded1,092 attempts/1,092 native declines/zero admissions, two
published owner scopes (first step and remaining loop) with distinct final keys
and successful_passes1 each. These counters fit the source contract, but the two
keys do not establish per-step instability or prove it caused every decline.
This path needs a genuinely committed eager plan to qualify prefetch. Supplying
the previous key for an unknown current configuration would weaken the contract;
restoring predicted Python policy is not a fix. No product source changed in this
audit. Candidate observations will be recorded independently when complete.
