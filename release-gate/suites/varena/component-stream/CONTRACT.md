# Eager component stream contract

Owner `/root/p3_trim_qualification_nov3` owns only `_offload_stream.py` and these
clients. Agent1 owns `_offload.py`/automatic-graph wiring. This helper is inert
until that shared integration is composed; it is not a performance qualification.

```python
with component_scope(device_owner, flat_inputs, capture_bindings=binding) as scope:
    result = auto_or_eager_on(scope.stream)
    if cold_native_sample_is_open:
        witness = scope.checkpoint()  # outer component remains open; no handoff
        if witness is None or not witness.can_train:
            mark_sample_unqualified()
        else:
            witness.wait()  # actual event completion + successful query
            assert witness.checked
            finish_native_cold_sample()  # before the next sibling runs
    result = scope.output(result)
return result
```

`device_owner` is the actual `_Device`. Ordinary operands plus any ordinary
buffers the wrapper introduces belong in `flat_inputs`. The owner RLock covers
the scope. An implicit0/1 caller uses the existing owner stream; an explicit
caller keeps its actual stream. The outer scope orders caller input -> selected
stream once, then selected completion -> caller once. Normal return is still
asynchronous; `wait()` is explicit measurement/consumer behavior.

`reuse(owner, current_stream)` is a small `_Device.execution` seam. Call it only
after existing validation, including current capture refusal. True permits
skipping the duplicate Torch stream context and joins. **Keep all existing
ordinary-input record_stream calls, materialized-weight records, native
exports/Pins/fences and error handling.** An ordinary out/alias operand can be
introduced inside the body and disappear there; outer arguments alone do not
prove that lifetime. Direct managed reads outside a scope return False and retain
the existing safe path.

Same-owner/same-current-stream nesting reuses the context and has no additional
caller input/output handoff or automatic event at exit. Cold measurement alone
calls `scope.checkpoint()` while the outer scope stays open: it uses the same
tracked-stream join/event implementation as the outer return handoff. A witness
must be waited or positively queried before native phase completion; `can_train`
alone is necessary eligibility, never proof of completion. Warm paths do not
request checkpoints or acquire their events. Nested intermediate inputs/outputs
are released at that nested exit, not retained until the outer return.
An observed different actual stream returns False for reuse: run the old safe
per-operation path on that stream. Known tails are joined at the return boundary,
and the scope plus nested descendants are marked `can_train=False`. Explicitly
unreported side work retains normal Torch caller ordering obligations; this
helper does not intercept arbitrary CUDA submissions. Automatic eligibility must
still use Agent1's exact adopted `_ForwardOrigin(installed, original)`, never
`__wrapped__` as provenance.

`scope.completion` is initially None and is supplied only for a final cross-stream
handoff (or host completion). Same-stream/nested exits leave it None. The explicit
open-scope checkpoint is the cold observation API; a closed scope refuses a new
checkpoint. Failed scopes invalidate their issued witnesses. CPU completion is
host-only and cannot train CUDA; capture-binding checkpoint returns None.
Capture bindings bypass
all eager stream queries, joins, events and tensor lifetime records and preserve
the binding's actual frozen stream/export behavior. The existing capture branch
in dispatch must still bypass eager execution.

`scope.output(result)` returns the identical result and records ordinary output
and alias lifetimes on the caller. It flattens registered pytrees. Opaque outputs
must supply exact leaves: `scope.output(decoded, values=(decoded.sample,))` keeps
a Diffusers return object unchanged. VAE encode's Gaussian distribution requires
its actual parameters/mean/logvar/std/var tensors, not just an unexamined container.
Unknown opaque CUDA-scope outputs refuse instead of silently losing their storage
consumers. Managed-weight aliases remain managed Versions and follow their existing
read path; no eager materialization is introduced.

For a later explicit consumer use
`scope.completion.handoff(stream, output_or_alias, values=...)`: it waits the
private producer event and records those actual ordinary tensor leaves on that
consumer stream. It returns the identical object. This does not predict future
consumers; further streams must be declared just as ordinary Torch requires.
Completion holds no output tensors or weight Pins. Scope input/output references
are cleared at exit, including errors; Torch stream and ContextVar state restore.

## Coverage and checks

- `cpu.py`: actual adopted host model, nested same-owner reuse, output alias
  identity, explicit opaque leaves, checkpoint while the outer scope stays open,
  failed witness refusal, exception state restoration and the existing direct
  managed-weight read path. No fake CUDA object.
- `gpu.py`: actual provider HOST gate, checked pending/completed event, explicit
  caller and actual stream-change behavior, nested context-count gate requiring
  Agent1's wiring, escaped output view consumed later while64 same-size allocations
  try reuse, exception restoration and direct weight read. Provider/core identity
  and source/binary hashes are required. No GPU execution has occurred here.
- `capture_gpu.py`: actual graph frontend/_CaptureBindings, actual CUDA capture and
  replay/readback. It requires Agent1's composed graph source and checks eager
  witness absence while bindings are active.
- `vae_gpu.py`: actual tiny Diffusers AutoencoderKL public encode/decode methods,
  explicit distribution/sample output leaves and reference comparison. Root forward
  call count must stay zero. This is method coverage, not root-forward, full SDXL or
  automatic wrapper qualification.

The first CPU pass uses preserved audited native images with the current helper
source. It is host compatibility evidence, not current paired SDK or GPU proof.
All GPU clients are prepared/unrun and need exact installed source/extension/shim/
core/std/provider/framework hashes and an owned external child deadline.

The26496 profile supplies only the measured motivation:21,925 per-op execution
scopes and43,850 nested wait_stream calls. This helper does not remove native
weight materializations, alter allocation policy, or establish a speedup. Qualify
unchanged actual model outputs and matched first/warm timing after integration;
preserve every failure and original profile/benchmark receipt.
