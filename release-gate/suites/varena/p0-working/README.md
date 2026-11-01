# Varena v2 P0 memory and capture proof

This bounded, model-free CUDA application runs through the ordinary Cozy CLI. Its fresh child installs the
candidate whole-process allocator before CUDA initialization. It downloads no weights and creates at most
256 MiB of fixture-managed working storage; CUDA contexts and library overhead are reported separately.

On an owned machine carrying the candidate TensorFS wheel:

```sh
cozy machine update "$NAME" --tensorfs "$TENSORFS_WHEEL" -y
cozy run ./release-gate/suites/varena/p0-working/gpu --on "$NAME" --gpus=1 \
  tensorfs_version="$TENSORFS_VERSION" extension_sha256="$EXTENSION_SHA256" \
  shim_sha256="$SHIM_SHA256" seed:=138
```

Get expected version and the two binary hashes from the exact candidate wheel, before installing it. The fixture
checks the installed extension (which contains Rust Varena) and shim, then records paths, Python, Torch commit,
GPU and per-case results. `case=<name>` selects one case; default `all` requires every registered case. Candidate build/installation
identity is separate from a source checkout. Never install this candidate into a shared local generation.

Cases cover empty capture initialization, 512-byte alignment, same-stream reuse, cross-stream retirement, reuse
inside capture, retained outputs after graph destruction, changed replay stream, shared and distinct graph pools,
clean capture exhaustion, and three graph replays after managed weight-region eviction/refill at the same address.
Driver memory/event counters are compared inside capture, outside setup and teardown. Each physical-byte
partition must balance; after every case all fixture allocations, graph pools and allocator event handles retire.
Cleanup seals deferred frees, synchronizes their completion and trims again. The `default-deferred-free` case
checks metadata-only default-stream reuse, then switches streams and verifies pending memory is not reused early.
The RNG case starts with no retained eager blocks, compares default and custom generators with eager byte-exact
references across two replays, reseeds and reproduces the first result, then verifies retained-output and final
teardown lifetimes. Its auxiliary default-stream allocations must also use pre-mapped capacity. No mock allocator/driver,
whole-card ballast, injected device fault, or package-call retry is used. Full SDXL/H3 performance/quality and real
card-pressure qualification remain separate P0 requirements.

## Allocator microbenchmark

Use the same exact candidate binary version/hashes as the stress call:

```sh
cozy run ./release-gate/suites/varena/p0-working/benchmark --on "$NAME" --gpus=1 \
  tensorfs_version="$TENSORFS_VERSION" extension_sha256="$EXTENSION_SHA256" \
  shim_sha256="$SHIM_SHA256" samples:=7 rounds:=50000
```

The app launches a fresh Torch-native child, waits for it to exit, then launches a fresh Varena child with the
same Python executable and visible GPU. Both verify the candidate extension/shim hashes even though only the
Varena child installs that allocator. Each warms once, then measures seven identical samples of eight allocation
sizes (512 bytes through 8 MiB) on the legacy default stream. There is at most one live request at a time; mapped
fixture storage is checked against 256 MiB after warmup and every sample. Context/library memory is outside this cap.

The primary timing is host time for Python dispatch into Torch's C++ raw alloc/free entrypoints. No synchronization,
telemetry, kernel launch or copy occurs inside the pair loop. Setup and warmup are excluded; the completion-inclusive
sample duration and final drain are separately reported. Every sample, size, pair count, range, binary identity and
GPU identity is retained. The P0 microbenchmark passes when the median host nanoseconds per pair is no more than
2× native. It is not a C++-only measurement or a model-latency result; Python dispatch overhead remains in both arms.
The benchmark returns `Report(passed=false, detail=<full JSON>)` on a ratio failure so every failed sample survives
in the result artifact. A successfully delivered Cozy call is not qualification: inspect `result.passed` and the
unchanged 2× ratio limit. Child execution/provenance failures still fail the call.

