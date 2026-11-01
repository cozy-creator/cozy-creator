# Complete H3 sparse producer allocations

Close the actual single-GPU, baked INT8, BF16 H3 producer operation before its outer head split. The native menu owns full output and next-mean storage, reusable 4096-row packed QKV, exact packed/contiguous projection dispatch and workspace, chunk-local input quantization, the actual Kitchen workspace query, and all bootstrap/rolling statistics. Prepared rotary/fab tensors retain their existing identity and Request lifetime. Keep the selected producer, Kitchen norm/RoPE, sink-query/key ranges, tau and token-augmentation-zero arithmetic.

Torch reductions are not closed merely by supplying `out`: the pinned Torch 2.14 `Reduce.cuh` allocates global staging and semaphore storage. Reuse a suitable existing bounded reduction where available. The existing H3 key-sum implementation can be specialized for FP32 workspace sums without changing its old BF16 entries; the other existing max kernels reduce whole heads or transformed GEMM rows, not the producer's per-channel values. A small explicit-output column absmax kernel is needed unless a suitable existing entry is found. Qualify actual full and ragged 4096-row chunks against the Torch recipe; do not estimate hidden reduction bytes.

Initially use real Loop-carried mean slots. The prior is selected once, remains unchanged through admission and execution, and receives the private full next-mean result only after every admitted head window succeeds. Unsupported consumers retain their ordinary path before acceptance; accepted failures never replay through a legacy chooser. Bootstrap remains two producer passes; rolling retains the current-call V-scale prepass and one Q/K producer pass. No opportunistic switch to stale V scales or another attention recipe is permitted.

Use actual `sol_attn_plan` full/tail workspace results and actual `int8_plan` output pitches. Packed QKV has row pitch three times its head width; this rules out the contiguous-only GEMV path on ragged tails. A caller-owned V/window-output buffer may be reused only after all V reads complete on the same stream. An active native phase Plan must carry its selected generation unchanged.

This is an initial implementation scope. Source, bounded GPU and full-model qualification remain separate. The existing INT8/Plan cohort and its matched full-model pair stay frozen. Root owns every native/kernel build, GPU action, rental and final qualification artifact.


### Host phase correction after run25515

The first26-case GPU attempt stopped before the kernel because Loop supplies a Tensor
index and AttentionLayout requires integer host metadata. The fixture now declares
`phases=(0,)` or`(0,1)` and uses the real keyword host phase for routing and rolling.
It never converts a device Tensor to an integer, including under capture. The actual
Runtime `_Loop.eager` CPU red/green proof reproduces the old constructor error and
verifies both host phases with CUDA uninitialized. All26 cases, Runtime9b and packagefc4
are unchanged. Original25515 is preserved; GPU retry is pending.


Run25536 then exposed retirement of a capacity-probe key inside the active Loop phase.
Native correctly returns Busy while that phase's key is still unknown; the generic Busy
text mentions a Region even though this call only retires metadata. Each real probe now
belongs to the Request and retires after phase leave through normal Request cleanup.
The later same-PID check includes those keys. No native admission/retirement rule changed;
the failed receipt is preserved and the underlying operation remains unqualified.


## Qualified bounded successor cohort

Runs25622/25624 passed all26cases and same-PID cleanup on Sorata, Runtime167ffc828,
TensorFSa96, packagefc4 and fixturef366. The original trainable Parameters were retained;
the inference-export fix used zero-copy detached aliases. All18 declared allocation
censuses match, including1,442 observed detach operations with zero allocations.
There are16 whole/window cases,6 minimum refusals,2 real post-write refusals and2 full
Processor cases. All18 carried means are exact. Twelve of18 complete outputs are exact;
the maximum relativeL2 is0.000188590981 against the unchanged0.01 budget. Both post-write
refusals preserve prior means/host carried state and the completed first output head.
Both declared Plan Region lifetimes return to baseline.

Working live17,039,360B/mapped20,971,520B returns exactly, with pending/pools/events zero.
The later request in PID121758 confirms31 measurement keys already retired. Source/input,
raw-result hashes and independent checks are in evidence/gpu-20261101. This is bounded
consumer/ownership qualification; full-H3 numerical, perceptual and performance evidence
is still a separate gate. Original failures25515/25536/25575 remain preserved.
