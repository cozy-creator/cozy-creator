# Varena's public API

Three frontends over one `libvarena.so`, which ships in the TensorFS wheel:

- **C:** [include/varena.h](include/varena.h), `vr_v1_*` symbols. No CUDA SDK or Rust compiler needed. Layouts never grow; an
  incompatible change gets new symbols.
- **Rust:** `crates/varena-client`, which links only the C ABI.
- **Python:** `tensorfs.varena`. `tensorfs.varena.torch` adds Torch ([TORCH-OFFLOAD.md](TORCH-OFFLOAD.md)).

| Concept | What it is |
|---|---|
| `Gpu` | this process's handle on one device; opening it takes a ledger row, in shared mode |
| `Region` | immutable bytes at a stable address, from host bytes, borrowed host parts or a `Source`; evictable when idle and refilled on use |
| `Use` | pins Regions for one stream until `done` |
| working allocation | `alloc`, `free`, `record_stream`: stream-ordered memory |
| `Cache` | a reserved range with exact page `commit` and `release`, and an optional shrink callback |
| `Plan` | named phases and their Regions; `phase(name, key)` measures a completed pass |

Splits and graph pools are Torch-side only: no C calls. Everyone allocates and declares intent; only the core decides what stays mapped.

```python
from tensorfs import varena

with varena.open(0) as gpu, gpu.region(weights) as region, gpu.plan({"decode": [region]}) as plan:
    with plan.phase("decode", "u8/15/my-kernel/eager"):
        with gpu.use([region], stream) as used, gpu.alloc(4096, stream) as scratch:
            ...  # submit your kernel on `stream`, using used.ptr() and scratch.ptr
```

`stream` is a live CUDA stream you own in the GPU's primary context.

## Streams

- Pass the stream your work actually runs on. `0`, `1` and `2`, the default-stream sentinels, are refused. So is an operation on a stream under capture.
- Name the stream on every launch. A kernel launched with no stream argument goes to the legacy default stream, which only syncs with blocking streams.
- `use` orders your stream's next work behind the fill. It doesn't mean the GPU is done, and only that stream is ordered: another reader takes its own
  Use, or a CUDA dependency before reading.
- Join every other consumer into the Use's or allocation's stream before `done` or `free`, or `record_stream` the allocation's side streams.
- Keep a stream alive through every deferred release on it. Before destroying a Torch stream, stop submitting and call `vt.retire_stream(stream)`.

## Ownership

- A Region pointer is only an address. A Use pins its backing until `done`, and closing the Region first doesn't invalidate it.
- No raw pointer or escaped tensor may outlive its Use, allocation or cache pages. GPU writes to Regions, training and fork are unsupported.
- `free` doesn't mean the memory is back. It is stream-ordered, and the core takes the pages later.
- A failed constructor zeroes its output and keeps caller ownership. A refused checked close keeps the handle, to retry once the reason has passed.
- A finalizer (`vr_v1_defer_close`, Rust `Drop`, Python `__del__`) hands the owner to one native retirer, which polls until its work is done. A job
  that fails stays there, since nothing unproven is destroyed: `retirement_stats` reports queued, retired and quarantined jobs and abandoned cache bytes.
- Sources may run again after eviction. A C callback must initialize every byte, keep its staging through completion even on failure, and never
  unwind or use its own Region. The ABI takes a Source only if construction succeeds.

## Caches

`commit` is all-or-nothing over wholly unmapped, page-aligned ranges, and holes are fine. `release` needs every reader and writer joined into its stream.

A shrink callback runs on a native worker with no allocator lock held, and Python's takes the GIL there. It chooses what to drop and releases those
ranges. Returning false, or `VR_BUSY`, declines. Only completed releases count: requested bytes don't, and a live KV a sequence still needs isn't free
memory. Close refuses live or pending pages, and may not be called from the callback.

## Errors

| Status | Python | Means |
|---|---|---|
| `VR_EXHAUSTED` | `Exhausted` | nothing more can be reclaimed; `vr_v1_error_requested_bytes` says how much was asked |
| `VR_CAPTURING` | `Capturing` | refused during capture |
| `VR_SOURCE_ERROR` | `SourceError` | a clean fill failure; the device is fine |
| `VR_FAULT` | `Fault` | a CUDA fault or a panic at the boundary: retire the process |
| `VR_INVALID` | `Invalid` | a violated contract |
| `VR_UNSUPPORTED` | `Unsupported` | not available here |
| `VR_BUSY` | `Busy` | try again |

`vr_v1_last_error` is per thread, unchanged by success, and valid until the next failure on that thread. Nothing retries application code.

Stats are snapshots, not reservations. `working_mapped` includes graph pools (`pools` is that part) and excludes `caches`. `working_capacity` includes
rounding, so it isn't `working_live`.

## Building and linking

`varena` builds only as a dylib, because a second linked copy would have its own device table. C and Rust clients link the wheel's `libvarena.so`, installed
at `<prefix>/lib/varena-v1` with the header at `<prefix>/include/varena/v1/varena.h`. Python checks the loaded core's identity against the
bundled one before importing `_ext`, and refuses a second allocator. `cozy` links no native core: `varena` is `tensorfs`'s optional `cuda` feature, enabled
only by the wheel. Build the SDK with `crates/varena/bundle.py`, as `prove-host.sh` does.

## Examples

Each needs a CUDA GPU, and passes or asserts.

| Example | Shows |
|---|---|
| [examples/native_engine.c](examples/native_engine.c) | a C engine loading a PTX kernel: Region, Plan, Use, scratch, no Python |
| `crates/varena-client/examples/engine.rs` | the same in Rust, with scoped ownership |
| `crates/varena-client/examples/parts.rs`, `source.rs` | one Region from many parts; a Rust Source |
| [examples/python_client.py](examples/python_client.py) | Python reading a Region's exact bytes back |
| [examples/cache_application.py](examples/cache_application.py) | interior holes and recommit in a Cache, with a shrink callback |

```sh
bash release-gate/suites/varena/public-api/prove-host.sh OUT    # builds the SDK and C engine
export PYTHONPATH=OUT/sdk/python LD_LIBRARY_PATH=OUT/sdk/python/tensorfs/.native
OUT/native-engine
target/debug/examples/engine
python crates/varena/examples/python_client.py
```
