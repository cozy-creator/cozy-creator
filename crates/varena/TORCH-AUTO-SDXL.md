# Reviewed stock SDXL automatic frontier

This is eligibility source and real component proof, not a claim that automatic
dispatch/replay or performance is implemented. Draft #2501 remains draft. GPU
execution belongs to the coordinator. The automatic controller is still unwired.

The inventory uses the four exact stock configs at SDXL base revision
`462165984030d82259a11f4367a4eed129e94a7b`. It constructs real, full-width library
models on the meta device without downloading weights or creating Sources/native
allocations. All 2,641 parameter paths and 178 declarations match preserved stock
census26446. Selecting the existing outermost frontier yields 83 boundaries;
95 declarations belong to a selected ancestor. This is a topology/source result,
not an inference result. Config blob digests, per-boundary paths and the census
comparison are in the accompanying evidence.

| Component / class | Frontier | Stock invocation eligibility |
| --- | ---: | --- |
| Text encoders: CLIPTextEmbeddings | 2 | Candidate; reviewed embedding lookup plus addition, max_norm disabled |
| Text encoders: CLIPEncoderLayer | 44 | Eager; Transformers output-collection hooks mutate an active collector |
| UNet: ResnetBlock2D | 8 | Candidate; immutable eval normalization/convolution/residual path |
| UNet: BasicTransformerBlock | 10 | Candidate; layer_norm, no GLIGEN/positional module/feedforward chunking |
| UNet: Transformer2DModel | 4 | Candidate only with actual stock return_dict=False tuple output |
| UNet: CrossAttnUpBlock2D / UpBlock2D | 2 / 1 | Candidate; checkpointing and FreeU disabled |
| VAE encode: DownEncoderBlock2D / mid ResnetBlock2D | 4 / 2 | Candidate child segments |
| VAE decode: UpDecoderBlock2D / mid ResnetBlock2D | 4 / 2 | Candidate child segments |

Thus the initial stock call contract admits **39 candidates**: all 25 UNet
frontier boundaries, six VAE encode and six decode boundaries, and two text
embeddings. **44 remain eager** for an identified framework effect, rather than
being misclassified from eval mode or a successful sample. The two text encoders
have 13 and 33 frontier boundaries respectively. Capacity, supported kernels,
actual cold aliases and native measurements remain additional admission gates.

## Exact implementation and effects

The review covers Diffusers0.41.0, Transformers5.22.1 and their transitive modules
on Python3.12. `_offload_sdxl.py` records path-independent actual code/default/
closure fingerprints. Exact imported class identities and recorded adoption
forward origins are required. Different implementations/bytecode decline;
package version strings and __wrapped__ annotations alone do not grant support.
Source hashes and symbol names in `sdxl-symbol-review.json` identify the inspected
implementations. Unknown custom classes/subclasses remain ineligible.

CLIP attention is restricted to the reviewed eager/SDPA functions; the selected
backend and actual function identity enter the automatic key. Diffusers attention
requires unchanged stateless AttnProcessor2_0 and its reviewed mask helpers. Custom
processors, adaptive normalization, FreeU, GLIGEN, checkpointing, NPU branches,
deprecated scale kwargs and unknown attention kwargs decline before a second
authored body. The reviewed SDPA enable flags join the dispatch key. Embedding
renormalization is rejected because max_norm mutates weights. No RNG rewind,
reseed, Python byte estimate, allocator policy or OOM execution retry is added.
No claim covers arbitrary monkeypatches of library globals or Python control flow.

The CLIP layer/attention forwards themselves perform deterministic eval tensor
operations. However Transformers5.22 installs persistent output_capturing_hook
hooks lazily, even though their collector is context-local. Stock
output_hidden_states=True activates that collector, appending inputs/results (or
None). Capture/warmup would repeat Python effects; simply replaying kernels would
omit descendant collection. This slice preserves those hooks and keeps the 44
layer boundaries eager. It neither deletes them nor fabricates hidden states.

## Outputs and VAE method entry

Actual stock candidate frontier calls return an ordinary Tensor or tuple of
ordinary Tensors. The four Transformer2DModel frontier calls use return_dict=False
in their stock callers; default return_dict=True object results decline. Independent
Tensor leaves must still pass actual cold C10 alias checks against all inputs,
parameters, buffers and one another. Repeated tensors and views stay eager. None,
opaque containers/distributions, unsupported/repeated container shapes stay eager;
the original cold result executes once and returns unchanged. Explicit
GraphSegment's independent output clones are a separate contract.

AutoencoderKL.encode/decode are component *entry scopes*, not whole-VAE graphs.
The six child segments in each path are the capture candidates. The reviewed
`vae_entry_leaves` seam supplies actual opaque return storage owners to the
existing component stream helper, preserving the exact original return object:
decode.sample and encode's distribution parameters/mean/logvar/std/var. It never
calls sample() to discover leaves or consumes RNG. The reviewed mode() returns
the existing mean. Tuple returns remain tuples; unsupported slicing/tiling,
Accelerate hooks or method overrides require another entry adapter. These
explicit entry scopes were exercised by the first client. The subsequent
COMPONENT-ENTRIES.md delta wires reviewed UNet/text outer scopes and explicit
VAE encode/decode method scopes by default, independently of the still-unwired
automatic graph controller. Host scope counts are not graph-selection evidence.

## Runnable gates and remaining automatic proof

On an installed matching SDK, with Diffusers/Transformers available:

```sh
python release-gate/suites/varena/torch-offload/sdxl_inventory.py \
  --recorded-snapshot /path/to/preserved-stock-census.json
python release-gate/suites/varena/torch-offload/sdxl_components.py --device cpu
# Coordinator only, prepared and UNRUN here:
python release-gate/suites/varena/torch-offload/sdxl_components.py --device cuda:0
```

The component client uses actual SDXL library components and a real
StableDiffusionXLPipeline with reduced widths/vocabulary/spatial sizes but the
same stock layer depths and all 178/83 declarations/frontier. It runs text,
denoise, encode and decode before/after real Source adoption, compares numerical
outputs, inspects every actual frontier invocation, verifies unchanged RNG and
tests actual processor/backend/config refusals and opaque VAE handoffs. CPU
does not initialize CUDA. GPU mode is a real eager component gate, not graph or
performance qualification. No native resource is mocked.

The remaining automatic frontend gate must run actual stock repeated denoising
after controller wiring: original cold call once, native measured preparation,
private bounded segment cache, observed repeated selection/replay counts per
UNet boundary, independent request outputs, exact parity, cache pressure and
weight pins/residency. Report matching settings and cold/steady timings against
eager. Component eligibility alone does not prove any of these. All source,
epoch-failure/reset/escape GPU and stock performance gates remain explicit.
