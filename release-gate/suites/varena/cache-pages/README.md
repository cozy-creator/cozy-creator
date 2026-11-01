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
