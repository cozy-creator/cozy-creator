# Shared native service host evidence

Branch: codex/xs138-p3-shared-service. Parent native065 is preserved.

Final source/binding check passes (6.87 seconds, one nice19 Cargo job).
`host-final.json` records exact source/harness hashes and the actual concurrency result.
The host harness uses real native controls, threads, channels, capture counters and
tmpfs ledger participants, with no CUDA/NVML library loaded and no fake CUDA Peer.
It records both client orders,256 admission/last-close races, unfinished-request
abandonment, unknown-client activity, retryable final close and a text-only permanent
failure tombstone after every Control reference is gone. GPU qualification remains open.

The initial build type-inference error and non-tmpfs fixture refusal are preserved.
The final two intentional native-thread panics exercise poisoned control state; they
are expected assertions, not suppressed runtime/GPU failures. Raw intermediate proofs
and output-only exact-source compilations remain under outputs/xs138-p3-shared-service.
