# Process-local CUDA weights

`tensorfs.varena` owns stable virtual addresses and reclaimable physical weight pages through
Rust. TensorFS remains the byte reader. A `Use.tensor` is an ordinary `torch.uint8` CUDA tensor;
its DLPack storage owns a native physical pin until its last alias dies. Runtime must fill all
logical bytes before `Use.commit()`. Commit joins the primary context before publishing ready
contents. An abandoned fill cannot be overwritten while an escaped tensor still pins it.

```python
from tensorfs import varena
varena.install_allocator()  # after importing Torch, before it reserves CUDA memory
arena = varena.Arena(0)
region = arena.reserve(byte_count)
use = region.prepare()
if use.needs_fill:
    checkpoint.components[component].tensors.read_into(key, out=use.tensor)
    use.commit()
# Keep use.tensor or a derived view alive for every kernel or graph that uses these bytes.
```

The compiled bridge ships inside `tensorfs._ext`; it has no Torch C++ dependency and does not
compile anything while serving. The Linux implementation changes verified `cudaMalloc` and
`cudaFree` data-import slots in loaded `libc10_cuda.so` and `libtorch_cuda.so`. The Windows x86_64
implementation uses the corresponding PE import tables in `c10_cuda.dll` and `torch_cuda.dll`.
It validates module layouts and original function pointers, temporarily enables writes on those
data pages, and restores their permissions. It does not change executable instructions.
Installation retains both original CUDA libraries and the hook extension for process lifetime.
It must precede concurrent CUDA work and other allocator/profiler hook installation.

Torch's native caching allocator still owns activation segments, splitting, caching, graph
pools and stream records. Every thread using the verified imports reaches the same native hook.
On a real CUDA allocation OOM, the hook reclaims inactive groups in its process/device, then
retries that one `cudaMalloc` call once if pages were released. Native errors return through
Torch's normal CUDA exception path. Direct native calls preserve CUDA codes on
`torch.AcceleratorError`; an unusable arena raises `PoisonedError`, whose `sticky` attribute is true, for executor
retirement, without inventing a driver error code. It never enters Python, acquires the GIL, reads bytes,
contacts another process, or reruns package operations. Reclamation may join the local primary
CUDA context; it has no bounded latency guarantee for opaque kernels.

`set_activation_limit(device, bytes)` optionally checks new hooked Torch segments plus current
resident weight pages. A deficit reclaims inactive weight groups before attempting the real
allocation. This is an **activation admission limit**, not a hard GPU/process cap: later weight
fills, CUDA contexts, and allocations outside the hooked imports can exceed it. Configure it
before Torch reserves memory. `allocator_stats()` reports actual hooked reservations, calls,
reclaims, refusals and retries; it is not a GPU ownership authority.

`capture(device)` forbids residency changes and allocation recovery across the entire process
on that device. Prepare and pin every dependency before capture/replay, and retain those pins
for graph lifetime. Raw Torch/CUDA captures outside Runtime must also enter this guard; the
streamless cudaMalloc hook does not infer arbitrary third-party captures. Reclamation synchronizes the primary context before unmapping, including
work on unknown side streams in that context. A `Use` whose consumer streams are declared with
`use.consume_on(stream.cuda_stream)` instead records a Varena event on each of them when it or
its tensor's last alias is released; reclaim then waits only on those events. Every stream that
touches the storage must be declared. `Arena.stats()` reports `context_joins` and `event_waits`.
Private foreign CUDA contexts and IPC are not supported.

Coverage excludes `cudaMallocAsync`, expandable segments, other CUDA libraries, static or
unknown Torch linkage, Torch modules loaded after installation, and runtime allocator changes.
Unsupported layouts/backends fail explicitly. Linux x86_64 Torch 2.14.1+cu130 is GPU-proven by
the stock-memory/native release-gate cell. Linux aarch64 and Windows x86_64 have implementation
only; the Windows hook has been cross-typechecked, not run on a Windows GPU. Neither is claimed
qualified until the same cell runs there. macOS and Windows ARM CUDA hooks are unsupported.

