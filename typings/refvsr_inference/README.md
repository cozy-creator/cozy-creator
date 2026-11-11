# RelayVSR converter types

These declarations cover only the construction and `nn.Module` surface used by
`relayvsr_census` in `crates/tensorfs-py/scripts/converters.py`. The converter
downloads upstream commit `e856773350f53036d825404c9ea9228e6dffadca`; its code is
not an installed dependency of the repository's type-checking environment.

- [`FlashVSRConditionNet`](https://github.com/kopperx/RelayVSR/blob/e856773350f53036d825404c9ea9228e6dffadca/refvsr_inference/models/flash/model.py)
  inherits `nn.Module`; its class factory accepts a named variant and constructor
  overrides, returning that model.
- [`WanTransformer3DModel`](https://github.com/kopperx/RelayVSR/blob/e856773350f53036d825404c9ea9228e6dffadca/refvsr_inference/models/wan/transformer.py)
  inherits Diffusers' model/config mixins and therefore `nn.Module`. The converter
  uses the mapping form of `from_config`, builds its low-quality projection,
  assigns `lq_proj`, registers a buffer and collects `state_dict` keys.

The factories' configuration values stay opaque here. Runtime validation and
the census against banked checkpoint headers remain upstream/converter work;
these stubs neither execute the models nor change conversion behavior.
