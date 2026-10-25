# Authored recipe and encoded consumer proof

This is a real small linear model, not a mock:16 outputs ×256 inputs, BF16 source weights and
bias, authored Model/Loader/uses_components, module-level INT8/NVFP4 Recipes and actual GPU
encoded-linear inference. It is component qualification, not a substitute for a production model.

The controller uses only the normal default-home CLI:

1. `cozy run ./source` writes a granted/adopted source artifact on the selected machine.
2. `cozy model upload run:N#model <owned-private-model> --lane bf16` uploads that tiny artifact.
3. `cozy model quantize <source-commit> --lane int8|nvfp4 --package ./...` runs each authored recipe.
4. `cozy run ./predict model.model=<quantized-commit> model.weights=<same-commit> --gpus=1`
   binds the actual quantized model and an independently granted raw checkpoint view.

```sh
python3 release-gate/suites/tensorfs/recipe-quantize/run.py \
  --machine OWNED_MACHINE --model fidika/unique-native-recipe-proof \
  --seed 209401 --out /absolute/durable/receipts
```

Root must run this controller under its owned-machine/run guard. It never rents, updates an SDK,
uses a private interpreter, deletes a model, publishes a release or invokes package internals.
The named private repository retains proof commits; the controller does not delete unrelated work.

Acceptance is explicit: recipe output is uploaded/adopted into the requested lane; native source
read counts are8192 forConvRot and16384 forNVFP4 (bothpasses), newbytes4160/2308; stored bias is
inherited unchanged. Inference must actually consume an encoded leaf with the declared scheme,
not silently accept a decoded ordinary Linear. Golden weights come from the committed artifact
through the shared TensorFS decoder. Weight relative error vs originalBF16 is≤0.025ConvRot or
≤0.15NVFP4. Actual encoded GPU inference vs those decoded artifact weights must be finite with
relative error≤0.035 and absolute error≤0.025, allowing the provider's activation quantization and
BF16 rounding. The fixture records provider, exact artifact/output hashes and both error scores.
No per-step loop, model throughput or cross-hardware performance claim is made.

Both ordinary recipe/inference lanes passed the current cohort below. Use Runtime0.32.3+ with
TensorFS0.15.6+ after the owner-qualified native Writer/caller gates. Unsupported model/provider
or interface behavior must fail visibly, never be patched around with a private loading path.

First live attempt reached source adoption, private upload and native INT8 recipe conversion.
Consumer run17692 then failed before the handler because this fixture expected a `net` wrapper
around construction config. Runtime deliberately presents a single component's config directly;
the factory now uses its typed `as_int` accessors. Original private commits and partial receipts
remain retained; encoded inference and the NVFP4 recipe were still pending at that point.

Corrected fixture run17717 created the source,17718 uploaded its private BF16 commit,17719
converted/adopted INT8 and17720 **passed actual encoded GPU inference** on hatsuka. The consumer
used `cozy.int8-convrot256.native-leaf/1`; relative output error was0.008017 and maximum absolute
error0.003208. NVFP4 then refused before a run: the controller CLI compared only lowercase legacy
dtype names and rejected canonical `BF16`. Its narrow repair recognizes canonical BF16/F16/F32
as well as the old aliases, retaining rejection of encoded or unknown kinds. The latest cohort
below proves that repaired CLI and the NVFP4 consumer. [Original component proof and refusal](evidence/fd4ff48fa-int8-and-cli-refusal.json).

On 2026-10-25, source **17866**, private upload **17867**, INT8 conversion/consumer
**17868/17869**, and NVFP4 conversion/consumer **17870/17871** all completed through the ordinary
CLI on local CUDA. Normal update 17853 installed Creator 8499, Runtime 7d1 and TensorFS 9e95/0.15.8.
The [compact proof](evidence/8499-7d1-9e95-both-lanes.json) retains exact installed artifact hashes,
four frozen fixture hashes, private commits, actual measurement logs and twelve raw receipt hashes.

| Lane | Source bytes read | New encoded bytes | Weight relative error vs BF16 | GPU output relative error vs stored weights |
|---|---:|---:|---:|---:|
| INT8 ConvRot | 8,192 | 4,160 | 0.006981334 | 0.007108101 |
| NVFP4 | 16,384 | 2,308 | 0.086561672 | 0.002528441 |

Each conversion encoded one tensor and reused none. NVFP4's read count includes its complete
amax pass; the inherited 32-byte bias is excluded from new encoded bytes. These are logical
source/part counts, not measurements of physical disk or PCIe traffic. Actual consumers selected
`cozy.int8-convrot256.native-leaf/1` and `cozy.nvfp4.native-leaf/1`; maximum absolute output errors
were 0.002247199 and 0.000858374. A provider label does not establish native Blackwell instructions
or performance on this consumer GPU.

The prior attempt 17844 failed before its handler because scheduled maintenance held uv's cache
lock. This later normal submission passed without changing the fixture or native implementation.
The enclosing full-model run and normal SDK restoration were still pending when these component
receipts were extracted; neither is implied by this proof.