Default and Torch-pooled streams live for the process. An owner of a destroyable stream must stop submissions,
synchronize after its final graph/tensor teardown, call `vt.retire_stream(stream)`, then destroy the handle. CUDA
stream destruction alone can return with work pending. Arbitrary external destruction without this retirement
contract remains unsupported; the default-stream benchmark does not qualify it.

The `replay-hook-failure` case launches real graph work on a different replay stream, then throws from Torch’s
per-graph replay-end hook. It checks that the original exception survives and the pool completion fence is still
recorded. A second arm resets the graph and requests trim inside that hook, requiring transient replay ownership
to retain the pool through completion recording. Both arms copy and validate the GPU result, then verify retirement.

`dlpack-record-stream` covers an interior-pointer DLPack view of working storage and a foreign TensorFS Region
alias. `external-stream-retirement` creates actual CUDA runtime streams, drains and retires each, destroys it,
then frees a retained tensor. Its three arms cover no remaining consumer, an already-declared consumer and a new
consumer declared after retirement. Both cases validate GPU output and final allocation/event retirement.

`pool-variable-reuse` fixes the prepared pool at 16 MiB. One arm frees a 16 MiB temporary then allocates 8 MiB;
the other frees two simultaneously live 8 MiB temporaries then allocates 16 MiB. Both measure a 16 MiB live
peak, copy into eager-owned destinations, check output bytes and make no allocator driver/event calls inside
capture. Each arm records its own result, including OOM, and neither retries with a larger pool. Both must pass.

## Fork rejection before CUDA

The separate CPU-only `/fork` entrypoint uses the same candidate version/hash payload. Its fresh child calls
`vt.install()` without opening a device, then makes six actual forks to check explicit status, empty-registry
stats, null alloc/free/record-stream callbacks and empty-registry trim. Each must reject the inherited process
before CUDA initialization. Separate forks prevent an earlier error latch from masking a later missing check;
the parent must remain usable with zero allocator/driver counters. This does not replace any GPU case or benchmark.

## Torch 2.14.0 production cohort

The fixture's project and lock select SDXL's exact Linux `torch==2.14.0` PyPI distribution. The CPython3.12
x86_64 wheel URL and SHA256 match `packages/sdxl/uv.lock` (SHA256 starts `fecffb58f51fd643`).
The original fourteen case bodies and allocator timing loop are unchanged; variable-size pool reuse adds one
case with two required arms. Both entrypoints default their expected
`torch_version` to `2.14.0+cu130` and fail before execution on another build. Receipts also record distribution
version, Torch source commit, CUDA version and C++ ABI; compare these with the actual model receipt because the
model's PyPI distribution may spell its version `2.14.0` while `torch.__version__` includes `+cu130`.

`cozy machine update` still selects the candidate Runtime/TensorFS pair. The fixture verifies their installed
extension and shim hashes. Do not replace that path with an untracked interpreter or install a different Torch
into an active generation. Preserve existing Torch2.14.1 receipts as evidence for their own cohort.

## cuDNN capture shortfall

`case=cudnn-capture-shortfall` uses real float32 NCHW convolutions. It selects a fixed small shape whose repeated
eager execution has a measured nonzero workspace and bitwise-stable output. All proposed shapes have2MiB output
and under5MiB of inputs/weights; the case requires peak managed backing below256MiB. No driver or library is mocked.

It reserves exactly2MiB for capture: the output fits, but the measured workspace cannot. The required contract is
one native preparation refusal escaping cuDNN immediately; `vt.graph` may translate it to Torch OOM only after
actual capture has ended and cleanup completes. The subsequent eager convolution must preserve exact output and
workspace demand. Diagnostics are emitted before assertions, so old cuDNN retries/cache mutation remain visible.
Existing cases and allocator microbenchmark settings/bars are unchanged. This new case has source/syntax checks
only until its real GPU receipt is recorded; no pass is inferred from its construction.

## Compute Sanitizer cleanup

