# Torch and Diffusers eager offload

Draft adapter, implemented at the Python boundary; CUDA execution and stock SDXL
qualification remain pending with the coordinator. Native dependencies and lifetime
contracts: [TORCH-NATIVE-EXPORTS.md](TORCH-NATIVE-EXPORTS.md).

```python
import torch
from tensorfs.varena import torch as vt

model = torch.nn.Linear(8, 8).eval()  # materialized CPU weights
owner = vt.offload(model, device=0)  # before any CUDA initialization
with torch.inference_mode():
    result = model(torch.ones(1, 8, device="cuda:0"))
```

Retain `owner` while calling the model. A Diffusers pipeline is accepted through its
`components`; retain and call the original pipeline normally. `owner(...)` also
forwards to the original callable. No model or checkpoint is downloaded by adoption.

## Weights, aliases and conversion

Adoption snapshots complete typed CPU storages with `Source.from_tensor`, grouping
tied Parameter objects and shared-storage views. Parameters keep their Python
identity and user metadata, become frozen managed tensors, and retain shape, strides
and interior offsets. Enrollment rejects outstanding TensorImpl owners, weakrefs,
gradients, unsupported tensor subclasses/layouts and existing Accelerate/Diffusers
offload hooks before replacing weights. Adoption requires exclusive model access.

Managed views, transpose and detach aliases retain their immutable Source version.
A read exports a temporary typed Region tensor whose storage owns the native Pin;
views do not keep every weight permanently resident. Independent tensor computation
and explicit clones are ordinary mutable working tensors. Use `weight.clone()` to
obtain an independent tensor; raw storage, data pointers, NumPy/DLPack exports and
reinterpret/conjugate aliases require an explicit copy. Private tensor internals are
outside the public contract.

`module.to(dtype=...)`, `.half()`, `.float()`, `.double()` and `.bfloat16()` derive
immutable Sources and preserve original Parameter identity and ties. Escaped aliases
keep the old dtype and values. Root and child conversions support the normal SDXL
VAE upcast/downcast shape without disabling `force_upcast`. Conversion prepares all
new versions before rebinding and retains the previous versions on failure. Module
device placement is fixed after adoption; arbitrary `_apply`, memory-format changes
and external Source mappings (`source` other than `None`) need explicit adapters.

Mutable buffers stay ordinary working tensors. Repeated slots for one buffer remain
tied; distinct mutable buffer views sharing storage are refused. Buffer state is not
reconstructed as immutable weight bytes. Replacing model slots or changing module
structure after adoption is unsupported.

## Streams and errors

Explicit caller CUDA streams are honored. For a legacy default caller stream, the
adapter submits on an owned nonblocking stream: it waits for caller work, then the
caller waits for the submitted operation. The initial native export orders Source
production on that actual stream. Every materialized weight read calls `record_stream`
after submission; ordinary inputs and outputs also receive lifetime records when
crossing the owned stream. Stream objects remain retained by the version's device owner.

For an ordinary tensor produced on another explicit stream, the caller follows the
normal Torch rule **before reading**:

```python
consumer.wait_stream(producer)   # orders contents
value.record_stream(consumer)    # protects allocation lifetime
with torch.cuda.stream(consumer):
    result = model(value)
```

Recording lifetime alone does not establish ordering. Managed weight aliases dispatch
through their version again on each consumer stream, obtaining the native producer
ordering and consumer tracking for that read.

Unsupported mutation/training/custom operators raise `OffloadError`; public Source
and native allocation errors propagate unchanged. There is no application OOM retry,
byte budget, eviction policy or Python prefetch policy. Module boundaries are exposed
in `owner.facts`; automatic Plan scheduling and successful H training are not supplied
by this adapter. Native admission and reclamation remain authoritative.

`owner.close()` disables module entry and conversion; adoption is permanent and does
not restore the CPU model. Escaped managed aliases still own their Source/device and
remain readable. Native Region/service destructors use the public deferred retirement
path; the adapter does not force reference counts or wait for context-wide completion.

## Supported boundary and runnable clients

Supported scope is ordinary materialized CPU eval models, immutable eager ATen reads,
ordinary buffers, direct weight access, managed aliases and controlled dtype changes.
Gradient-bearing calls require an explicit inference/no-grad context; enabling weight
gradients and training are refused. Capture, compile/tracing, custom raw-pointer
extensions, tensor subclasses, meta/checkpoint adoption and advanced draft verbs are
unfinished. Schema write checks are supplemented by training checks and actual alias
detection; they are not a security boundary against private/custom code.

`device="cpu"` executes the same Source/alias/conversion frontend on real host
snapshots without installing CUDA. This is useful for integration; it proves no native
residency or GPU ordering.

Clients live in `release-gate/suites/varena/torch-offload`:

```sh
python adapter_cpu.py                    # installed package, Torch CPU + Diffusers
python adapter_gpu.py --device 0         # coordinator GPU, matching native Torch shim
python sdxl_gpu.py /existing/local/sdxl   # coordinator GPU, no model downloads
```

The SDXL client compares a stock baseline and adopted pipeline, retaining normal VAE
conversion and Parameter identities. Its default two steps are correctness smoke;
model quality, constrained-memory behavior and performance require separate actual
GPU receipts. Earlier CPU feasibility receipts remain preserved under `evidence`.
