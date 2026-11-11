# Frozen producer references

The numerical definitions in `ci_offline_producers/oracle.py` were selected verbatim
from Runtime `8c057e77d2fc0b25f7d3e4700930163cc9d4dd53` before deletion, with `row_chunks` from
its existing safetensors reader. The reference imports neither current production
producer adapters nor native TensorFS encoders. It is a standalone release-gate
artifact, never a production fallback.

- Original `derive/microscale.py`: SHA256 `f089dbd1b6d193b79e81e5b16f68246e5878c7b524569314698d0fc82cdb3d70`.
- Original `derive/safetensors_io.py`: SHA256 `f5ef98c05ff4bcc00b5a8689323f215b088ee181370f02bf8a4172025344dd59`.
- Frozen `oracle.py`: SHA256 `4c8182edc9216a91b69fc56ca3e8172f476c66adced5fa29c3e4f7140151bbb4`.

FP8 is per-row F32 maximum/448 floored at F32 1e-12, F32 division, saturating
E4M3 RNE and F32 reconstructed multiply before F64 scoring. W4A8 is F64 H256/16,
the frozen F32 codebook widened to F64, two ALS passes over 16-element groups,
F32 row scales floored at 1e-8, E4M3 relative group scales, F32 multiplication
before RNE/clipping to signed int8 levels, and first-index tie selection.
The native implementation preserves the [NumPy contiguous F64 reduction order](https://github.com/numpy/numpy/blob/main/numpy/_core/src/umath/loops_utils.h.src)
for each 16-element ALS sum. All stored bytes compare exactly; reduction-order
changes in accumulated SSE/energy are allowed only within relative tolerance 2e-13.
