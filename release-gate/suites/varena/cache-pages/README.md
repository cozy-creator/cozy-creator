# Real native Cache page proof

This fixture exercises the native paged Cache primitive and thin Python binding.
The standalone Rust helper links the exact source Varena crate, loads neither Python
nor Torch, verifies the real GPU UUID through an actual Peer, and reads every sentinel
and filler byte. Python owns only normal Cozy orchestration and exact child cleanup.
The separate `python_binding` cell checks the public wrapper against the same extension
hash. It does not qualify an asynchronous owner callback or Runtime graph retirement.

- `pages`: three distinct sentinel pages, a middle hole released behind an actual
  submitted blocked CUDA host function, pending-only close refusal, refusal to recommit
  pending pages, and no early trim. A last owner dropped after release retains pending
  pages until the real event completes, then native pressure retires them. The hole is
  recommitted at the same base; both neighbors and the new middle are fully read back.
  Cache backing is separate from the measured native working phase peak. Invalid ranges,
  a real managed cap, and actual CUDA capture refusal are checked too.
- `physical_rollback`: a real live working filler is sized from observed driver free
  minus an explicit 128MiB margin and completely read back. A new Cache range requests
  at least the entire device capacity while that filler and old sentinel pages remain
  live. Each actual partial multi-page create failure must roll back every new page.
  Actual native driver counters record failed creates and successful map/rollback work;
  the exact old ranges, VA and sentinel bytes must remain intact. No synthetic OOM or
  reduced test scope is used. No automatic peer worker is installed in this helper.
- `abandoned_live`: a separate native process drops the last Cache owner without a
  release/completion contract. Its live backing must survive trim and a complete
  sentinel readback. It is deliberately retained until process exit. This is safety
  evidence for unproven ownership, not successful cleanup or reusable capacity.
- `python_binding`: ordinary `tensorfs.varena.Cache` commit/release/poll/recommit,
  direct full-byte CUDA readback, fixed base and exact cache-accounting cleanup.
- `split_choice` is a focused follow-up to the e545 primitive freeze: at a four-page
  managed ceiling, two live Cache pages permit only a two-page choice. They remain
  unavailable behind an actual blocked release fence. After observed completion,
  those still-mapped Ready pages permit the four-page choice; native begin must
  physically reclaim them before allocation, full readback and measured phase
  completion. The selector reports reclaimable Cache bytes separately from working
  slots. The helper emits the real before-selection comparison even on a failing
  pre-fix core; a smaller successful operation cannot satisfy the larger-choice gate.

`planned_split_choice` repeats that real live/pending/Ready sequence alongside one filled,
pinned Region in an actual active Plan, increasing the managed limit by exactly its page.
Every choice must preserve the declared phase weight; Ready Cache must permit the whole
choice under the phase-aware alternative as well. The selected generation reaches begin,
and the weight sentinel is fully read before checked cleanup. This checks the coherent
Plan/Cache merge separately from the earlier unplanned red/green receipts.

Native Capture tests cover only the product Working driver's counters. The fixture's
own CUDA capture/stream calls are explicit and outside that counter boundary. Error
ending actual capture terminates the isolated helper without GPU-owning Rust unwind.
The delayed-release callback performs no CUDA calls and its userdata remains owned until
actual stream completion, including error cleanup. Queued owner callbacks are not part
of this first primitive; no GIL/shrink-success claim follows from these cells.

`freeze.py` archives a clean exact commit. The build operator builds the helper and
TensorFS from that source using the persistent manylinux cache. `prepare.py` verifies
source/archive hashes, ELF64/x86_64, GLIBC <= 2.28, no Python/Torch linkage, and freezes
the project plus input. Supply the actual parent TensorFS extension SHA; the controller
checks that installed file before entry and reports its versions separately. The three
standalone Rust cells can use the current parent SDK because the child owns its new
native Cache core. Only `python_binding` requires the new extension. It must be run and
qualified separately after SDK installation. Inputs may remain GPU UNBOUND until rental.
All failed receipts and earlier artifacts must remain preserved.

Root owns native builds, SDK installation, GPU execution, rentals and settlement. The
fixture controller targets only its own unreaped process after PID/birth/pidfd checks;
its private ledger is a mode0700 directory on `/dev/shm`, never an assumed tmpfs `/tmp`.
Each call records actual post-child driver headroom without inferring release from PID
death or metadata alone. Native async owner progress, declined/busy shrink, and actual
Runtime graph retirement remain later, separate tests.

## Asynchronous owner follow-up

