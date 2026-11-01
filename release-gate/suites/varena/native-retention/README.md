# Actual Runtime native demand retention

An ordinary Cozy GPU App uses the executor's genuine Context, Request and Memory.
It writes a real TensorFS checkpoint and loads a five-block pointwise model through
`Declared`, `Weights` and `Memory.load`. Each block adds a 4 MiB weight; a common
scalar stays in the root's normal ownership scope. No context, allocator, reader
or graph owner is mocked. This is a bounded consumer proof, not full H3/SDXL or
smaller-GPU qualification.

The fixture measures the actual first-pass working mapping peak and actual native
weight-region sizes. It derives two test capacities from those observations:
room for the complete model beside the observed working peak, and room for the
common weight plus two blocks beside that peak. Cold and warm calls use the public
`denoise_loop` and require exact expected output. Roomy calls retain every weight
and perform no second-pass source fill; the constrained calls must reclaim through
actual capacity pressure with zero `streaming_limit` eviction. Region addresses
remain stable. These capacities are fixture inputs, not production memory policy.

The same model is then retained through Runtime's actual `Binding.prepare`,
captured using `vt.graph`, and replayed with `vt.replay`. Native pressure cannot
reclaim bound weights. After graph and binding retirement, an escaped weight view
and an escaped captured-output view must still contain the correct bytes. Once
those aliases are dropped, ordinary reclamation must return their backing.

A real source transform changes identity while an old weight alias survives. The
new fill must use fresh storage and the old alias must retain its former contents.
A second case raises after an actual partial GPU write and escapes a view of the
failed Fill. That region must stay unpublished, the next demand must refill
healthy storage, and the escaped partial bytes must remain unchanged. No package
operation is retried by the fixture's production path.

The actual executor's asynchronous int8 readiness probe is joined before the
strict baseline. Fixture-owned allocations must return to that baseline, with
zero pending bytes/pools and zero resident weight pages. Borrowed library handles
and their retained workspaces are not cleared.

Generate an artifact-bound payload outside this source tree:

```bash
python make-payload.py /absolute/runtime.whl /absolute/tensorfs.whl \
  GPU-OWNED-UUID /absolute/results/payload.json
cozy run /absolute/frozen/native-retention/gpu --on OWNED_MACHINE --gpus=1 \
  --input /absolute/results/payload.json --json
```

Inspect both the CLI completion and the returned `passed` value. Preserve complete
run records and failures. Source/static checks do not qualify GPU execution.
