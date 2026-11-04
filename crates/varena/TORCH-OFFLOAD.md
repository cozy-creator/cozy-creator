# Torch and Diffusers eager offload

Draft adapter, implemented at the Python boundary; CUDA execution and stock SDXL
qualification remain pending with the coordinator.

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
As in stock Torch, the four convenience casts convert floating weights/buffers only;
complex values retain their dtype and imaginary parts. Explicit `.to(dtype=...)`
converts both floating and complex values. Ordinary writable `out=` destinations
remain mutable working tensors; managed weights may only be read.

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
byte budget, eviction policy or Python prefetch policy. Native admission and
reclamation remain authoritative.

Inventoried boundaries are registered as native Region sets. Each declared module
forward activates its native intent; nested forwards restore the enclosing boundary,
and normal/exceptional exit deactivates the outermost intent. Direct `.forward()` calls
are covered too. Reused modules execute under their first canonical boundary name;
all alias paths remain declared. Dtype changes rebuild the declarations using the new
immutable versions before the next entry. `owner.intent_facts()` reports declarations,
entry counts, pending refresh and active intent without registering or activating
anything. Declarations are first registered at forward entry; CPU mode reports
`native=False`.

The additive C ABI `vr_v1_plan_activate(plan, name)` / `vr_v1_plan_deactivate(plan)`
delegates to existing native Plan methods. Public Python and Rust Plans expose
`activate`/`deactivate`. These calls perform no fill/Pin or execution observation, and
refuse interference with an open execution observation on that Plan. No compute fingerprint or
successful H sample is fabricated. Program-order prediction and automatic execution
scheduling remain unfinished.

`owner.close()` disables module entry and conversion; adoption is permanent and does
not restore the CPU model. Escaped managed aliases still own their Source/device and
remain readable. Native Region/service destructors use the public deferred retirement
path; the adapter does not force reference counts or wait for context-wide completion.
Conversion and close must occur outside active declared forwards. A failed intent
cleanup retains its exact Plan/Region/Source owners and refuses new module entry;
explicit close can retry retirement.

An actual dtype-version change retires obsolete Plan custody before conversion commits,
so the final VAE downcast does not leave old Regions Ready solely for a stale Plan.
Tentative Parameter changes are rolled back if Plan close refuses; old Plan, versions,
buffers and intent state stay owned, and the native failure propagates. A prior setter
failure does not close the old Plan. No-op dtype calls preserve current intent. Escaped
aliases and current packed members retain their own owners; native fences and existing
reclamation determine when physical bytes can actually be released. Conversion does
not trim or promise an immediate drop in mapped bytes.

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
python review_cpu.py                     # stock out=/complex conversion regressions
python intent.py                         # actual CPU declaration/nesting/failure flow
python intent.py --device 0              # coordinator GPU: native phase view observation
python plan_retirement_cpu.py            # actual CPU transaction/native refusal boundary
python plan_retirement_gpu.py --device 0 # coordinator: packed retirement/alias/refill
python adapter_gpu.py --device 0         # coordinator GPU, matching native Torch shim
python sdxl_gpu.py /existing/local/sdxl   # coordinator GPU, no model downloads
```

The SDXL client compares a stock baseline and adopted pipeline, retaining normal VAE
conversion and Parameter identities. Its default two steps are correctness smoke;
model quality, constrained-memory behavior and performance require separate actual
GPU receipts.
