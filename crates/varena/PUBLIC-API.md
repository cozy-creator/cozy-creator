# Varena external API, ABI 1 development checkpoint

This is the **unreleased P4 interface**, composed onto the coordinator's native
retirement/async-fill/shared-service cohort. See
[native-client-contracts.md](native-client-contracts.md) for the implemented native
seams and the [P4 tracker](https://github.com/cozy-creator/tracker/blob/master/tracker/cross-cutting/xs-138-p4-public-api.md)
for exact source, host and GPU boundaries. No merge or release is authorized.

The exported contract is [include/varena.h](include/varena.h): `vr_v1_*` symbols,
fixed-width status codes, non-reused integer handles and fixed v1 layouts with checked
output sizes. Failed constructors zero their output and retain caller ownership.
ABI 1 layouts do not grow; incompatible changes require new versioned symbols/layouts.
The draft in design Appendix A was not a previously shipped ABI.

| Concept | Implemented interface | Current boundary |
| --- | --- | --- |
| GPU | open/close, stats, trim; repeated opens share a public arena and the process working allocator | Linux shared-library packaging; primary CUDA context only |
| weight Region | copied immutable host bytes or C Source callback, stable VA, retained Uses | Full-region fills; current core joins completion before publishing; no asynchronous fill claim |
| working allocation | alloc, pointer, record_stream, free | Eager inference; allocation stream retained; side streams must be recorded |
| rebuildable Cache | reserve, exact page commit/release, poll/info, asynchronous owner shrink, close | Owner selects disposable pages; live KV is never reclaimed merely because it is a Cache |
| execution Plan | copied phase descriptors retaining Regions and Sources, scoped measured eager passes | One arena per GPU, one active phase; no automatic repeat/prefetch scheduling |

The Rust frontend is `crates/varena-client` (use dependency alias `varena` if desired).
It links only the C ABI; it does not embed a Rust allocator. `Gpu`, `Region`, `Use`,
`Allocation`, `Cache` and `Plan` retain native owners. `Pass::complete()` publishes a
successful pass; dropping a pass aborts it. Importing a foreign stream is explicitly
unsafe because its context and lifetime cannot be verified by Rust's type system.
Calling foreign kernels on integer device addresses also remains the caller's unsafe
integration boundary. Ordinary host-region and ownership operations are safe Rust.

Public Python callers use `tensorfs.varena`. Runtime-only integration interfaces
are being moved to a private module; they are not a second supported public API.
Low-level use/allocation/cache release requires the actual consumer stream. There
is no Rust `default_stream` convenience; unsafe `borrow_stream` retains the caller's
explicit stream identity and requires its owner to survive deferred uses.

```python
import torch
import tensorfs.varena as va
import tensorfs.varena.torch as vt

vt.install(0)  # before Torch initializes its CUDA allocator
work = torch.cuda.Stream(device=0)
with torch.cuda.stream(work):
    with va.open(0) as gpu, gpu.region(b"immutable bytes") as weight:
        with gpu.plan({"decode": [weight]}) as plan:
            with plan.phase("decode", "u8/15/my-kernel/eager"):
                with gpu.use([weight], stream=work.cuda_stream) as used:
                    with gpu.alloc(4096, stream=work.cuda_stream) as scratch:
                        # Submit your checked CUDA kernel on this same stream,
                        # using used.ptr() and scratch.ptr.
                        pass
```

Additional consumers must declare their actual streams or be joined into the
recorded consumer stream before releasing a Use or allocation. Stream completion
is distinct from Python scope exit. The standalone C/Rust examples create owned
nonblocking streams and prove their own completion with events.

## Ownership, streams and errors

Closing a GPU releases its handle; children retain device ownership. A Region pointer
is only a reserved address. A Use pins its backing and Source until `done`; closing the
Region first does not invalidate the Use. Plans retain Sources as well as reservations.
Sources may be called again after eviction. C callbacks must initialize every logical
byte, retain staging through completion even on failure, and never unwind or recursively
use their own Region. The ABI takes Source ownership only after construction succeeds.
Final Arena resources are handed to the native per-device retirement queue; the ABI
worker polls a host ticket and never synchronizes the CUDA context. Genuine owners
stay retained; unknown completion is quarantined. Public asynchronous Source
adoption remains separate from the implemented native fill/Source mechanisms. Checked closes retain the same handle
on refusal. Rust `Use::done`, `Allocation::free` and `Pass::complete` borrow `&mut self`
so errors remain retryable. Rust/Python finalizers transfer handles to the native
retirement worker; they do not repeat a refused close and forget its owner.

The worker retries capture/busy and already submitted completion fences. Live Cache
pages with no release proof and permanent faults enter observable quarantine;
`retirement_stats()` reports queued/in-flight jobs, completed jobs, quarantine and
abandoned live Cache bytes. These job counts are not physical-reclamation credits.
An abandoned Rust Pass whose abort is refused transfers its entire Plan for retirement.
Use an explicit complete/abort retry if the caller needs to retain that Plan.

Final Arena teardown keeps retirement exclusion through destruction, without holding
the capture mutex across context synchronization. The complete Arena stays owned on
failure. Persistent observer references held by Peer and the callback worker still
require the coordinator's pending native retirement interface; P4 neither forces
reference counts nor stops a surviving shared service to bypass that requirement.

Streams must belong to the GPU's primary context and remain live through every retained
Use/allocation. Stream 0 means the legacy default stream, not an arbitrary current Torch
stream. Public operations reject observed capture/replay; direct raw CUDA capture outside
Varena's registered graph integration is unsupported. No raw pointer or escaped tensor
may outlive its owning Use/allocation/cache pages. GPU mutation of immutable weights,
training, arbitrary escaped storage aliases, reset, fork and private contexts are unsupported.
Borrowed streams must survive deferred retirement too. Use checked close/retry or wait
for retirement to drain before destroying a foreign stream. This is part of the unsafe
Rust stream-import and raw C/Python stream contracts.

Working frees retain their original stream and record every declared side consumer.
Free does not mean physical release has completed. Cache releases require the owner to
join all readers/writers into the supplied stream; commit is all-or-nothing over entirely
unmapped, page-aligned ranges. Release/poll/recommit supports interior holes. A shrink
callback runs on the native worker without allocator/ledger locks; returning Busy/false
declines. Requested bytes and callbacks returning do not count as reclaimed bytes.
Close first excludes/drains callbacks, then refuses live or pending pages, keeping the
handle retryable. Do not close from inside your own callback. Python calls release
the GIL while draining; the native weak adapter acquires it on the callback worker. Use explicit close
before interpreter shutdown; closures should not strongly capture their own Cache.
Python now uses the existing weak PyO3 callback adapter, attached to the same native ABI
Cache owner; no raw Python CFUNCTYPE callback survives interpreter shutdown. Both checked
and deferred close stop admission and drain that owner. Only its already-running callback
may use the release/read borrow during queued retirement; new commits remain rejected.

Statuses preserve `Exhausted` (requested bytes available), `Capturing`, `SourceError`,
`Fault`, `Invalid`, `Unsupported` and `Busy`. Python has matching exception classes;
the separate Torch allocator retains its Torch OOM translation. CUDA faults require
process retirement; clean source failures do not become OOM. C last-error text is
thread-local, unchanged by successful calls, and valid until the next failure on that
thread. Rust copies it immediately. Panics are caught at the C boundary; a reported
panic requires retiring the process. None of these APIs retries application code.

Stats are snapshots, not reservations. `working_mapped` includes graph pool backing;
`pools` is its diagnostic subset. `caches` and `weights_resident` are separate physical
classes. `working_capacity` includes allocation rounding; requested `working_live` is
not the same number. Counters include existing TensorFS/Torch users of this core.

## One native core and building

`varena` now emits a **dylib only**, never an rlib. TensorFS's internal Rust adapters
link that library; C and the safe Rust frontend call its exported ABI. Python resolves
the symbols through `_ext`'s dependency and checks its core identity against the native
extension. A native client must link the SDK/wheel's `libvarena.so`, not ship its own
allocator copy. Native-first and Python-first load orders are covered by the mixed client.
The public C ABI does not depend on a Rust compiler. The internal Rust link used by
TensorFS is built as one cohort with its exact dependency and matching `libstd`.

Before extension import, Python compares the bundled and loaded native image identities
(ELF build ID, Mach-O UUID or PE CodeView identity). It retains a loader reference and
validates the canonical object again after acquisition, including concurrent package/native
loads. An incompatible private dependency is refused before importing `_ext`; a second
allocator is not a recovery path. This is local private Rust binary compatibility, not
Runtime/Cozy commit lockstep. An older compiled C client can still bind a compatible C ABI.

Do not build the core separately as a primary Cargo target and then package it with an
existing extension: Cargo selects a different Rust dependency-linkage mode. The bundle
uses `target/<profile>/deps/libvarena.so`, the actual extension dependency. Rust internals
are not a stable third-party ABI. Source/build changes require rebuilding that internal
cohort; public external clients need only the versioned C contract.

Run the host proof from the Creator worktree (it performs no CUDA initialization):

```sh
bash release-gate/suites/varena/public-api/prove-host.sh .task/p4-host-proof
```

It builds the full TensorFS extension and CLI, native C engine, C++ header client,
safe Rust example and mixed native library. It creates a relocatable SDK with Python
sources, extension, core, Rust runtime and header. The proof executes both binding load
orders, checks one mapped core, shared error state, per-thread error isolation and the
native request sequence. It also executes the C/Rust host paths and `tfs version`.
This is real host execution, **not allocation, CUDA or model qualification**.

For a production-shaped wheel, use `crates/tensorfs-py/scripts/build-wheel.sh`
and `--torch-python` when a Torch shim is needed. The final bundle preserves the
`libvarena.so` SONAME instead of an auditwheel hash rename. It installs shared libraries
at `<prefix>/lib/varena-v1` and the header at `<prefix>/include/varena/v1/varena.h`;
both native `tfs` and Python load those files. No compiler/toolchain is required after
installation. macOS/Windows packaging, ABI cohorts and GPU execution remain unqualified.

Native dependencies are now included by build.rs/maturin, without post-build core
replacement. PEP 517 builds stage the CLI automatically. Direct maturin builds use the
existing CLI prebuild step (`python backend/stage_cli.py <matching build options>`);
the CI paths now build the complete cohort and audit the completed Linux wheel's floor.
Target/profile/features, including equals-form options and `-F`, are forwarded to the
CLI prebuild. Windows source builders need file-symlink capability; the unsafe stale
hard-link fallback is rejected. Windows/macOS execution qualification remains separate.

`scripts/bundle-native.py` packages native cozy/TensorD archives with immutable private
library names and final dependency receipts before signing. Installers, native release
extraction and machine update staging carry those libraries before replacing the binary.
Old library files remain for running owners and rollback. A raw installed private-linked
binary is automatically packed with its verified adjacent dependencies for a dev update.
No live installation/update or publication is performed by the P4 qualification scripts.

Rollout must first deliver the library-aware updater changes to an existing static cozy
cohort, or use the updated installer. Older deployed updaters extract only the executable
and will refuse a new dynamic binary before replacement; this source change cannot add
dependency support to an already-running older updater. The updater patch is kept separate
for coordinator integration. No Runtime version or source SHA equality is required.

## Runnable GPU examples

With the SDK produced above:

```sh
export PYTHONPATH="$PWD/.task/p4-host-proof/sdk/python"
export VARENA_LIB_DIR="$PYTHONPATH/tensorfs/.native"
export LD_LIBRARY_PATH="$VARENA_LIB_DIR"
.task/p4-host-proof/native-engine
target/debug/examples/engine
python crates/varena/examples/python_client.py
python crates/varena/examples/cache_application.py
```

The C native engine loads a PTX kernel without Python/CUDA headers, doubles four floats,
checks exact output and repeats after reclaim/refill. The Rust example exercises scoped
Plan/Region/working ownership. Python reads exact bytes after refill. The cache application
checks all bytes of three pages around an interior release/recommit, then explicitly
requests and observes a declined shrink. That explicit request does not claim
allocator-triggered recovery.

The GPU owner additionally runs `public-api/mixed.py --gpu` in **both load orders**
with a matching installed Torch shim. It passes a Python-created GPU handle to C,
observes C allocations through native/Torch accounting, executes Torch arithmetic,
allocates through safe Rust, then checks quiescent accounting after all frees and trim.
Run it through the coordinator's ordinary Cozy release-gate lane. Do not use the host
proof as permission to use a machine or rent a GPU.

## Remaining design integration

General `tensorfs.varena.torch.offload(pipe)` is **planned, not implemented**. The three-line
Diffusers snippet in the design is not a runnable supported example yet. In particular,
tied Parameters, direct reads, escaped aliases and dtype rebinding need the coordinating
adapter and real model proof. The existing `vt.install`/graph adapter is separate from
this eager external API and requires its already-documented Torch/CUDA build cohort.

The draft's automatic ledger join/configure, region/cache priority, advisory `room`,
public split/pool ownership and CBOR phase records are not exported as pretend-working
ABI 1 calls. Native mechanisms continue to evolve in P1–P3. The coordinator must supply
a shared get-or-open peer-service lifetime for public automatic join/configure (today's
Owner::open refuses an already-owned service), a multi-arena Plan seam if required, and
an asynchronous Source publication seam before those public contracts can be widened.
No allocator policy, ledger behavior or graph reclamation implementation was changed here.

P4 qualification remains open: mixed native/Torch CUDA accounting/reclaim, native and
Python examples, source-failure/fence/capture cases, Cache pressure/GIL/decline behavior,
8 GB Diffusers once offload exists, and full H3 on 2/4 GPUs. No merge/release or GPU claim
is authorized by this interface checkpoint.
