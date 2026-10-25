# Native int8 ConvRot encoding

Run this real CPU conversion cell through the installed SDK in the default Cozy home:

```sh
cozy run /absolute/path/to/convrot-encode/run seed=103 --on MACHINE --json
```

The installed TensorFS candidate must expose `raw.quantize`; the Runtime candidate must retain
the current `derive.microscale.encode_int8_convrot256` reference producer. This cell compares
their actual encoded weight and scale bytes, including zero/subnormal/outlier/tie inputs and
row-chunk invariance. It writes the encoded tensors with the existing Writer, reads their
stored parts back, checks decoded output and verifies refusal followed by a healthy retry.
The output records source-function and native-extension hashes.

This qualifies tensor arithmetic and Writer/readback integration. It does not switch the
production recipe, prove model quality, run a full native checkpoint job, prove cancellation
inside one encoder call or establish performance. No unit tests or mocks.

Installed CPU qualification passed on2026-10-25: run **17469**, combined source7e98dce7f,
TensorFS0.15.3 wheel f80ef0d9… / native126fd414…, Runtime67e2e0c10, TensorDca3849147,
NumPy2.5.4, on RTX3060 host quatre through the default CLI. All24 logical-dtype cases,
chunk sizes1/2/7, 11 refusals and existing Writer/readback passed.
[Exact receipt](evidence/7e98dce7f-cpu.json). This does not qualify the later measured
encoder/Writer.quantize checkpoint loop or production Runtime cutover.
