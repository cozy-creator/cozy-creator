# Reviewed stock SDXL automatic frontier

The controller is now wired and this follow-up enables only the reviewed framework
paths below. The preceding nn-only vertical6c7dffc71 remains independently
qualifiable. Draft #2501 remains draft; GPU execution/performance qualification
belongs to the coordinator and is unrun in this lane.

The Linux RC composes this activation as `29566077d` with the ownerless graph
resource payload (`5111318ca` + `b0d1c2d7f`) and required typed native overlap
support. Public `vt.offload(pipe)` therefore selects this controller by default;
there is no separate activation switch or caller reserve. Every selected epoch
uses scoped readiness and existing checked exports/tails. The actual stock client
below requires repeated automatic UNet replay rather than accepting eager output
as automatic coverage.

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
VAE encode/decode method scopes by default, independently of the separately
implemented automatic graph controller. Host scope counts are not graph-selection evidence.
The current follow-up now connects the reviewed invocation predicates to that
controller; it does not change alias/RNG/ownership or native byte policy.

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

The remaining automatic frontend gate must run actual stock repeated denoising:
original cold call once, native measured preparation,
private bounded segment cache, observed repeated selection/replay counts per
UNet boundary, independent request outputs, exact parity, cache pressure and
weight pins/residency. Report matching settings and cold/steady timings against
eager. Component eligibility alone does not prove any of these. All source,
epoch-failure/reset/escape GPU and stock performance gates remain explicit.

## Prepared real stock repeat gate

`automatic_sdxl_gpu.py` runs TWO complete calls on ONE actual LOCAL stock SDXL
pipeline/Offload/native GPU handle in the same process. Each call uses20 steps at
1024x1024 by default and a fresh CPU generator with the same seed/prompt/guidance.
The fixed local `fp16` weight variant is loaded once; there is no download. Run
separate matched ordinary-Cozy processes with --graphs eager and --graphs auto:

```sh
python release-gate/suites/varena/torch-offload/automatic_sdxl_gpu.py \
  --model /operator/local/sdxl --output /operator/results/eager --graphs eager
python release-gate/suites/varena/torch-offload/automatic_sdxl_gpu.py \
  --model /operator/local/sdxl --output /operator/results/auto --graphs auto
```

Each mode saves separate `call-1/output.npy` and `call-2/output.npy`, with a
`result.json` beside each and a combined top-level result. Per-call records retain
config hashes, numerical settings, native
selection/decline facts, component entries and actual CUDA event timings from
immediately before UNet through the scheduler callback. Total inference time
includes prompt/VAE/postprocessing and excludes file writes. Host submission
timings are separately labeled. An ancestor UNet timing hook executes once per
step, outside every selected child definition. No child hook/method is substituted.
Each call records before/first-step/final facts, cumulative counters plus per-call
capture/replay/entry deltas, and prior UNet definitions actually reused. Only the
first call requires fresh captures; the second may reuse its existing definitions.
Both require repeated cached denoise reuse in automatic mode; returning
an image eagerly cannot satisfy its gate. The second call crosses the stock VAE
upcast/downcast Version/intent changes with no owner close, reset, re-adoption or
manual cache clearing between calls. Actual process/owner/native-handle IDs are
recorded. The first complete result stays saved if the second fails, and failures
are rethrown without repeating authored inference. Output parity/quality and performance
comparison still require the paired receipts; a single mode claims neither.
First and same-owner warm-call inference times are separate and exclude file writes;
their sum excludes between-call receipt work. No speed pass follows from one pair.

Stock txt2img exercises decode, not encode; its report states the actual encode0
entry count. The separate real component client retains --graphs eager explicitly
for its parity/eligibility gate and exercises BOTH encode/decode without claiming
automatic graph execution. It now also checks the actual controller preflight on
all39 observed candidate calls/keys. Both CUDA clients remain prepared/unrun here.

## Controlled eager/automatic experiment

Question: does the connected segmented cache improve repeated denoising without
changing supported model behavior or losing reclaimability? The chosen product
design is default automatic segmentation with native measured admission and idle
cache retirement. `graphs="eager"` is the current diagnostic control, never an
old-ABI fallback or a second allocator design.

Use one exact RC/native/helper vector and matched GPU UUID/capacity, local model
revision/config hashes, Torch/Diffusers/Transformers, dtype, kernels, seed, prompt,
dimensions, steps, guidance and cache state. Interleave eager/auto arms; repeat warm
measurements and report sample count/spread. Report cold preparation separately
from steady steps and total inference; do not infer a general win from one pair.

Acceptance requires actual automatic captures/reuse, preserved output/container
semantics, finite outputs and roughly equal quality against native eager, with
numerical differences reported under the existing design §4.9/§6 contract. Model
bit identity is not the quality gate; copies/refills/lifetime assertions stay exact.
There must be no unexplained retained growth or lost pending backing. A speed claim
requires matched repeat measurements; regressions are investigated/adjusted on the
same RC rather than hidden by retrying a failed body or adding compatibility modes.