Use Compute Sanitizer 2025.4.1 for this Python proof: NVIDIA's [release notes](https://docs.nvidia.com/compute-sanitizer/ReleaseNotes/index.html#updates-in-2025-4-1)
document a Python-launch memory-leak fix. The unchanged fixture on 2025.3.1 retained a live tensor after graph teardown (run 23954); changing only
the tool to 2025.4.1 retired all managed bytes/pools/events in all 16 cases (23987). That second run still failed
full leak checking: 9 bytes were unused Torch pinned-host scalar staging buffers, and another 5,384 bytes
were attributed to cuDNN handle creation. The latter remain under investigation and are not suppressed.

After all cases retire, the child now calls public `torch.accelerator.empty_host_cache()` and records host
statistics before/after. Both active and allocated host bytes must reach zero. Device ownership assertions,
full memcheck/leak checking and the error exit code remain unchanged. This does not qualify full-model memcheck.
Instrumented runs return the complete report with `passed=false` on tool/teardown failure, avoiding exception
message truncation. As with the benchmark, successful CLI delivery is not qualification: require `result.passed`
and the instrumented report's zero exit code and zero error summaries.

## Native cuDNN lifetime control

The explicit `/native_cudnn` entrypoint launches a fresh child from the same interpreter/SDK and checks the same
Torch0 and candidate binary provenance. It never installs Varena; it asserts Torch's native allocator. A real
float32 convolution uses input1×32×128×128,32 output channels and3×3 filters, matching the shortfall case's
selected geometry. It verifies finite copied output, deletes every tensor, synchronizes and empties native device
and public host caches. Both native allocated/reserved and host active/allocated counters must reach zero.

Use the same `compute_sanitizer` payload and2025.4.1 binary as `/gpu`. The shared launcher retains memcheck,
full leak checking, error exit99 and all-process checking. A nonzero sanitizer verdict remains `passed:false`
with its complete report; the control does not suppress leaks or redefine a failing sanitizer run as clean.

Torch2.14.0's cuDNN handle destroy callback intentionally omits cudnnDestroy because of shutdown-order crashes;
thread exit returns handles to the process pool. The5,384bytes reported through cudnnCreate in run23987 are
therefore a distinct ownership hypothesis from the1+8bytes of scalar pinned-host cache. This control tests
whether the same cuDNN allocations persist with the native allocator after legitimate cache cleanup.
Source: https://raw.githubusercontent.com/pytorch/pytorch/08187d9e0fba026dc8217405802ab5381dc88d90/aten/src/ATen/cudnn/Handle.cpp
No borrowed handle is manually destroyed; no context reset or suppression is used. All16 Varena cases and the
allocator microbenchmark remain unchanged.

Native control **24009** and Varena **24010** both finish with zero managed device and host-cache bytes, then
report the same three cuDNN allocations (5,376 + 4 + 4 bytes) under full leak checking. All 16 Varena cases pass
their device/pool/event assertions. Both instrumented reports remain `passed=false`, exit 99 / three errors;
the matching native lifetime behavior is recorded separately from the tool's strict process-exit verdict.

## P1 additions (source only until rerun)

The original 16 CUDA cases remain. `native-working-ownership` adds actual TensorFS IO into
Varena backing, including reuse after pending default-stream writes and destruction while an
unreported DLPack side-stream consumer runs. `shared-managed-limit` checks that both weight
backing and working mappings count, and clearing a static ceiling or temporary room does not
lift the other. These cases use the same fresh-child binary provenance and <=256 MiB bound.
Capture diagnostics additionally include attempted unknown-owner `context_joins`.


## P2 phase observation and explicit raw admission (source only until run)

`/phase_policy` uses the same exact-wheel provenance and fresh CUDA child as `/gpu`,
but runs the P2 observation/admission cases. It creates one local immutable raw TensorFS
source and bounds device storage with the real Varena managed limit; no model download,
mock driver, whole-card ballast or production prefetch cutover is used.

```sh
cozy run ./release-gate/suites/varena/p0-working/phase_policy --on "$NAME" --gpus=1 \
  tensorfs_version="$TENSORFS_VERSION" extension_sha256="$EXTENSION_SHA256" \
  shim_sha256="$SHIM_SHA256" case=all
```

