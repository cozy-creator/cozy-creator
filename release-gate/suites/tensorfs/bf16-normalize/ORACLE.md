# Frozen BF16 normalization reference

Runtime source commit: `9bf8b6b1fa6a50b4951172cbae49c80470a8cdbb`.
`oracle.py` retains verbatim `numel`, `to_f32` and `from_f32` definitions from
`derive/safetensors_io.py`. Its small `normalize` entrypoint uses write_bf16's
same F32 difference/absolute-value calculation before F64 squared error and
energy accumulation and the exact amax/256 acceptance threshold. It explicitly
rejects nonfinite source/reconstruction. RNE carrier behavior, including negative
zero, is checked by bytes. SSE/energy may differ only by reduction order within
relative tolerance2e-13.

The original full modules and selected write_bf16/read functions are preserved
as historical text beside the oracle, before production deletion. The original
RowwiseDequant is also preserved: widen payload and scale to F32, multiply once
in F32, with no logical F16/BF16 narrowing before normalization. The oracle
independently constructs FP8 and I8 element values; it imports no current
production converter or decoder.

These files are gate-only references, never a production fallback. Empty tensor
handling deliberately covers the approved native contract; the old Python
reader's leading-empty case produced no windows, and inner-empty fixtures expose
that boundary explicitly. Allocation/Writer/cancellation behavior comes from the
real operation, not this small numerical oracle.

Exact hashes:

```json
{
  "runtime_commit": "9bf8b6b1fa6a50b4951172cbae49c80470a8cdbb",
  "sources": {
    "quantization.py": "5f5a9bf2e2caefbe729bd88ee0e320c409dc29f1d3252b4128909124a360549c",
    "safetensors_io.py": "f5ef98c05ff4bcc00b5a8689323f215b088ee181370f02bf8a4172025344dd59",
    "internal/encoding/formats.py": "057e924d1b85abec9d7109196b0ec459a63991cd0a9aa3b2e97a22125452f023"
  },
  "oracle_sha256": "847886fd6be13bbd27c1bbf1ba3b0f437edb47722d4a2f7a842b3a03cc67ed79",
  "verbatim_reference_sha256": {
    "RowwiseDequant.historical.txt": "5919bc8894be68322739f5047732bccec8d2b1f192829ad87b0aa2b256a431f4",
    "normalization-functions.historical.txt": "1cc9a77649dbb11d2b4b5782786026101d666dfc397263a8648afb81f8d18914",
    "quantization.py.historical.txt": "5f5a9bf2e2caefbe729bd88ee0e320c409dc29f1d3252b4128909124a360549c",
    "safetensors_io.py.historical.txt": "f5ef98c05ff4bcc00b5a8689323f215b088ee181370f02bf8a4172025344dd59"
  }
}
```
