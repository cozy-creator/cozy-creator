# Actual raw TensorFS Source clients

The example uses the ordinary native Writer and Checkpoint APIs, then their Source with
real stored file bytes, an explicit inline recipe value, and uncovered padding. It runs
against the same shared Varena core as TensorFS. No model inference or quality claim follows
from these byte/lifetime cells. Root owns portable builds, GPU identity, installation and
rentals; this branch has not run a GPU mode.

```sh
cargo build --locked --offline -p tensorfs --example raw_source -j 1
raw_source host NEW_OUTPUT_DIRECTORY
# Separate child process and fresh output directory for each actual GPU mode:
raw_source ordering NEW_OUTPUT_DIRECTORY GPU-UUID 0
raw_source multi_fill NEW_OUTPUT_DIRECTORY GPU-UUID 0
raw_source cancel NEW_OUTPUT_DIRECTORY GPU-UUID 0
raw_source cancel_active NEW_OUTPUT_DIRECTORY GPU-UUID 0
raw_source prepared_drop NEW_OUTPUT_DIRECTORY GPU-UUID 0
raw_source partial_error NEW_OUTPUT_DIRECTORY GPU-UUID 0
raw_source capture NEW_OUTPUT_DIRECTORY GPU-UUID 0
```

Every directory must be new; previous checkpoints and failed receipts are preserved.
The host mode builds a real one-MiB checkpoint, proves complete file/inline/padding bytes
and overlap refusal before writes, and asserts libcuda was never loaded. It preserves the
existing joined host cancellation behavior: padding is initialized before piece-level
cancellation checks, while file/inline bytes remain untouched. That is not proof of the
new raw GPU cancellation contract. The first host attempt's incorrect zero-padding-progress
expectation is preserved in outputs, rather than changing the joined host implementation.

The seven GPU modes are authored and unrun:

- `ordering`: retain actual gate memory and fixed borrowed producer stream IDs, queue real
  waits before Source submission, and close all host submissions with exactly two staged
  pieces. Drop caller Source/Checkpoint handles before submission. Prove Published differs
  from completed/Ready; two separate consumers receive Varena event ordering and full bytes.
  A second consumer gate outlives Source resource retirement. An independent Arena completes
  useful copies while producers and consumers are held. Final physical accounting is checked.
- `multi_fill`: one Source operation fills Regions in two Arenas on one GPU. Both complete
  byte readbacks and exact counters must pass. All native Fill retirement tickets must finish
  before a real subsequent CUDA capture succeeds. This exercises shared resource claims.
- `cancel`: cancel before submission; require no publication and zero successfully enqueued
  bytes, then eventual native cleanup without a context join.
- `cancel_active`: one scoped submitter reads three real file pieces with a two-slot ring.
  Actual gated DMA retains the third piece at ring reuse. The main thread waits for two
  proven file enqueues, requests cancellation, then opens the real gate so the host submitter
  can close. The first two initialized ranges remain exact through an escaped alias; no
  Region publishes. This is distinct from the <=2-piece publication case.
- `prepared_drop`: drop an unstarted operation; native no-write cleanup must finish without
  leaving permanent Pending or physical backing.
- `partial_error`: truncate only this invocation's newly written checkpoint chunk after its
  verified descriptor is owned. A single reader really enqueues the first file range before
  the second reaches EOF. No partial Region publishes; an escaped Pin preserves the first
  bytes. Restore the original fixture chunk and refill at the same VA, with no context join.
- `capture`: actual CUDA capture refuses preparation before engine initialization, returning
  the original Fill owners. Retry those same inputs after capture and verify complete bytes.

Producer identities are read-only native diagnostics. They grant no ownership or permission
to extend Source submission. The fixture uses them only before submission and retains gate
resources until all work finishes; it does not add a product gate method or mutate the
producer list. Consumers use CUDA-visible Varena event dependencies. The gate operations
follow the current [NVIDIA stream-memory API](https://docs.nvidia.com/cuda/cuda-driver-api/cuda_driver_api/group__CUDA__MEMOP.html);
no CUDA13.4 driver/toolkit execution is claimed from documentation review.

The example's ten-second checks are explicit fixture protocol deadlines. The operator must
also bound the whole child process because an actual driver failure can prevent CUDA calls
or a scoped GPU-dependent submitter from returning. A failed process is not a cleanup pass.

Source facts distinguish requested logical bytes, successful device-enqueue bytes (including
padding), host submission time, observed producer completion and resource retirement. Each
operation counts its own enqueues even when several operations share a cancellation token.
Zero enqueues never certify no work after an ambiguous driver error. Completed mixed raw
I/O uses a `raw` metric with actual issued bytes and CUDA event busy time; it does not train
H2D ceilings from memset bytes or present submission time as completion. Failed partial
reads are not credited as complete logical reads.

Remaining outside this fixture: full models/performance, the Python adapter under an actively
held GIL, cross-process Source behavior, multiple devices, encoded/continuation/host-export
adoption, and external P4 ABI adoption. Preserve the six previous async Fill fixtures; this
suite supplements them without changing their source or qualification boundary.
