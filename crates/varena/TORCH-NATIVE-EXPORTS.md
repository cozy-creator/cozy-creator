# Typed native tensor exports and immutable host Sources

Status: draft implementation for the Torch offload owner [#2449](https://github.com/cozy-creator/cozy-creator/pull/2449), native lane [#2450](https://github.com/cozy-creator/cozy-creator/pull/2450). Signatures were proposed on #2449 before implementation edits. Integration input is the same frozen #2397 `2ba51b29e5` snapshot. CPU/source checks do not qualify GPU execution; the coordinator owns GPU tests and integration.

## Public signatures

```python
Source.from_buffer(data: bytes | bytearray | memoryview, *, dtype: str,
                   shape: Sequence[int], strides: Sequence[int] | None = None,
                   byte_offset: int = 0) -> Source
Source.from_tensor(tensor: torch.Tensor) -> Source
Source.derive(dtype: str | torch.dtype) -> Source
Gpu.region(data: Source | bytes | bytearray | memoryview) -> Region
Use.tensor(index: int = 0, *, dtype: str | torch.dtype,
           shape: Sequence[int], strides: Sequence[int] | None = None,
           byte_offset: int = 0, stream: int | None = None) -> torch.Tensor
Region.tensor(*, dtype: str | torch.dtype, shape: Sequence[int],
              strides: Sequence[int] | None = None, byte_offset: int = 0,
              stream: int) -> torch.Tensor
```

`Source` is exported from `tensorfs.varena`. It snapshots complete native-endian CPU storage, including gaps, interior offsets and nonnegative element strides. CPU Tensor/Parameter inputs must be ordinary dense strided storage; resolve lazy conjugate/negative views first. Snapshotting requires the caller to prevent concurrent writes. No Tensor or mutable buffer remains an owner. `derive` converts the whole typed CPU storage into another immutable snapshot, preserving element strides and scaling byte offsets. Old Sources, Regions and escaped tensors keep their previous version. Supported dtypes are bool, signed/unsigned 8/16/32/64-bit integers, float16/bfloat16/float32/float64, complex64/complex128.

`Gpu.region` copies these immutable bytes through the existing native `vr_v1_region_from_host`. Native bounded staging, producer publication, failure and retirement remain unchanged. Source construction and conversion perform no CUDA work and do not invent checkpoint/file I/O or allocator callbacks. Empty Source metadata is valid; native Region reservations remain nonempty, so skip empty tensors when adopting a model.

## Native ownership and ordering

Install `tensorfs.varena.torch` before CUDA initialization and before a typed Torch export. The bridge refuses exports without its native consumer routing installed. `Use.tensor` defaults to the explicit consumer stream already supplied to `Gpu.use`. `Region.tensor` requires that stream explicitly and closes its temporary Use after obtaining the independently pinned tensor. The caller submits reads on that stream. For example:

```python
from tensorfs import varena
from tensorfs.varena import torch as vt
import torch

source = varena.Source.from_tensor(torch.arange(12, dtype=torch.float32).reshape(3, 4))
vt.install(0)
stream = torch.cuda.Stream(device=0)
with torch.cuda.stream(stream), varena.open(0) as gpu:
    with gpu.region(source) as region:
        weight = region.tensor(dtype=source.dtype, shape=source.shape,
                               strides=source.strides, byte_offset=source.byte_offset,
                               stream=stream.cuda_stream)
    result = weight.sum()  # Region wrapper is already closed; storage owns the Pin.
    del weight
    stream.synchronize()
```

The same-core Rust seam is:

```rust
pub fn abi::export_use(id: u64, index: usize, stream: u64)
    -> crate::Result<abi::TensorExport>;
// TensorExport exposes ptr(), len(), device(); its Drop owns release.
pub fn TensorExport::record_stream(&self, stream: u64) -> crate::Result<()>;
// The tracking module below is crate-private; it never supplies Pin ownership.
pub struct tensor_consumers::Consumers; // Clone
pub fn Consumers::new(pin: &Pin, initial_stream: u64) -> crate::Result<Self>;
pub fn Consumers::record(&self, stream: u64) -> crate::Result<()>;
pub unsafe fn Consumers::release(&self, pin: Pin) -> Arc<Region>;
```

`export_use` clones the canonical Use Pin and calls its existing `order_on(actual_stream)` before exposing storage. The Pin retains the in-flight submission ticket; this orders producer events without inferring completion or launching another fill. Explicit stream overrides order the same producer on the override. A native DLPack storage owner retains that Pin, its consumer family, and the canonical Weight, including Source and GPU-service custody. No Python attribute or temporary Use wrapper supplies storage lifetime.

The PyO3 bridge `_varena_public_use_dlpack(handle, index, code, bits, shape, strides, byte_offset, stream)` checks dtype, dimensions, strides, alignment, checked span and logical Region bounds. Torch 2.14.1 rejects a nonzero DLPack byte offset; the bridge therefore exports the checked interior data pointer with DLPack offset zero. The native owner still pins the entire Region. Scalars, zero extents and broadcast strides have explicit span semantics.

Every later consumer must establish CUDA dependencies before reading and call `tensor.record_stream(actual_stream)` before final storage release. A weak native range registry routes both base pointers and logical interior addresses to every live exported storage family for that Region. Registered reservation padding is rejected. Legacy default stream handles 0/1 are supported for later consumers; initial exports require an explicit stream and per-thread sentinel 2 is unsupported. Streams must remain live through final alias release.

The final native storage deleter records consumer tails after its last alias disappears, then transfers the exact canonical Weight to the existing native retirement queue. It does not attach Python or wait for GPU completion. Recording uncertainty falls back to the existing conservative context join at reclamation. Forked destructors retain inherited owners without entering inherited CUDA/owner locks.

## Qualification boundary

The standalone clients and receipts are in `release-gate/suites/varena/native-tensor-export`. Host checks cover real Torch Source behavior and the built native metadata/refusal boundary. The prepared GPU client belongs to the coordinator: native Pin lifetime after Use/Region closure, interior aliases, Source producer ordering, later streams, dtype versions and final release require actual CUDA execution. Deterministic in-flight/failing foreign Sources, capture and stock-model/SDXL VAE behavior remain separate coordinator/adapter gates. Typed exports are mutable Torch storage; the offload adapter must enforce immutable inference semantics.

There is no Python memory policy or application OOM retry. No rental, merge or release is authorized by this lane.
