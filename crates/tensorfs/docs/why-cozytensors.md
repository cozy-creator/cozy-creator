# Why cozytensors

A weights file can contain valid arrays and still produce noise or fail during inference
because the loader interprets its quantized values incorrectly. Reading the bytes is
only the first step: the reader must know how packed weights, scales and zero points
reconstruct each tensor.

These nine reports from 2025–2026 show failures involving producer-specific quantization
conventions. They illustrate the problem cozytensors addresses today: making a tensor's
encoding part of the format, with specified decoding rules and required information.

Safetensors specifies array storage, while producers describe quantization through their
own [metadata conventions](quantization-metadata-history.md). Every integration must
recognize those conventions and apply their decoding rules correctly.

## Put quantization meaning in the format

The [encoding contract](../spec/cozytensors.md#6-encodings) gives each encoded tensor
an explicit scheme, parameters and named parts. A conforming decoder must reject missing
required information or unsupported parameters instead of silently choosing a default.
TensorFS implements these checks and decoding rules for the encodings discussed below.

The benefit depends on the source: recognized encodings can be imported and decoded;
ambiguous sources must be rejected until their encoding is established. The examples
state which outcome the current implementation supports.

## Quantization encoding failures

### 1. WAN scaled FP8 loses its weight scales

Report or fix published: **2026-03-31**.

A [ComfyUI WAN regression](https://github.com/Comfy-Org/ComfyUI/issues/13238) produced noise
while `scaled_fp8`, `scale_input` and `scale_weight` appeared in unexpected-key warnings.
The [fix](https://github.com/Comfy-Org/ComfyUI/commit/e2ddf28d78f190b27d136668a7dc15c7f0ec75dc)
ran legacy quantization conversion after removing a model prefix. The failure was in
recognizing the convention, even though the arrays were readable.

**Why cozytensors helps.** The scale is a required part of the encoded weight, so it
cannot be mistaken for an unrelated parameter and skipped by a conforming decoder.
[`fp8.tensorwise`](../spec/cozytensors.md#fp8tensorwise) joins FP8 `weight` and required
scalar `scale` in one logical tensor, decoded as `weight × scale`. The optional
`input_scale` concerns activation quantization and does not enter weight dequantization.
TensorFS [recognizes the legacy markers](https://github.com/cozy-creator/cozy-creator/blob/5108c1d11d78cb81a9866ae8f6035d7ae033444e/crates/tensorfs/src/import/carriers.rs#L571-L583) and
[tests their import and decoding](https://github.com/cozy-creator/cozy-creator/blob/5108c1d11d78cb81a9866ae8f6035d7ae033444e/crates/tensorfs/tests/quantized_import.rs#L510-L555).

A conforming decoder cannot silently drop the required scale. Correct source conversion
and a consumer using that decoder remain necessary; an existing ComfyUI loader does not
gain cozytensors support simply because the representation exists.

### 2. GPTQ v1 and v2 store zero points differently

Report or fix published: **2025-10-07**.

[vLLM's report](https://github.com/vllm-project/vllm/issues/26343) describes repeated `!!!`
when its `gptq_gemm` path treated a GPTQ v2 checkpoint as v1, despite the sidecar declaring
`gptq_v2`. V1 stores `zero − 1`; v2 stores `zero`. The
[fix](https://github.com/vllm-project/vllm/pull/26092) passed the correct zero offset.
Other vLLM backends already supported some v2 configurations, so the failure was path-specific.

**Why cozytensors helps.** Zero-point interpretation becomes a required property of
each encoded tensor, consumed by a shared decoder, rather than a sidecar convention
each backend must translate into its own defaults.
The [GPTQ schemes](../spec/cozytensors.md#gptqint2-gptqint3-gptqint4-gptqint8) require
`zero_offset: 0|1` alongside group and activation-order parameters. The
[importer](https://github.com/cozy-creator/cozy-creator/blob/5108c1d11d78cb81a9866ae8f6035d7ae033444e/crates/tensorfs/src/import/carriers.rs#L1026-L1058) maps the source format to that offset,
and the [decoder](https://github.com/cozy-creator/cozy-creator/blob/5108c1d11d78cb81a9866ae8f6035d7ae033444e/crates/tensorfs/src/format/decode/gptq.rs#L131-L158) uses it. For stored zero 3,
weight code 5 and scale 0.5, choosing v1 yields 0.5 while v2 yields 1.0.

Missing source format currently defaults to v1, so correct source metadata still matters.
The original checkpoint already declared v2: its failure demonstrates that a format
cannot force a buggy kernel to obey metadata. A consumer retaining encoded weights must
select or configure a compatible kernel.

### 3. LTX-2.5 NVFP4 lacks the loader's quantization markers

Report or fix published: **2026-08-11**.

A [ComfyUI report](https://github.com/Comfy-Org/ComfyUI/issues/15511) describes an official
LTX-2.5 NVFP4 checkpoint whose 3,528 scale tensors appeared as unexpected keys. With no
recognized header or per-layer quantization markers, the loader passed packed U8 weights
to an ordinary linear layer and failed at the first attention projection. ComfyUI supports
several marker forms; `comfy_quant` is one of them.

**Why cozytensors helps.** The logical weight and its packed representation are distinct,
and the scales are required parts of that encoding. A correctly imported tensor cannot
be treated as an ordinary half-width matrix by a conforming decoder.
An [`nvfp4`](../spec/cozytensors.md#nvfp4) logical tensor records its full floating-point
shape separately from packed storage, with required block/global scales and layout
parameters. A conforming decoder must use those parts or reject the tensor.

TensorFS [rejects undeclared scale-bearing carriers](https://github.com/cozy-creator/cozy-creator/blob/5108c1d11d78cb81a9866ae8f6035d7ae033444e/crates/tensorfs/src/import/carriers.rs#L1241-L1273)
instead of guessing NVFP4 semantics from shapes. Applied to the structure described in
the issue, that would prevent silent misinterpretation but would not produce a usable
model. Conversion needs authoritative producer information, followed by support for the
model architecture. This review did not import or run the cited LTX checkpoint.

### 4. H3's NVFP4/AWQ text encoder loses its smoothing factors

Report published: **2026-08-04**.

A [WanGP H3 report](https://github.com/deepbeepmeep/WanGP/issues/2066) described output
ignoring prompts, with incorrect language or garbled speech. Its NVFP4/AWQ text-encoder
path dropped per-column `pre_quant_scale` factors, while INT8 embeddings needed their
per-row scales applied. The [proposed fix](https://github.com/deepbeepmeep/WanGP/pull/2071)
restored those operations; the reporter confirmed improvement. The PR was still unmerged
when reviewed. A separate attention-kernel problem in later comments is outside this case.

**Why cozytensors helps.** The [`nvfp4` decoding formula](../spec/cozytensors.md#nvfp4)
includes the supplied `pre_quant_scale` factor on each input column. TensorFS's
[decoder](https://github.com/cozy-creator/cozy-creator/blob/5108c1d11d78cb81a9866ae8f6035d7ae033444e/crates/tensorfs/src/format/decode/nvfp4.rs#L79-L84) applies it when constructing the logical
weight; `int8.rowwise` similarly includes embedding scales. A consumer of the decoded
tensor receives those factors already applied. A consumer retaining packed weights must
honor them too. The importer must preserve a source factor that exists: the scheme also
supports producers that do not use one, so it cannot detect every incorrectly omitted factor.

### 5. FLUX NVFP4's packed dimensions are mistaken for logical dimensions

Fix proposed: **2026-08-04**.

A [ComfyUI FLUX.1-Fill report and patch](https://github.com/Comfy-Org/ComfyUI/pull/15273)
described a model failing on its first forward pass because configuration detection used
the packed U8 tensor dimensions. A logical 3072×3072 NVFP4 matrix occupies a U8 array of
shape 3072×1536: each byte stores two values. Inferring the model dimension from that
array halved the expected width. The proposed correction consults the encoding; the PR
was still unmerged when reviewed.

**Why cozytensors helps.** The [tensor record](../spec/cozytensors.md#4-header) declares
the logical shape independently of the packed part's byte count. The `nvfp4` scheme
specifies the two-values-per-byte storage. A consumer can inspect the correct logical
dimensions before decoding. This addresses quantization's physical-versus-logical shape
distinction; it does not require inferring the model's operators or axis semantics.

### 6. An FP32 destination causes FP8 block scales to be skipped

Report published: **2026-09-03**.

A [TensorRT-LLM DeepSeek/GLM indexer report](https://github.com/NVIDIA/TensorRT-LLM/issues/18658)
compared loaded weights against raw FP8 casts and correctly dequantized values. The
loader matched the raw cast: because the destination was an ordinary FP32 linear layer,
it cast FP8 values without applying the checkpoint's 128×128 block scales. Long-context
output degraded. The [proposed fix](https://github.com/NVIDIA/TensorRT-LLM/pull/18665)
dequantizes before loading that layer; it remained unmerged when reviewed.

**Why cozytensors helps.** [`fp8.blockwise`](../spec/cozytensors.md#fp8blockwise) binds
the weight and scale grid into one logical tensor. Its
[decoder](https://github.com/cozy-creator/cozy-creator/blob/5108c1d11d78cb81a9866ae8f6035d7ae033444e/crates/tensorfs/src/format/decode/fp8_blockwise.rs) applies the scales regardless of the
destination layer's arithmetic dtype. Reading decoded FP32 weights therefore means
dequantization, not a raw dtype cast. Current Qwen/DeepSeek carrier support and
[fixtures](https://github.com/cozy-creator/cozy-creator/blob/5108c1d11d78cb81a9866ae8f6035d7ae033444e/crates/tensorfs/tests/real_blockwise_mxfp4.rs#L141-L205) exercise this encoding; they do not
prove execution of the full checkpoint in the report.

### 7. A name-exclusion rule disables FP8 handling for the wrong layers

Report published: **2026-04-25**; linked fix merged **2026-04-22**.

[SGLang's Qwen FP8 report](https://github.com/sgl-project/sglang/issues/23687) described
incoherent output and scale warnings. The [linked fix](https://github.com/sgl-project/sglang/pull/23467)
corrected substring matching in `modules_to_not_convert`: an entry for `mlp.gate` also
matched `mlp.gate_up_proj`, so quantized dense weights were treated as unquantized.
The verified fix concerned matching module boundaries; the issue's initial loop-mutation
hypothesis was not the established cause.

**Why cozytensors helps.** Each weight declares its own encoding. A correctly imported
`fp8.blockwise` tensor is decoded with its scale grid without consulting a model-name
exclusion list. TensorFS's [carrier definitions](https://github.com/cozy-creator/cozy-creator/blob/5108c1d11d78cb81a9866ae8f6035d7ae033444e/crates/tensorfs/src/import/carriers.json#L139-L147)
join the Qwen/DeepSeek weight and scale. The inference engine must use that decoded
tensor or honor its encoding; replacing the container alone does not fix an unchanged
SGLang module-construction path.

### 8. QLoRA export drops NVFP4's required global scale

Fix proposed: **2026-08-04**; merged **2026-08-11**.

A [ModelOpt QLoRA export fix](https://github.com/NVIDIA/Model-Optimizer/pull/2064)
corrected a path that discarded `weight_scale_2` while removing PEFT wrapper names.
The remaining NVFP4 arrays lacked the global scale needed for dequantization. The patch
preserved the deployment tensors and added regression coverage, including a Qwen3 case.

**Why cozytensors helps.** NVFP4's global scale is a required `scale_2` part, associated
with the encoded weight. TensorFS's [scheme declaration](https://github.com/cozy-creator/cozy-creator/blob/5108c1d11d78cb81a9866ae8f6035d7ae033444e/crates/tensorfs/src/format/decode/nvfp4.rs#L31-L40)
requires it. A conforming decoder rejects the incomplete encoding instead of inventing
a scale or treating the packed values as ordinary weights. This is a refusal guarantee:
the format cannot recover a scale that an exporter already discarded.

### 9. An exporter derives scales from already-packed NVFP4 bytes

Fix proposed: **2026-07-31**; merged **2026-08-03**.

A [ModelOpt low-memory export fix](https://github.com/NVIDIA/Model-Optimizer/pull/2038)
described rejected checkpoints and repeated-punctuation output. The exporter treated
already-packed NVFP4 bytes as dense values, derived incorrect scales, and wrote only
half the required block-scale count. The fix reused the stored compression scales and
converted their layout while preserving the block/global-scale product.

**Why cozytensors helps.** The encoding distinguishes packed parts from decoded values
and defines the required scale grid for the logical shape. TensorFS's
[NVFP4 validation](https://github.com/cozy-creator/cozy-creator/blob/5108c1d11d78cb81a9866ae8f6035d7ae033444e/crates/tensorfs/src/format/decode/nvfp4.rs#L31-L38) rejects a half-sized scale grid.
That would catch the reported structural defect before numerical decoding. It cannot
repair scales computed from the wrong values or detect every incorrect scale array whose
shape happens to be valid.

## Scope of the guarantee

These examples concern reconstructing numerical tensor values. Model layout and the
operations that use those tensors still depend on the consuming model implementation;
the current format does not independently describe that computation.

A producer must identify the source encoding correctly, and a consumer must honor it.
An incorrect but structurally valid scale or parameter can still produce wrong values.
The format makes the decoding contract explicit; it cannot repair a wrongly declared
encoding or a kernel that ignores that contract.

## Sources and implementation evidence

The upstream incidents and current-code assessments were reviewed against primary sources
and TensorFS source at [5108c1d11](https://github.com/cozy-creator/cozy-creator/commit/5108c1d11d78cb81a9866ae8f6035d7ae033444e).
Existing tests were inspected; this documentation review did not rerun those tests or
reproduce the reported full-model workloads. The format contract, importer behavior and
downstream model execution are separate claims throughout.
