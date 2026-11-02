Native export/Source lane for this contract, separate worktree `/home/fidika/cozy/.worktrees/cozy-creator/xs138-p4-torch-native-exports`, branch `codex/xs138-p4-torch-native-exports`. Fresh master `9bb428322`; imported the same frozen #2397 snapshot as this adapter lane. No shared implementation edits yet.

Proposed exact public Python signatures:
```python
Source.from_buffer(data: bytes | bytearray | memoryview, *, dtype: str,
                   shape: Sequence[int], strides: Sequence[int] | None = None,
                   byte_offset: int = 0) -> Source
Source.from_tensor(tensor: torch.Tensor) -> Source  # CPU, dense strided
Source.derive(dtype: str | torch.dtype) -> Source
Gpu.region(data: Source | bytes | bytearray | memoryview) -> Region
Use.tensor(index: int = 0, *, dtype: str | torch.dtype,
           shape: Sequence[int], strides: Sequence[int] | None = None,
           byte_offset: int = 0, stream: int | None = None) -> torch.Tensor
Region.tensor(*, dtype: str | torch.dtype, shape: Sequence[int],
              strides: Sequence[int] | None = None, byte_offset: int = 0,
              stream: int) -> torch.Tensor
```
`Use.tensor` defaults to the actual explicit stream named by `Gpu.use`, never to a Torch default stream. Explicit export stream is ordered through native `Region::prepare_on` before exposure, even when the Source is still in flight. Metadata uses element strides and byte offsets, checked natively against the logical Region bytes. A storage-owned DLPack deleter retains a cloned native Pin plus Source/GPU owners independent of Use/Python wrappers.

Native internal seam:
```rust
pub fn abi::export_use(id: u64, index: usize, stream: u64) -> crate::Result<TensorExport>;
pub struct abi::TensorExport; // owns Pin + consumer family + canonical Weight owner
pub mod tensor_consumers {
    pub struct Consumers; // clonable native shared family
    pub fn Consumers::new(pin: &Pin, initial_stream: u64) -> crate::Result<Self>;
    pub fn Consumers::record(&self, stream: u64) -> crate::Result<()>;
    pub unsafe fn Consumers::release(&self, pin: Pin) -> Arc<Region>;
}
```
The PyO3 capsule entry `_varena_public_use_dlpack(handle, index, code, bits, shape, strides, byte_offset, stream)` bridges this shared core; no independent allocator. Weak native pointer-range routing feeds consumer declarations from Torch `record_stream` for Region base and interior pointers. Final release records tails after last storage alias; errors retain conservative native joining.

Source is an immutable host description: buffer and CPU tensor entry points snapshot bytes, with shape/strides/offset and dtype retained. `derive` converts whole storage on CPU into a new immutable Source; existing Sources/Regions/escaped aliases retain the old version. `Gpu.region` adopts through existing `vr_v1_region_from_host`, so native staging, producer publication/failure and retirement stay unchanged. No Python memory policy, application OOM retries or allocator-stack Python Source callbacks.

Files I will own in this lane: new `sources.py`, new PyO3 public tensor export module, native `abi.rs` export seam/new `tensor_consumers.rs`, small `working.rs` routing and `api.py`/`__init__.py` wiring, new standalone release-gate clients. I will not edit the offload owner's worktree. GPU qualification stays with the coordinator. Please flag signature/ownership conflicts here; I will publish the separate draft implementation PR and exact adopting commit.
