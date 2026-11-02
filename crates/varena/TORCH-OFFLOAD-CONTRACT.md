# General Torch/Diffusers offload: adapter contract for coordination

Status: proposed, awaiting API/Source-owner agreement. This document does not declare
offload implemented or GPU-qualified. Native checkpoint: Creator #2397 at
`2ba51b29e5c84927c4a7d45337374d0ea8ff27b3`.

## Ownership and files

This lane owns a new `tensorfs.varena._offload` implementation, its documentation and
`release-gate/suites/varena/torch-offload` clients. The only proposed shared Torch-file
edit is a lazy `offload` export in `tensorfs.varena.torch`, after agreement. Public
`api.py`, `__init__.py`, `_runtime.py`, Source/native bindings, allocator/ledger and
retirement policy stay with their existing owners. No Runtime policy is copied here.

Proposed entry point: `vt.offload(model_or_pipeline, device=0, *, source=None)` returns
an owning offload handle while keeping the original module/pipeline callable. Adoption
is for immutable inference; training, in-place weight mutation and unadapted raw-pointer
extensions are refused. No application-code retry follows allocation failure.

## Required native/API agreement before shared-file edits

1. **Storage ownership:** a supported typed Torch/DLPack export from the canonical
   Region/Use path, carrying dtype, shape, strides and byte offset. Its storage deleter
   must retain a native Pin independently of the temporary Use/Python wrapper. A raw
   `Use.ptr()` and a Python tensor attribute are insufficient. Please provide exact
   callable signatures and the adopting implementation commit.
2. **Ordering and consumers:** export must order an in-flight Source producer before
   the specified actual consumer stream. Every later consumer is recorded before final
   native storage release. Current private `_VarenaUse.__dlpack__` assumes completed
   fills and ignores its stream argument; it is not an asynchronous export contract.
   Torch's pluggable record_stream hook reaches foreign DataPtrs, but current native
   working::record_stream_any ignores Region pointers. If ordinary aliases are allowed,
   the owner must supply Region/storage consumer routing, including interior views.
3. **Sources:** immutable host-backed model bytes and dtype-derived versions go through
   the Source owner's supported interface. Please specify how the adapter supplies
   CPU tensors/buffers without inventing file/checkpoint I/O, source callbacks on the
   allocator stack, or duplicate lifetime policy. Producer/staging/failure ownership
   remains native. Do not infer completion or publish a fill early from Python.

## Proposed Torch behavior to confirm

- Use metadata-only managed Tensor wrappers so direct weight reads re-enter dispatch.
  Hooks provide module/block intent; they are not the lifetime boundary. Preserve view,
  detach and transpose aliases as managed aliases. Ordinary non-alias computation
  results remain ordinary working tensors. Raw storage/data_ptr/DLPack escapes need
  their explicit owning-export contract or a clear rejection.
- Preserve tied slots and the original Parameter Python identity. Preflight the
  supported swap_tensors conditions; reject unsupported external weakrefs/use states
  before mutating model slots. Do not attach Parameter weakref finalizers that make
  swap_tensors itself invalid. Shared-storage views need consistent group descriptors.
- Reject writes based on dispatch/operator semantics, not `_version` polling or a
  supposed read-only DLPack flag. no_grad/inference_mode does not enforce immutability.
- Distinguish ordinary explicit tensor copies from controlled module dtype rebinding.
  Module.to/.half/.float must retain offload, identities and ties while old escaped
  aliases retain their prior version. This includes SDXL's normal VAE upcast/downcast;
  neither Runtime's rejection of post-placement conversion nor disabling force_upcast
  implements that behavior. Arbitrary `_apply` transformations are not silently allowed.
- Place adapter work on an owned nonblocking stream before submission, or honor an
  already explicit caller stream. Establish input/output event dependencies. Cover
  direct parameter operations and VAE encode/decode paths outside forward. Never send
  public Use the default stream implicitly or move queues after work was submitted.
- Inventory declared module boundaries (`_no_split_modules`, repeated layers and
  pipeline components), preserving duplicate module paths and aliases. Pass intent
  to the core; add no Python byte caps, victim selection, working reserve or prefetch
  sizing policy. Mutable buffers stay ordinary working state, not reconstructible weights.

## Proof and integration boundary

Local work can qualify real Torch CPU metadata, alias/identity, mutation rejection and
conversion behavior without allocating CUDA or downloading model weights. GPU tests
belong to the coordinator: exact tied/direct/escaped-view reads, source failure,
producer/consumer streams and final alias completion; SDXL VAE dtype cycle; real stock
Torch/Diffusers inference and the separate 8 GB/quality/performance gates. Capture and
custom access need explicit supported ownership; unsupported routes fail clearly.

No edits to the shared API/Source files are proposed from this lane. Please confirm
the entry point, alias/conversion/stream contract, file ownership, and concrete native
export/Source signatures before the shared Torch entry point is changed.
