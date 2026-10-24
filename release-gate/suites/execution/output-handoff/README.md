# GPU / CPU output handoff

This real release-gate package performs CUDA GEMM, produces actual raw RGB pixels, and uses the
ordinary Runtime/TensorD lossless WebP output path. A second request of the same entrypoint should
reuse the executor and start GPU work while the first request is still processing its host output.
No checkpoint is downloaded; this is lifecycle/overlap proof, not model-quality qualification.

Use the default Cozy home and an explicitly selected candidate machine cohort. The controller
uses only ordinary CLI run/cancel commands and records every run. It never updates the SDK.

```bash
python3 gate.py --out /path/to/new/proof-directory --expect overlap
python3 gate.py --out /path/to/new/cancel-directory --cancel-output
python3 gate.py --out /path/to/new/custody-directory --cancel-custody
python3 burst.py --out /path/to/new/burst-directory --cpu-capacity 2
```

Use `--on <name>` for an already rented candidate machine. The controllers never rent or update it.
Remote overlap and cancellation use machine-side phase timestamps, avoiding comparisons between
the laptop clock and the GPU host clock. The uninstrumented `--expect serial` baseline is local-only.

The custody cancellation case produces a real ordinary file (240 MiB by default, bounded by
`--file-mib`). It cancels after encoding finishes and must observe accepted cancellation before
the machine's output-finished transition, while the next GPU call remains active and succeeds.
It exercises actual copy/hash/fsync work with no sleeps or injected barriers. If the output settles
before cancellation arrives, the case fails as unexercised; it cannot claim the race was covered.

Run through the shared heavy wrapper. `--expect serial` records the old behavior as a baseline.
Each case first warms the same entrypoint, then submits the image request and its queued successor.
The default image is 4096x4096 random RGB, at most 48 MiB raw; the extra cancellation case does
more real GEMM to keep the successor active while the preceding output is canceled.

Acceptance: correct GPU result, committed image (ordinary case), same executor PID, first GPU work
complete before the successor starts, and successor GPU work before the first request settles.
Cancellation must not stop the reused executor's successor. Candidate runs additionally require real
output_encoding begin/end log events overlapping the successor's GPU interval; the canceled-output
case must deliver cancellation before the successor's GPU work finishes. The observation deadline
only stops the proof and requests cancellation of its own
runs; it never authorizes killing the daemon, an executor, or somebody else's work.

No successful candidate run is claimed until a receipt exists against the changed TensorD binary.

The burst case requires the service's actual CPU parallelism and an idle qualification window.
It checks that detached output work never exceeds that capacity plus the one active GPU slot,
that enough backlog was generated to exercise the policy, that every image commits, and that one
executor is reused without overlapping independent GPU invocations. It queues at most 16 real
images and cancels only its own runs if the proof cannot finish.