The cases require last-success replacement (not a historical maximum), actual pending
capacity despite a global peak reset, preserved samples after abort/caught OOM/retry/key
mismatch, rejected overlapping scopes, unknown-key refusal, and non-publication for
preparation/capture/replay. A native replay guard entered and left inside an eager scope
must invalidate that sample even without Torch's replay hooks. Dead-owner records retire.

The raw-source cases reproduce three concrete admission mistakes:

- A prior 64 MiB phase plus an unrelated 32 MiB live tensor under a 96 MiB cap must decline
  a 2 MiB prefetch, preserving room for the repeated 64 MiB operation. The outside live
  tensor cannot be subtracted from the phase's future demand.
- Simultaneous 512 B and 1024 B allocations use two real 2 MiB size-class slabs. After trim,
  a 4 MiB cap must decline a 2 MiB prefetch, despite only 1536 B of live block demand.
- An almost-full slab holding 4095 outside 512 B allocations gives no current-phase credit.
  The repeated two-allocation phase needs another slab and must retain that room.

Additional arms retire outside pending buffers without subtracting phase-owned demand,
retag same-address pending reuse, complete and byte-check an admitted raw source, and
cancel a second read admitted beside already completed eager backing. Every case drains
its real stream/fill/tensor owners and checks zero managed bytes, graph pools and events.
These are explicit API proofs. The current model prefetch and split policy remain unchanged;
known configuration commitment and native replay-demand integration are still required
before replacing those production decisions. A successful delivery is not a passed proof:
inspect the result and every case. No P2 GPU receipt exists at this source checkpoint.

## P2 graph-definition replay observations (source only until run)

`/replay_policy` uses the same exact candidate payload and fresh-process harness. It records
real `keep_graph=True` CUDA graphs with temporary allocations that are freed during recording,
then uses Runtime's actual ResidentRepeat/NativeSequence continuation ABI. No mock capsule,
Python replay loop or direct model-internal invocation supplies the completion evidence.

The source fixture checks: capture itself publishes nothing; a reusable native stage records
three successful completions; prefix/body traces union shared scratch instead of summing a
whole pool; a graph's reset retires its token while another graph retains the same pool; an
unrelated graph cannot qualify using that stale token; fresh graph definitions with the same
semantic key start separate samples; unknown keys and failed capture do not publish. A
measurement-only wrapper must leave weight-preference transitions at zero.

The callback arm holds one bounded callback interval across native observe/rearm. A real GPU
sleep gives the host time to observe the first successful iteration and the active second
observation. A subsequent actual allocation/free must invalidate only the current stage and
preserve the first successful sample. The test verifies this interleave in the returned native
facts; if the required interleave was missed it fails instead of assuming a timing-based pass.
All graphs, native continuations, tensors and events are retired through their real owners.
Exact native execution module path/hash is included alongside TensorFS/Torch provenance.

Run only on the owned rental after installing the paired candidate; this source checkpoint has
not executed these CUDA cells. The existing 18 allocator cases, P2 eager/admission fixture and
unchanged allocation-overhead gate remain independent required checks.

## P2 production raw lookahead (source only until run)

`/raw_prefetch` creates real immutable raw TensorFS groups, captures their readers at fixed
addresses, detaches temporary input aliases, and runs two passes through actual native source
and graph continuations. A small existing demand streaming limit forces second-pass misses;
the new speculation path must leave that setting unchanged.

The fitting case requires the first current definition's unknown sample to decline and its
second pass to admit/read/use the complete 4 MiB successor. Three refusal cases require zero
speculative issued bytes: complete-group physical room is insufficient even though its first
region fits, the supplied semantic key is unrelated, or the supplied graph definition differs
from the active current stage. The managed limit is set after linking, so link-time capacity
cannot authorize later reads. All demand outputs must still contain the exact stored bytes.

