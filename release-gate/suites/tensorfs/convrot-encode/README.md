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
