# Quantization metadata workarounds

Safetensors describes each stored array's dtype, shape and byte offsets, and permits
a string-to-string `__metadata__` map. Its [format](https://github.com/safetensors/safetensors#format)
does not standardize how packed weights, scales and quantization parameters relate.
Producers have supplied that missing agreement through sidecar configuration files,
custom header metadata and specially named tensors.

## Metadata stored as tensors

- ComfyUI's [MXFP8 format](https://github.com/Comfy-Org/comfy-quants/blob/main/docs/formats/mxfp8.md)
  stores `<layer>.comfy_quant` as a `uint8` tensor containing JSON such as
  `{"format": "mxfp8"}`. Its loader recognizes that special name, parses the bytes as JSON,
  and interprets `<layer>.weight_scale` using ComfyUI's particular block layout.
- [bitsandbytes](https://github.com/bitsandbytes-foundation/bitsandbytes/blob/main/bitsandbytes/functional.py)
  packs non-tensor quantization properties into a tensor under a key such as
  `<layer>.weight.quant_state.bitsandbytes__nf4`. Its loader looks for that naming convention
  and unpacks the metadata to recover properties such as the logical dtype and block size.
  Transformers also publishes [quantization configuration alongside the weights](https://huggingface.co/docs/transformers/main/en/quantization/bitsandbytes).

## Conventions adopted in 2024–2026

- **January 2024:** Transformers [v4.37.0](https://github.com/huggingface/transformers/releases/tag/v4.37.0)
  supports saving and loading bitsandbytes 4-bit models, including their producer-specific
  quantization-state tensors ([integration](https://github.com/huggingface/transformers/pull/26037)).
- **October 2024:** ComfyUI v0.2.4 [reads scaled FP8 checkpoints](https://github.com/Comfy-Org/ComfyUI/commit/a68bbafddb93d6f111af3d1fc22d3e38ce8186de):
  per-layer `scale_weight` tensors, flagged by an empty `scaled_fp8` tensor.
- **November–December 2025:** ComfyUI v0.3.68's [mixed-precision system](https://github.com/Comfy-Org/ComfyUI/pull/10498)
  records each layer's format as `_quantization_metadata` JSON in the file header; v0.4.0
  [stores it](https://github.com/Comfy-Org/ComfyUI/pull/11000) in a `<layer>.comfy_quant` tensor instead,
  while converting the older forms on load.

## What cozytensors changes

These conventions can be documented and reliable within one ecosystem. The difficulty
is that a generic safetensors reader has no standard rule connecting those entries to
the weights they describe. Two tools can read every byte correctly and still disagree
about the encoding. [Why cozytensors is needed](why-cozytensors.md) shows recent failures
that explicit quantization encodings and their decoding rules can address.

The [cozytensors encoding contract](../spec/cozytensors.md#6-encodings) makes the scheme,
parameters and named parts properties of each encoded tensor, with specified decoding
rules. An importer still has to interpret the original producer's convention correctly;
once imported, readers can use that common contract.