Implementation references: PyTorch documents that [`use_mem_pool`](https://docs.pytorch.org/docs/stable/cuda.html#torch.cuda.use_mem_pool)
routes only the current thread, so it cannot provide this process-wide allocation path.
The loader bindings follow [Linux dlopen](https://man7.org/linux/man-pages/man3/dlopen.3.html),
[Microsoft PE/COFF](https://learn.microsoft.com/en-us/windows/win32/debug/pe-format),
[VirtualQuery's memory layout](https://learn.microsoft.com/en-us/windows/win32/api/winnt/ns-winnt-memory_basic_information),
and [GetModuleHandleEx module pinning](https://learn.microsoft.com/en-us/windows/win32/api/libloaderapi/nf-libloaderapi-getmodulehandleexa).
No GPL allocator implementation or source was used.

## Owned native refill

`region.pin()` atomically borrows existing ready contents as an ordinary `Use`, or returns
`None` when contents are cold or still loading. It never allocates, maps, starts a fill, or
performs allocation recovery. The check and pin increment hold the same arena lock as reclaim:
either the pin wins and prevents unmapping, or reclaim wins and the caller observes a miss.
Poison, fork and ownership failures propagate rather than becoming misses. Rust's strict
`Region::pin()` retains its existing not-ready error and delegates to `try_pin()`.

The returned tensor and escaped aliases use the existing DLPack physical-pin owner. A pin
proves ready storage ownership, not completion of subsequent writes through another alias;
consumer readiness and write-stream ordering remain the caller's contract. Reclaim still
joins queued work in the primary context before unmapping after the last alias is released.

On supported Unix CUDA builds, `Region.prepare_read(checkpoint, parts)` admits the
region and initializes the existing TensorFS bounded staging rings and copy streams.
Each part is `(component/key#role, byte_offset, exact_stored_length)`. Offsets must
be nonoverlapping and inside the region; TensorFS fills every gap with zeroes.
The region belongs to this immutable recipe. Changing source or transform identity
requires a different region.

`supports_prefetch()` requires non-reclaiming admission, the native producer/capture guard,
and a separate replay recovery guard. Pass `reclaim=False`
for speculative work: a streaming-window deficit, physical allocation OOM, or an
abandoned mapped fill refuses admission without eviction, watermark reduction,
or a context join. Demand preparation keeps its existing recovery behavior.

Preparation starts no payload read. Call `start()` after active resident work has
been admitted. The native worker owns its checkpoint, local source, part descriptors
and Fill. It uses the existing buffered/O_DIRECT staging engine, with no Python
transformer, credential callback or DLPack-owned destination on the worker.
Staging/streams are warmed by preparation; timing events can still be acquired by
the read engine on first use. This is not complete CUDA-resource preallocation.
`poll()` and `wait()` report issued bytes separately from checked completion.
Only successful checked completion publishes the region; ordinary `prepare()` and
DLPack views can then consume it on any stream. This first interface does not publish
pending GPU writes, so no consumer waits on an unrecorded or pooled event.

`cancel()` requests that new bounded pieces stop; it does not join a running read.
`wait()` (or completed `poll()`) reports the checked drain of submitted copies.
Cancellation before `start()` settles immediately and releases the Fill without
requiring a later start. Cancellation never publishes partial data. Ordinary input failures also require checked producer
drain before the Fill is released. If completion is uncertain, the actual Fill,
region and source owners are retained and the arena is poisoned against reuse.
The existing conservative context join for unknown/escaped consumer reclamation
is unchanged. This is producer readiness, not a substitute for consumer lifetime.

The first overlap contract is a premapped/admitted raw destination beside a resident
compute loop. Decoder/kernel preload and separate evidence are required before any
decode-overlap claim. There is no persistent recipe index or allocator-OOM writeback.

Each admitted native read also owns a device producer lease from before mapping/staging
preparation through checked completion. Actual CUDA capture refuses while any such
lease exists, including an unstarted read; cancel or complete it before capture.
Producer admission likewise refuses during capture. Cancel-before-start releases this
lease immediately; unknown completion retains it with the actual owners. Resident graph
replay uses `varena.replay(device)`: it permits independent producers while suppressing
allocation recovery during the host launch. Retained physical pins protect graph dependencies
through device completion. Non-reclaiming producer admission and checked producer publication
may overlap this replay guard; paths that join a context or reclaim backing remain excluded.

Native mutex waits in Python methods release the GIL. Final storage release and abandoned-fill
abort update atomic pin/content state immediately; only final reservation teardown uses TensorFS's
existing cleanup worker. Allocation recovery never waits for that worker or for Python. Escaped
aliases keep separate pins, and reclaim still joins GPU readers before unmapping.

This candidate requires an executor-lived Arena that outlives its captures and replays. Final
Arena teardown during an unrelated capture/replay can retain backing and VA until process exit;
deferred teardown widens the window in which that can happen. It is not successful reclamation.
Temporary-Arena teardown across unrelated graph activity is not qualified by this implementation.

## Explicit execution phases

`supports_phases()` checks native methods, not the package version. At an actual
component-phase transition, `Arena.reset_preferences()` clears the soft preferred prefix
and its watermark. It never changes a physical pin, fill, mapping or virtual address.
The consumer marks its incoming cached regions preferred, then may call
`reclaim_unpreferred(bytes)` to release only other idle backing. Both methods refuse
actual capture/replay and release the GIL while waiting on native locks. Repeated blocks
or denoising calls within one phase must not reset the watermark.

## Native load followed by execution

`LoadSlot(arena, checkpoint, parts, bytes=..., threads=2)` reserves a dedicated address and
initializes its unpublished Fill with zeroes using the existing bounded read engine. Its
`tensor` is U8 storage for preparing a known captured body, not a claim that model weights
have been loaded. Every exported alias retains the physical pin. The initialization lease
is checked complete and released before capture; `seal()` closes further capture exports.

Runtime's `PreparedRepeat.capture(..., load=slot)` retains this owner and seals it after
capture. Submission hands TensorFS the private `cozy.execution.continuation.v1` capsule:
C-layout version/size/flags/device/PID, an opaque retained context, and native-only
retain/release/start/poll/cancel/wait functions. The ABI is declared by Runtime's
`native/execution/include/cozy_execution_v1.h`; TensorFS mirrors its C records, never Rust
objects. The capsule and provider stay owned through its final native release.

In checked mode, the existing refill worker reads the declared raw stored parts into that same address,
checks every copy stream, commits Fill to a retained Pin, starts the native continuation,
and waits for checked compute completion. No Python callback or binder runs between reading
and execution. `Dst.stream=0` selects the read engine's explicit owned streams, not CUDA's
legacy or per-thread default stream. Runtime records caller dependencies before export and
launches on its own explicit stream. Composed users wait for completion before consuming
results; submission makes no immediate ordering promise on the caller's stream.

A slot is one-shot. `cancel()` before submission persists and prevents launch. During a job
it requests native stop; `wait()` proves the whole chain complete. `close()` retires unused
work or requests cancellation while the job retains its active owners. Read failure and
cancellation settle the reserved execution epoch before releasing resources. Sticky or
unproven native completion retains the actual source, Fill/Pin and continuation owners until
process exit, with the arena poisoned. Ordinary region prefetch keeps its existing behavior.

The first slice retains its producer lease through compute, excluding concurrent actual
capture. `facts()` exposes source bytes and native stage times, `compute_wait_entered`, and
`completion_proven`; observations remain readable after an error. Only `completion_proven`
authorizes normal owner release after failure. Times are monotonic elapsed nanoseconds from
load submission.
Real release-gate proof must cover loaded-byte correctness, a parked Python thread, failed
source reads, cancellation before launch and during native wait, and continuous pin ownership.

`LoadSlot.encoded(arena, checkpoint, component, key, dtype="BF16", max_scratch_bytes=...,
threads=2)` prepares one encoded checkpoint tensor for the same checked native chain. Its
`tensor` is an initialized, contiguous typed view with the checkpoint's logical shape; output
dtype must be F16, BF16 or F32. The native shared format plan and installed CUDA catalog validate
all roles, shapes, lengths, layout and required kernels before any source read. No new format,
Python decoding callback or host-decoded upload fallback is introduced. ConvRot uses the same
existing rotated/H4 plan with resident source bindings.

The explicit scratch bound counts aligned packed parts plus the requested byte sizes of every
decoder-private GPU buffer, including metadata casts and both rotation buffers. It excludes
the final output, shared read-engine staging, CUDA modules/driver memory and allocation-granularity
overhead; it is not a hard process-VRAM ceiling. Scratch allocation uses the existing producer
guard and remains owned alongside the final Fill. Neither preparation nor an enqueue event
publishes weights as ready.

In checked mode, after a checked source read, the same worker calls the existing `raw::decode_into`, including
its device error-word and completion checks, then checks cancellation before committing the
final Fill and starting the unchanged continuation. Any failed read/decode launches zero
dependent graphs. A semantic decode failure drains before normal cleanup; uncertain completion
retains actual owners. The shared decoder currently erases numeric CUDA error codes into its
I/O family: that family during GPU decoding conservatively poisons/retains the chain. This
does not apply to known metadata or source-validation errors before decoding. Decoder capacity
failure uses the existing native out-of-memory family, never a disk-full error.

Returned native facts add `decode_scheme`, `decoded_bytes`, `scratch_bytes`, and
`decode_completed_ns`. Successful encoded work must satisfy
`read_completed_ns <= decode_completed_ns <= launch_started_ns`. Read bytes count stored parts,
not expanded output bytes. Raw slots retain zero decode counters and their existing behavior.

### Optional event ordering

Runtime's `load_ordering="events"` uses the size-checked optional continuation capability.
The placement's existing final-issue callback orders raw copies, the shared prepared decoder,
and dependent compute through native stream events. It performs no source reads, allocations,
byte-engine recursion or host completion wait inside the callback. Decoder preparation and
bounded staging remain native work; the callback does not run a Python transform.

The worker checks source and compute completion before publishing the Fill or returning output.
The physical pin, checkpoint, packed buffers and decoder scratch remain owned throughout; staging
is reusable only after its producer copy is checked complete. An encoded device error flag skips
the entire authored program, including index reset and prefix. A submitted graph therefore does
not prove body execution. Before enqueue commitment, a source/preflight/cancel error still means
zero submissions. Cancellation after that commitment is cooperative execution cancellation.

Late device or ownership failure discards output and retires uncertain owners; it cannot prove
that no body ran. This is an explicit contract distinction from checked mode, whose assertions
remain supported. Facts report selected `ordering`, `execution_committed`, `decode_issued_ns` and
`decode_checked_ns`. Host completion timestamps record observation, not device stage order;
event mode may observe decoder completion after compute. `decoded_bytes` remains zero when
encoded values fail validation. Only checked composed completion authorizes ordinary release.

Private decoder streams, metadata and packed GPU buffers are destroyed while the native producer
exclusion is still held; successful completion releases that exclusion only afterward. Closing an
unused encoded slot during actual capture defers its owning Work to the existing cleanup worker.
If capture is still active, the worker backs off briefly and requeues that same owner so unrelated
releases can proceed. It never frees private CUDA resources inside capture. Other exclusion or
completion failures retain the owners and poison the arena. Thus unused-slot GPU cleanup may be
delayed until capture ends; it does not block the capturing Python thread.

Composed slots check private event, allocation and stream destruction before publishing an
event-mode Fill. Successful handles become inert. A failed release retains the actual partially
released Work, poisons the arena and cannot be retried by Drop; no successful output is reported.
These checks reuse the decoder and buffer owners rather than a separate cleanup registry.
