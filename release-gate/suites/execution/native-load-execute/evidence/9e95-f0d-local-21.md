# Matched 21-cell GPU qualification

All21 cells passed on the local RTX4070 Laptop GPU (SM89, driver595.99.02), after ordinary
SDK update17794 installed Runtimef0d25c8e7/wheel24c95e67 and TensorFS9e95ee586/wheel8c5b6f9a.
The observed TensorD was **4178a2a11**, not the newer edda startup candidate.
[Exact artifact, fixture, run and receipt hashes](9e95-f0d-local-21.json).

| Actual runs | Verified behavior |
|---|---|
| 17796 | Ready-only pins: cold/loading misses unchanged; escaped aliases block reclaim; side-stream result720896 remains correct; same-address refill; zero final pins/resident bytes, no poison. Existing allocation recovery and graph tests also passed. |
| 17798 | 2,364 stored factors;60 ready reads with zero `_ensure`/`_assign` calls and zero source bytes. Actual GPU math, tied Parameter identity, BF16 conversion, eviction/refill, side streams and mutable-fill aliases passed. |
| 17800–17805 | NVFP4 and ConvRot each decode toF32/F16/BF16, then3 dependent GPU iterations from one host graph launch. Exact final tensors match independent format equations; only2 Python warmup/capture calls. Read→decode→launch ordering and alias cleanup passed. |
| 17806–17807 | Encoded pre-cancel and cancel-race settled with0 I/O/0 launches. The race was won before loading; this does not prove cancellation during an active decode. |
| 17808–17810 | Private truncated source returned IntegrityError with0 launches. Real NaN group scale read25,088bytes then returned DecodeError with0 decoded bytes/0 launches; healthy checked cleanup. Five constructor refusals cover plain/missing/dtype/unsupported resident source/scratch capacity. |
| 17811–17817 | All7 retained raw modes passed. Active cancel stopped after74 iterations; partial read issued4MiB before an I/O failure with0 launches; scope return joined2,000 iterations, and scope unwind canceled after1 iteration. |
| 17819 | Existing resident NVFP4 decoder passed96 format combinations and its typed-refusal/healthy-retry checks. |

For each positive encoded cell, NVFP4 read18,436 stored bytes and ConvRot read33,280; finalF32
storage was131,072bytes and F16/BF16 storage65,536bytes. Stored-byte counters are not physical
disk/PCIe traffic. Scratch facts count requested private bytes, not totalVRAM. These native
chains use checked host completion before launch, not an event-only handoff.

The factor-read profile measured84.953ms for the entire60-read/30-GPU-matmul section including
cProfile overhead. It is not a per-read latency, model benchmark or matched speedup.

The machine was ready with0 running/queued calls after the cells. The wrapper restored the
captured prior d859 Runtime/TensorFS0.15.6 pair. Root identified that old pair as incompatible;
the ready/idle restoration receipt is not a healthy-SDK compatibility certificate. Root's
known-healthy follow-up is separate.

No Windows/MPS/multirank, SVDQ calibration, ordinary NVFP4 recipe consumer, pressure/output
handoff, or edda startup acceptance is implied. Remaining lifecycle/platform gates still apply;
this evidence does not authorize merge or release.
