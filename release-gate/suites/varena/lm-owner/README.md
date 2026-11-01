# Real LM cache owner

This fixture creates a seeded small Qwen3ForCausalLM, writes every actual parameter to a
TensorFS file and loads it through the normal resolver, Backend, ModelRegistry and Placement.
Its decorated component method calls public `lm.generate`. There is no mocked inference,
replacement forward, fabricated completion or forced graph eligibility.

The recorded CPU comparison uses Torch2.14/Transformers5.17.0 with CUDA uninitialized.
Both sources return the exact reference tokens `[41, 54, 12, 16, 41, 13, 10, 14]` and rebuild
correctly. Active public progress observes one occupied row and one Region pin; discard
refuses. Control74cce leaves one Binding/two retained weight blocks after idle KV discard.
Candidate534023328 closes the Binding and leaves zero retained blocks. The candidate CPU
command passes `--require-closed`, so this is an assertion, not only a reported counter.
Evidence and exact LM/model/script hashes are in `evidence/cpu/`.

The GPU entrypoint `lm_owner` requires the real placed engine to capture and replay. Its
public progress callback queues a native shrink and observes active-owner refusal. Once
generation ends, ordinary Placement holds are released; native shrink must retire idle
graphs, KV and weight bindings. Escaped token output and an initialized KV prefix must
remain unchanged. A new generation must bind/capture again and allocate different KV
storage while the old alias remains live. Explicit `lm.release` must close the owner.

Final accounting is compared with the same placed module's derived buffers, input IDs and
shared library Domain still live. The Domain is initialized and primed before the baseline;
all generated outputs/KV aliases are dropped before comparison. The real registry/Backend
close must leave no registered models or pinned weights. This is a scoped owner baseline,
not a claim that the process has no library workspace or that every model object was GC'd.

The model has one layer, width64, four attention/two KV heads, head width16 and vocabulary64.
Its random checkpoint qualifies lifecycle and token equivalence, not pretrained-model
quality. This does not prove live-sequence preemption or paged KV interior-hole adoption.
GPU execution is pending; no package is installed or GPU rented by this lane.

Prepare a clean frozen checkout with exact candidate artifacts:

```sh
python prepare.py --runtime-wheel /absolute/runtime.whl \
  --native-candidate /absolute/native/candidate.json --output /absolute/new-payload
```

The default UUID is `UNBOUND`; the build/GPU operator must provide the actual UUID before
execution. The input pins Runtime module hashes, TensorFS version/extension and fixture
files. Dependencies pin the inspected Torch2.14/Transformers5.17.0 contracts. Runtime's
development candidate is required; this fixture does not invent a published release floor
for the new binding-retirement behavior.