These cells use later immutable checkpoints and do not extend the earlier primitive
receipts retroactively. `native_owners` calls two real native Cache owners: the first
is busy, the second releases pages. Pending, Ready-Unjoined and Cold-Unjoined weight
owners remain untouched by the completed-only collector; actual event-wait and
context-join counters must stay unchanged until the test explicitly releases its
submitted CUDA host gate. `arena_concurrency` overlaps real Arena creation, fills,
full readbacks and destruction with native global reclaim/resident snapshots and Peer
publication. It checks the registry/capture lock correction without a product probe.

Five separate parent-extension cells require the new weak Python bridge and the exact
Runtime wheel. Preparation records the wheel hash, version and all eight participating
Runtime modules; it does not infer the loaded implementation from a helper build.

Common setup waits for the actual Runtime `int8_linear.native` readiness producer before
any controlled baseline/cap and records its before/after counters and library identity.
This prevents unrelated cold-start probe allocations from entering a protocol's measured
capacity; the16MiB release/resize and native limit formula are unchanged. The readiness
source modules are pinned along with the eight owner modules.

- `owner_allocator_progress` fills actual GPU capacity, retains a live Cache and asks
  Torch for another allocation. The native callback worker must run the real Python
  owner, collect physically released pages and permit a real allocator retry. Actual
  physical OOMs, complete output readback and zero peer-credit fields are required.
  Before/after/allocation-return snapshots are emitted before assertions. A peer episode
  may report exhausted while the independent local release permits the real final driver
  retry; that telemetry interleaving is recorded rather than required to have one name.
- `owner_held_gil` uses `ctypes.PyDLL` and the small `gil_resize.c` fixture to call the
  real `Storage.resize_` with the allocating caller's GIL held. It records the failed
  call and zero callback marker before returning to Python. Only afterward may the
  owner release its Cache and a separate allocation succeed. This is an explicit
  unavailable-owner refusal contract, not a retry of package execution or a same-call
  success claim. The initial managed-cap refusal must not become a driver OOM.
- `graph_owner_retirement` builds actual captured `PreparedRepeat`/`NativeSequence`
  objects and installs the idle sequence through the real Loop `_Program` owner.
  Its active target first declines. After completion, logical retirement must allow
  another three-stage NativeSequence to advance while the owner callback remains in
  capture-only exclusion. A real escaped view retains captured pool storage both
  during that other replay and after replay completion; only dropping that view may
  free the retired pool. A preceding real Cache owner records its actual release or
  refusal; ordinary NativeSequence ownership is not assumed to retain a global Varena
  replay exclusion. Owner facts, Cache state and native context are emitted for diagnosis.
  Complete outputs and final working/pool/Cache baseline are checked.
- `graph_owner_replay_exclusion` is a separate case: an explicit Varena replay scope covers
  the actual live A sequence through its checked completion. Here the first Cache owner's
  actual physical-release refusal is required, and it must not starve later logical B
  retirement. The ordinary sequence case does not inherit this stronger exclusion claim.
- `owner_capture_close` queues a real owner during an actual CUDA graph capture, observes
  the native capture gate and pending request, and closes admission with a typed refusal. It requires
  zero product working-driver calls in that interval and retirement only after capture
  ends. The untouched live Cache remains fully readable until an explicit owner release;
  callback retirement itself is not a release contract. The recorded graph also replays.

Build `gil_resize.c` from the frozen source with the pinned manylinux compiler, for
example `cc -shared -fPIC -O2 -Wall -Wextra -Werror -o gil_resize.so gil_resize.c`.
It deliberately leaves the CPython symbols unresolved for `PyDLL` to bind in the
running interpreter; it neither links Torch nor replaces its allocator. Record the
exact command, source and binary hashes. Pass that artifact and the actual candidate
wheel to `prepare.py --gil-gate ... --runtime-wheel ...` together. Both are optional
for standalone cells. Source/ELF checks and CPU construction do not qualify GPU
progress, capture races or graph-owner lifetimes.

## Current shared-core helpers

The current controller requires the exact two bundled native libraries from the installed
TensorFS wheel. `prepare.py --native-libraries DIR --async-binary PATH` copies those unchanged
libraries, sets helper-relative RPATHs and records every hash. Setup checks both bundled files
and the actual parent's mapped core/std against the manifest before launching a child. Earlier
static-helper receipts remain frozen; this source does not accept a missing core bundle.

The six `async_*` entrypoints run the genuine native async-fill modes through the ordinary
Cozy request path. Separate async input records bind the actual executable hash. GPU UUID,
exact owned process identity, complete native result and checked child exit are required.
There is no second native implementation or CUDA simulation in this controller.