The fixture reports actual prefetch_started/used/skipped and issued/read bytes, native
observation rows, unchanged streaming limit and final allocator/weight retirement. Delivered
CLI completion is not sufficient; the fitting arm must have actual source lookahead and the
refusal arms must not invalidate their successful demand measurements. These CUDA cells and
full-model capture/denoise/H2D qualification remain pending at this source checkpoint.

## P2 native split lifecycle and capacity (source only until run)

`/split_policy` exercises real CUDA allocation/copy operations under a copied menu with one
32 MiB output and 32/16/8 MiB scratch variants. The native managed caps select 64/48/40 MiB
choices; only the executed exact choice becomes measured after one enclosing phase completion.
A query before that proof must still say declared. No per-chunk synchronization or operation
retry is used.

Additional cells require typed minimum exhaustion before any allocation operation, separate
outside-live accounting, the real two-small-class slab floor, rejected nested leaf ownership,
preserved successful samples after abort or a caught actual allocation failure, and no learning
from a side-stream leaf without a matching completion owner. An empty invalidation diagnostic
must still prevent publication and preserve the previous successful phase sample.

These cells qualify native menu/admission/observation mechanics, not the actual channel_norm
consumer or its quality/performance. That requires the paired Runtime's real Request, phase and
compiled launcher path; full-model, capture-choice and multi-GPU cutovers remain separate.

The stream-aware split followup adds four real cells. An 8 MiB legacy-default-stream buffer is
freed while its preceding GPU work is still pending; under a 40 MiB managed cap, a 32 MiB output
plus 8 MiB scratch must select/begin without a completion query, then reuse the original scratch
pointer in order. The same pending block must not count on another stream or twice for two
simultaneous equal-class requests. A globally completed compatible cache block must also keep
its pointer through admission. Cases verify actual pending events and fail if their required
interleave was missed rather than treating elapsed time as proof. All existing menu, completion,
minimum-refusal and ownership cells now pass the actual stream explicitly.

## P2 clearing the legacy cache window (source-only until run)

`/retention_policy` uses the actual native allocator and two fixed-address 4 MiB
weight regions. The roomy 8 MiB case first demonstrates the old 4 MiB streaming
window evicting one region, then clears that window and requires both regions to
remain resident without another eviction. The refilled region keeps its address.
The two constrained cases require demand reclamation to remain effective: a 4 MiB
managed limit, and a 4 MiB externally assigned room under an 8 MiB managed limit.
These are capacity mechanisms, not claims about smaller GPU hardware.

The clear method is called under real native capture and replay exclusion guards.
All native arena statistics except the window itself must remain unchanged, and
preferences remain unset. The fixture checks actual byte contents, fill/eviction
counts and reasons, then releases every region and verifies zero managed working
retirement through the common CUDA fixture. No borrowed library handle is reset.
The external room control exercised here is the existing `make_room` interface;
this does not qualify P3's cross-process table or automatic policy.

The API changes only arena intent metadata. It does not query CUDA, reclaim
backing, alter process/development/peer limits, change pins or choose a Runtime
execution mode. The separately owned Runtime retention change must qualify its
actual graph/cache ownership boundary before using it. Native source checks and
fixture syntax pass; GPU execution remains pending.
# Bounded working-cache trim regression

`bounded_trim` creates 8 MiB of ready, unpinned weight bytes plus three completed idle
working allocations (8, 16, and 32 MiB) under a 64 MiB managed limit. An actual 24 MiB
allocation cannot reuse their exact size classes. Releasing at most 32 MiB of whole
working segments suffices; the case requires at least 24 MiB of other completed free
backing while its correctly filled 24 MiB output is live, and at least 48 MiB free after
that output's stream completes and retires. It verifies every output and weight byte,
unchanged weight fills/evictions, and final zero native working/weight ownership.

The case does not assert segment ordering or pointer selection. It uses real drained
stream retirement to establish completed cache without calling trim. `Report.passed`
is the cache-preservation gate; `detail.execution_passed` separately records successful
allocation, byte checks, and cleanup. A red preservation result retains the full report.
Initial source is unchanged `9ccb36a2`; no product fix or H3-causality claim is included.
