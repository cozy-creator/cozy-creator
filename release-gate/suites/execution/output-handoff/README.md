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
python3 derive.py --out /path/to/new/derived-checkpoints
```

Use `--on <name>` for an already rented candidate machine. The controllers never rent or update it.
Remote overlap and cancellation use machine-side phase timestamps, avoiding comparisons between
the laptop clock and the GPU host clock. `--expect baseline` records the GPU handoff gap without
classifying encoding overlap: controller settlement is a later delivery event even on the same host.
The gate also requires the persisted `started_at_ms` after executor return. The current public CLI
does not expose the raw outcome `execution_started` bit, so this observer does not assert that bit.

The custody cancellation case produces a real ordinary file (240 MiB by default, at most
480 MiB with `--file-mib`, below the installed aggregate output limit). It cancels after encoding finishes and must observe accepted cancellation before
the machine's output-finished transition, while the next GPU call remains active and succeeds.
It exercises actual copy/hash/fsync work with no sleeps or injected barriers. If the output settles
before cancellation arrives, the case fails as unexercised; it cannot claim the race was covered.
For remote machines, `--cancel-after SECONDS` schedules the external cancellation after queuing
the successor, using measured workload timing instead of waiting for a delayed network log.
The final machine timestamps must still prove cancellation landed inside the requested interval.
This delay changes only the controller, never the GPU workload, encoder or custody path.

Use a Python that has Pillow (for downloaded pixel parity); remote orchestration may run at
nice 19 without holding a local compilation slot.
Each case first warms the same entrypoint, then submits the image request and its queued successor.
The default image is 4096x4096 random RGB, at most 48 MiB raw. The encoding-cancellation case
produces four actual images (192 MiB total) and does more real GEMM to keep the successor active
while the preceding output is canceled. The successor is queued once the first request is accepted.

Acceptance: correct GPU result, a downloaded image whose decoded pixels match the GPU source
(ordinary case), same executor PID, and first GPU work complete before the successor starts.
Cancellation must not stop the reused executor's successor. Candidate runs require real
output_encoding begin/end log events overlapping the successor's GPU interval; the canceled-output
case must deliver cancellation before the successor's GPU work finishes. The observation deadline
only stops the proof and requests cancellation of its own
runs; it never authorizes killing the daemon, an executor, or somebody else's work.

No successful candidate run is claimed until a receipt exists against the changed TensorD binary.

Installed Creator377 / TensorFS377 / Runtime272 qualification on dororo passed ordinary overlap
(17112–17114), cancellation during four-image encoding (17143–17145 with `--cancel-after 1.5`),
and capacity-one backlog (17165–17169, peak detached outputs exactly two). Image pixels were
verified against their GPU source. The 480 MiB custody-only cancellation attempts landed outside
the roughly 440 ms custody window and remain unqualified; those failures are preserved.
Receipts live under `outputs/varena-remote-20261024/cohort-2` in the workspace root.

After updating only TensorD to d8063c123 (removing redundant pre-return output flush/copy),
the same installed SDK passed overlap (17182/17184/17185), encoding cancellation
(17188–17190), and capacity-one backlog (17193/17195–17198). The observed overlap was
188.02 ms for one image and 3.45 s during the four-image cancellation case. Backlog again
peaked at exactly two and every image was verified. These are lifecycle checks, not a
performance comparison: the initial warm receipt was delayed by a shared controller restart.
Two further custody attempts were unexercised (17200 canceled before handoff; 17203 committed
before cancellation arrived). Their successors completed. Follow-up receipts are in
`cohort-2/d806-followup`; detached-output service restart is also not qualified.

The burst case requires the service's actual CPU parallelism and an idle qualification window.
It checks that detached output work never exceeds that capacity plus the one active GPU slot,
that enough backlog was generated to exercise the policy, that every image commits, and that one
executor is reused without overlapping independent GPU invocations. It queues at most 16 real
images and cancels only its own runs if the proof cannot finish.

The CPU derivation case uses declared weights outputs, Context.output and adoption through the
installed Runtime/TensorD pair. It checks actual committed tensor bytes, a later request's zero-write
replay, and a distinct recipe's separate repository. It does not claim process-restart replay or
GPU prepared-artifact performance. This covers the TensorFS release/lane API integration needed by
the combined candidate; no private SDK invocation or alternate store is used.

`cozy run ./gpu_parent 'payload:={"value":7}' --gpus=1 --on <machine>` exercises a GPU
parent awaiting a GPU child in a separate executor while its own CUDA tensor stays live.
Both execute real CUDA arithmetic; the parent verifies its value after the child returns.
This covers family admission without memory lending or replaying the parent.

`cozy run ./native_loop steps=30000 --gpus=1 --on <machine>` uses the installed executor's
real Context.plan directly. One host graph launch must perform every device increment while
the application thread parks, with exactly two Python body calls (warmup and capture) and exact
output. This complements the isolated candidate-child lifecycle/primitive cells.

## Hard-cancel cohort edda/f0d/9e95

Actual local RTX4070 Laptop qualification passed ordinary overlap17829–17830, cancellation
inside encoding17832–17833, and cancellation inside real480MiB custody17835–17836. Every
successor completed correctGPU work in the same executorPID; canceled attempts published no
result/output. The ordinary image's decoded pixels matched its GPU-source hash. The exact
selected timestamp-window assertions were used, with no retries or injected delays.
[Proof and immutable receipts](evidence/edda-f0d-9e95-local-lifecycle/PROOF.md).
Normal restore17837 returned the healthy4178/c1/e521 cohort and observed zeroactive work.
These are lifecycle gates, not performance comparisons or output-service-restart proof.
