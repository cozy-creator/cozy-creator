# Varena external API, ABI 1 development checkpoint

This is the **unreleased P4 interface**, built on Creator
`dfb912042f60fd99a0bbe404b9927665e9b39715` (#2405). Runtime inspection baseline:
`2626e3f48bc147f1f3d46b19bd385356e628680c` (#2014). Neither checkpoint is a universally
GPU-qualified release. [P4 tracker](https://github.com/cozy-creator/tracker/blob/master/tracker/cross-cutting/xs-138-p4-public-api.md).

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

Python's versioned surface is `tensorfs.varena.api`; `tensorfs.varena.open` and `Gpu`
are convenient re-exports. Existing `Arena`, `Region`, `Cache`, `Plan` and Torch/Runtime
adapters at the old module root retain their current signatures. Public `Gpu` methods
return the ABI classes. Both surfaces reach the **same core**.

```python
from tensorfs.varena import api

with api.open(0) as gpu, gpu.region(b"immutable bytes") as weight:
    with gpu.plan({"decode": [weight]}) as plan:
        with plan.phase("decode", "u8/15/my-kernel/eager"):
            with gpu.use([weight], stream=0) as used, gpu.alloc(4096, stream=0) as scratch:
                # Submit your checked CUDA kernel using used.ptr() and scratch.ptr.
                # Join side streams into stream 0 before the Use ends.
                pass
```

## Ownership, streams and errors

Closing a GPU releases its handle; children retain device ownership. A Region pointer
is only a reserved address. A Use pins its backing and Source until `done`; closing the
Region first does not invalidate the Use. Plans retain Sources as well as reservations.
Sources may be called again after eviction. C callbacks must initialize every logical
byte, retain staging through completion even on failure, and never unwind or recursively
use their own Region. The ABI takes Source ownership only after construction succeeds.
Final Region retirement uses the existing core's completion rules and can wait; fully
asynchronous final teardown is not claimed.

Streams must belong to the GPU's primary context and remain live through every retained
Use/allocation. Stream 0 means the legacy default stream, not an arbitrary current Torch
stream. Public operations reject observed capture/replay; direct raw CUDA capture outside
Varena's registered graph integration is unsupported. No raw pointer or escaped tensor
may outlive its owning Use/allocation/cache pages. GPU mutation of immutable weights,
training, arbitrary escaped storage aliases, reset, fork and private contexts are unsupported.

Working frees retain their original stream and record every declared side consumer.
Free does not mean physical release has completed. Cache releases require the owner to
join all readers/writers into the supplied stream; commit is all-or-nothing over entirely
unmapped, page-aligned ranges. Release/poll/recommit supports interior holes. A shrink
callback runs on the native worker without allocator/ledger locks; returning Busy/false
declines. Requested bytes and callbacks returning do not count as reclaimed bytes.
Close first excludes/drains callbacks, then refuses live or pending pages, keeping the
handle retryable. Do not close from inside your own callback. Python CDLL calls release
the GIL while draining; CFUNCTYPE acquires it on the callback worker. Use explicit close
before interpreter shutdown; closures should not strongly capture their own Cache.

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

For a production-shaped Linux wheel, use the existing `crates/tensorfs-py/scripts/build-wheel.sh`
and `--torch-python` when a Torch shim is needed. The final bundle preserves the
`libvarena.so` SONAME instead of an auditwheel hash rename. It installs shared libraries
at `<prefix>/lib/varena-v1` and the header at `<prefix>/include/varena/v1/varena.h`;
both native `tfs` and Python load those files. No compiler/toolchain is required after
installation. macOS/Windows packaging, ABI cohorts and GPU execution remain unqualified.

Release integration is still required: the `cozy` CLI/TensorD also depend transitively
on Varena and their archives now need the shared core/runtime. This task does not modify
the coordinator's release workflow. Existing CI/PEP 517 paths that call maturin directly
must adopt the same bundling before publishing; a raw maturin wheel is not the supported
P4 artifact. The documented SDK and bundled Linux wheel are the current install paths.

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
