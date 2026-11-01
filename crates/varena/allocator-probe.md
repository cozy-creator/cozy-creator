# Default-off allocator boundary probe

`tensorfs-py/allocator-probe` forwards to `varena/allocator-probe`. Neither feature is
default. An ordinary build has no probe module, TLS, bindings or marker calls. The
diagnostic changes code layout and adds CPU work; its allocator ratio cannot qualify
the normal 2× bar. It changes no allocator policy, PID check, cache, lifetime or shim ABI.

The purpose is to attribute the actual user/system CPU excess measured in xs-138. It
measures the existing Rust Torch callbacks and working allocator, not a replacement or
mock allocator. CPU-only calibration contains no real allocator operation or CUDA call.

## Records and boundaries

Each OS thread has 1,024 fixed slots. Reset touches them outside the host timing window.
Allocation and free each increment their own sequence and sample every 1,021st call.
The stride is coprime with the benchmark's eight sizes. A default 400,000-pair sample
therefore produces 391 allocation and 391 free rows; the fixture resets/reads each
sample separately. Overflow or reentrancy is reported, never silently discarded.

The Rust bridge starts a record before its installed/error checks and finishes after
the native result/error handling. Core entry/exit bracket the actual working call.
The PID span brackets `std::process::id()` itself; the original identity comparison
still occurs before either inherited registry mutex. Other spans cover:

- capture-registry acquisition on free;
- device-registry acquisition, lookup, Arc clone and registry unlock;
- working-state acquisition and poison check;
- bookkeeping from acquired working state through slot/block/lifetime/accounting changes.

Core exit follows working/capture unlocks and owner-Arc retirement. The residual core
span therefore includes those effects, size-class/other branches and gaps between
markers. Bridge total and core total are **inclusive**. The fixture reports disjoint
core spans plus `core_other`, and `bridge_outside_core`; adding the two inclusive totals
again would double-count. Timestamps are nanoseconds since reset from `Instant`.

Marker selection precedes the first timestamp; row storage follows the last timestamp.
Those costs are not part of the timestamped bridge span. Adjacent marker/helper work is
included in interval differences. No per-call Python, shared lock, logging, heap growth
or CUDA call is added by the probe. Records mark missing/duplicate points, nesting,
slow recovery and failed callbacks; such records do not qualify warm-path attribution.

## CPU calibration and uncertainty

The diagnostic child calibrates on the actual host before its timed samples:

1. 256 empty monotonic-clock pairs.
2. 128 sampled empty records for each operation, using its exact marker and allocation-
   metadata helper order. These are empty spans, not fake allocations or PID calls.
3. Disabled/enabled enter/exit-only loops, 100,000 callback iterations each.
4. Disabled/enabled **whole empty marker bundles**, 50,000 alloc/free pairs each. Every
   unsampled mark and allocation-metadata helper still executes, just as in the real
   diagnostic build; occasional sampled clocks/storage remain included.

Calibration reports raw elapsed values and units. Dispatch/bundle loops include their
own loop overhead. Empty spans show timer/helper scale, not an exact correction for
each differently compiled real path. Nothing automatically subtracts calibration or
clamps negative adjusted estimates. Interpret differences smaller than this uncertainty
as unresolved. No fixed-function latency is inferred from source counts.

## Real fixture and durable receipt

`p0-working/allocator_probe` uses the same `Benchmark` input and original native-then-
Varena children, sizes, warmup, seven samples and 400,000 pairs. `pairs()` and statements
between start/submitted timestamps are unchanged. Private reset/read methods run outside
that interval; post-submitted serialization is included only in drain/completion time.
The normal `benchmark` entry never enables the probe.

The receipt schema is `varena.allocator_probe/1`: named columns, operation/flag mappings,
timestamp names, sequence/size/class and complete compact raw rows. The fixture validates
exact counts, every size, marker presence/order, no overflow/nesting/error/slow path and
zero timed driver/event/context-call deltas. A validation failure rejects attribution
with `diagnostic_valid=False` while retaining its raw data.

Runtime's real typed inline-result door is 4 MiB, independent of its 256 MiB corrupt-frame
guard. The distinct diagnostic entry saves the **entire** report as an ordinary JSON
`File` through `Outputs.save_bytes`, with SHA256/length and summaries inline. This uses
normal output custody and its existing authored allowance; no invented size ceiling or
log-only transport exists. Raw rows and calibration remain available even when diagnostic
validation fails. Telemetry carries a short summary, not the only timing evidence.

The actual GPU run remains a separate root-owned qualification step. Feature-off/on
compilation and metadata/AST review are not function-cost measurements. A later justified
production change must rerun the unchanged uninstrumented gate and fork/lifetime/error
checks; this probe does not authorize an owner cache or removal of process checks.
