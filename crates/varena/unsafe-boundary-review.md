# XS-138 unsafe boundary review

Initial source review base: `a0338b898060fcd16f8bb169a638afc89f08f080`. Reconciled master: `7a4a0407a095ff79c5344c8bffa3357a1355934f` (#2480); its six retired platform boundaries are removed. Existing master boundaries are preserved; retired Torch allocator/ELF/IAT boundaries are removed. This records source-contract review, not a claim of universal runtime or memory safety. Every changed native cohort still requires its applicable real-client and GPU gates.

The new Rust client and mixed-client fixture deny unsafe at crate roots; scoped allowances identify their existing ABI adapters. The guard remains unchanged, includes examples and archived Rust proof sources, and audits the shipped dependency graph.

New boundary contracts:

## `crates/tensorfs-py/src/varena/refill/raw.rs`

Unsubmitted cleanup calls abort_completed/into_completed_region only before this Work has launched a writer; prepare_raw transfers exclusive Fills and failure returns their real owners. Preserve the no-fallthrough cleanup and submitted-fills-retired distinction; never infer no work from issued_bytes==0.

## `crates/tensorfs-py/src/varena/tensor_export.rs`

DLPack repr(C) fields, capsule name and deleter must match the consumer. Checked dtype, rank, stride span, offset alignment and logical Region end bound the pointer. Held owns shape/stride vectors and TensorExport until exactly one final storage deleter. A consumed capsule must not call delete twice; forked destructors leak inherited native custody instead of touching inherited mutexes. Stream tracking and failed-tail quarantine are mandatory.

## `crates/tensorfs-py/src/varena/working.rs`

Allocator callback ABI receives a valid writable Failure from the native shim. catch_unwind contains allocation/destructor panics; fork rejection precedes inherited state access. Bindings must remain static and layout-identical to the shim. Preserve sticky callback errors and process custody; do not move allocator policy into this bridge.

## `crates/tensorfs/examples/raw_source.rs`

Three unsafe host-read calls have live mutable Vec destinations and owned checkpoint/source inputs. Overlap and cancellation cases must retain refusal bytes; fixture never claims GPU proof from the host path.

## `crates/tensorfs/examples/raw_source/extra.rs`

Gated cancellation/multiple-Fill readers retain gate, stream, Region/Pin and output owners until checked readback or event completion. Completed-pin release is valid only after those readers finish. Capture failure must not produce a cleanup pass.

## `crates/tensorfs/examples/raw_source/gpu.rs`

Library retains exact CUDA function pointers/context; actual owned streams initialize gates and data. Driver pointers are bounded by real Varena allocations. Multi-stream pins remain fenced until each dependent stream completes; partial Source remains unreadable. Cleanup destroys only owned completed streams after resources_retired and submitted_fills_retired, preserving failure receipts.

## `crates/tensorfs/src/io/device/owned.rs`

Unsafe Send relies on retained Engine/context/handles and entering that primary context on every worker operation. submit accepts only exclusive retained GPU destinations and owned host/file inputs. Event queries require closed submission plus the complete recorded end-event set. Partial release/error retains remaining resources; Drop forgets unretired native owners.

## `crates/tensorfs/src/io/source/raw.rs`

prepare_raw requires exclusive Fills with no prior writer and every escaped view independently pinned. Native ReadPlan owns bytes and files; fixed retained producer streams cover all writes. publish/abort occurs only after host submission is closed. Shared Claim moves GPU owners outside mutexes. Cancellation/partial failure must never publish partial contents or retire before event completion.

## `crates/varena-client/examples/engine.rs`

Unsafe borrowed stream remains alive through checked Use/Allocation close and deferred native retirement. Copy verification retains both complete allocations, reads all expected bytes and checks completion before freeing.

## `crates/varena-client/examples/parts.rs`

Borrowed stream and all part/view storage remain live through each full copy/readback; mutation of input after snapshot checks immutability. Must retain all deferred retirement before stream close. New only in #2397.

## `crates/varena-client/examples/source.rs`

Actual native Source/SourceJob owns its true stream, CUDA library, mapped host gate and Gpu. Prepare submits no destination writes; submit closes; native completion precedes checked stream destruction. Gate/control/consumer resources remain alive until deferred Source retirement drains. Negative status is not a completion certificate.

## `crates/varena-client/examples/support/cuda.rs`

Library outlives all function pointers; owned primary context/stream/event persist through checked close. verify_copy is unsafe because caller certifies source/destination lengths and lifetime. Event synchronization proves this stream only. There is deliberately no unchecked CUDA Drop after a failed proof.

## `crates/varena-client/src/lib.rs`

C ABI ownership adapter: error CStr is copied while its native thread-local buffer is valid; repr(C) arrays/outputs live through synchronous calls. Borrowed Stream is an unsafe import with deferred-retirement lifetime obligations; Uses/Allocations hold it. FnMut shrink callback must be serialized and destroyed exactly once after callback quiescence; panic stays inside C. Missing crate-root deny(unsafe_code) violates Cargo.toml policy. Add root deny with narrow local allows at existing reviewed ABI helpers, owning impls, callbacks and ffi/source modules; no new file is needed.

## `crates/varena-client/src/source.rs`

Unsafe Source/SourceJob implementors guarantee stable target-context streams, closed submitters on return, complete initialization on success, retained sources/staging/code owners, and inert Drop after checked retire. Callback Box reconstruction is exact-once; provider callbacks need Send+Sync and jobs exclusive Send. Failed prepares remain in provider custody; panic faults/quarantines rather than invoking unchecked GPU Drop.

## `crates/varena/examples/async_fill.rs`

Actual CUDA context/streams/stages and independent reader Pins are retained through producer gates and full byte checks. Unsafe FillResource/FillOwners contracts are exercised by no-write, partial, pending, capture and retirement cases. Host callbacks/resources cannot be replaced with fabricated owners; on ambiguous submission native custody must remain.

## `crates/varena/examples/async_fill/host.rs`

Mirrored C HOST VMM structs/signatures require library/context lifetime; allocation fields are recorded immediately after each successful step. Granularity bounds ensure two MiB stage capacity. Release clears each field only after successful unmap/release/address-free, retaining remaining state on error.

## `crates/varena/examples/async_fill/retirement.rs`

Independent Arena initialization and consumer copying have real stream completion. Final Pin fence retains the consumer stream until Arena retirement ticket completes; observation must not accidentally retain an Arena and manufacture the result.

## `crates/varena/examples/fork_guard_cpu.rs`

Actual fork/raw-fork/clone paths own exact pipe FDs/child PIDs and exit without inherited Rust destructors. Parent waits/reaps only its created children. Shared-VM copied TLS scenario is deliberately a contract test, not product support. RLIMIT_AS restoration must precede allocation/formatting; no CUDA is initialized.

## `crates/varena/examples/ledger_peer.rs`

Actual forks exercise descriptor invalidation and numeric FD reuse with exact parent/child custody. dup2 targets fixture-selected descriptors; inherited Drop must not close replacements. Concurrent fork/open test depends on initialized atfork hooks and _exit, never inherited runtime teardown.

## `crates/varena/src/abi.rs`

Raw arrays/C strings/output pointers remain caller obligations after null/count checks: alignment, initialized readable memory, disjoint writable outputs and length lifetimes are not proven by null checks. Monotonic typed handles and process checks precede registry access. Shared library identity must remain unique. Use finalization fences its explicit stream; returned pins/weights survive partial failures. catch_unwind and sticky FAULT protect the FFI boundary. New host-parts outputs change only after all fallible steps succeed.

## `crates/varena/src/abi/host_parts.rs`

Every nonempty pointer must be readable and stable until snapshot returns. layout checks null/length, checked 256-byte alignment and isize bounds; fresh unique Arc allocation cannot overlap live inputs. All payload bytes and padding are initialized before assume_init. Do not turn raw descriptors into a safe public constructor without preserving this contract. Four inline unit tests conflict with local no-unit-test rule unless explicitly authorized.

## `crates/varena/src/abi/host_source.rs`

Source retains immutable Arc bytes, bounded HOST VMM stage and actual producer stream. Previous DMA completes before stage reuse; final copy stays asynchronous. Native Fill retirement proves completion before HOST unmap/event/stream release. Partial prepare and release failure retain exact remaining handles. Claim removes owners from the mutex before CUDA calls.

## `crates/varena/src/abi/source.rs`

Provider/Job unsafe Send/Sync is justified only by their foreign contract. Successful prepare transfers exactly one job before validation; malformed success is retained, failed prepare transfers nothing. Stream arrays must remain readable until copied. Submit must stop all host writers on normal return. BUSY retains custody; permanent error/panic quarantines; drop callbacks become inert only after checked retire.

## `crates/varena/src/abi/tensor_export.rs`

Export clones a native Pin before pointer exposure and orders the explicit consumer stream. Final foreign storage drop calls Consumers::release exactly once, keeps the canonical Weight/Gpu alive, and transfers failed-tail ownership to quarantine. Forked drops retain inherited owners. Native ticket/owner lifetime must outlast DLPack aliases.

## `crates/varena/src/async_fill.rs`

Unsafe API marks caller obligations, not arbitrary FFI. FillOwners owns fixed true producer streams/resources; into_async transfers exclusive Fill custody. begin is required before exposing a destination; publish means every byte initialized and all submitters closed; abort also closes all submitters. Each escaped/ordered reader retains an independent Pin. Completion precedes resource retirement and producer-exclusion release. Keep these requirements at the owning types.

## `crates/varena/src/cooperative/domain.rs`

geteuid has no pointers; directory is private and owner validated. NVML generated UUID structure/version and output handle live under the Nvml library instance; ordinal-to-UUID comparison must remain exact. This path must execute only in GPU owner startup, never CPU service discovery. The direct uid call may reuse ledger OS uid helper; no need to merge domain policy with ledger mappings.

## `crates/varena/src/fork_guard.rs`

Only initialized, read-only AtomicU32 mmap is published; normal fork gets private zeroed page by MADV_WIPEONFORK. A published mapping is process-lifetime; losing Prepared unmaps only its own unpublished allocation. Fallback pointer tag is never dereferenced. PID validation precedes TLS/registry access; copied authenticated TLS in shared-VM children is explicitly outside contract.

## `crates/varena/src/ledger/os.rs`

Registry lock pins descriptor Arc allocations through atfork; child invalidates and closes each inherited FD once without unlocking parent OFD locks. All raw methods verify original PID. Files are private regular tmpfs, reserve backing before fixed mmap, and hold publication/OFD locks during mutation. Mapping requires page-aligned AtomicU64 accesses and fixed lengths; final owner alone unmaps. First atfork hook must be installed before a racing fork; retain that initialization contract.

## `crates/varena/src/tensor_consumers.rs`

release requires the final storage alias gone, no future submission, all consumers joined to declared streams, and the matching Region Pin. Unregister address routing before release; interior pointers route only live families. Unknown/sticky tracking failure returns the exact Pin for quarantine. Independent exports retain independent Pins; no observer becomes a GPU owner.

## `crates/varena/src/working.rs`

free_completed requires exclusive eager allocation and proven completion of every producer/consumer. free_untracked requires final owner/no future work and retains pending work until native retirement. Neither accepts graph pool pointers with future replays. These unsafe preconditions belong at Working rather than in an unrelated FFI module.

## `release-gate/suites/varena/cache-pages/src/arena_retirement.rs`

Event/stream/graph signatures retain CUDA library/context. Completed eager frees and pin releases follow actual readback or event fences; observation drops exact owned Arena roots. Event failures must not be reported as retired ownership. Repeated capture loader is a consolidation candidate.

## `release-gate/suites/varena/cache-pages/src/main.rs`

One context and owned setup stream back fill/full-readback helpers. Exact allocated ranges bound memory calls; setup sync must not synchronize independent gated consumers. Close is attempted once after ambiguous errors; teardown does not certify failed work.

## `release-gate/suites/varena/cache-pages/src/multi_plan.rs`

Each fill/working/free/pin completion follows owned setup-stream completion/full readback. Multi-Arena intent does not certify another device. Raw capture begin/end/destroy repeats the same fixture lifecycle helper and should share a typed boundary.

## `release-gate/suites/varena/cache-pages/src/owner_retirement.rs`

Raw capture setup keeps stream/context owners alive; unproven capture end calls immediate exit, avoiding unsafe owner teardown. Successful end drops exclusion then destroys owned graph. Same capture boundary is duplicated in sibling fixtures.

## `release-gate/suites/varena/cache-pages/src/probes.rs`

Host callback references a stable Box<Gate> retained until stream completion, does no CUDA, and uses atomics. Stream Drop opens a held gate before synchronizing/destroying; failed sync must retain the Box. Pin/working releases require completed actual readers. Capture helper duplication is local to this suite.

## `release-gate/suites/varena/evidence/shared-python-owner/retirement-host-harness.rs`

Archived appended output harness still matches the guard scan and must be inventoried if tracked. Fork child checks rejection before inherited queue mutex, uses raw _exit, and parent waits its own child. It proves host queue control only; do not rewrite frozen evidence merely to lower count.

## `release-gate/suites/varena/multi-arena-plan-cpu/inventory.rs`

Output-only appended harness forks with real held registry mutex; child must reject before touching it and _exit; parent waits/reaps exact child. Synthetic metadata keys prove generation/host control only and never GPU ownership.

## `release-gate/suites/varena/native-owner-cpu/default_service.rs`

Real file/registry/thread fixture, with own fork child rejected before inherited registry. Synthetic device keys never reach CUDA discovery. It tests actual metadata custody, not fabricated native GPU objects; keep proof limit explicit.

## `release-gate/suites/varena/native-owner-cpu/lifecycle.rs`

Child inherits deliberately locked owner control and must fail before access; only exact owned child is waited/reaped. Host control observations do not prove CUDA lifetime cleanup.

## `release-gate/suites/varena/public-api/src/lib.rs`

Exported symbols must be unique and preserve exact C signatures. Borrowed fixture stream remains alive across checked free/done retry; RETRY holds all Gpu/Region/Use/Allocation owners. C entrypoints must not unwind through caller. Apply the documented crate-root deny/boundary discipline to this new FFI fixture crate as well.

The zero-span export correction and packing/source-unit-test cleanup are now source-integrated from draft #2481. Their host receipts remain tied to that lane, and GPU execution is still pending; frozen baseline receipts remain attached to their original hashes. The landed platform cleanup is preserved, including deleted shared-memory/dead memory APIs; consumers use only Signal, wait_any and Waitable.

## Platform signal dependency extraction

The existing Windows signal/event boundary moves byte-identically from
`crates/platform/src/memory/windows.rs` to `crates/platform-signal/src/windows.rs`.
The unsafe inventory substitutes that exact path; no raw FFI is added. The new
crate root denies unsafe, while the moved Windows boundary retains its existing
local allowance and SAFETY notes. Unix socket/poll code is also moved unchanged.
`platform::memory` re-exports the shared implementation and preserves error
operation/errno conversions for active callers. Varena now depends on this OS-only
crate, preventing worker platform/fetch feature unification from moving installer
crypto into the shared native core. Signature verification and fetch behavior stay
in platform unchanged.
