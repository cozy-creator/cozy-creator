# Actual Runtime native demand retention

An ordinary Cozy GPU App uses the executor's genuine Context, Request and Memory.
It writes a real TensorFS checkpoint and loads a five-block pointwise model through
`Declared`, `Weights` and `Memory.load`. Each block adds a 4 MiB weight; a common
scalar stays in the root's normal ownership scope. No context, allocator, reader
or graph owner is mocked. This is a bounded consumer proof, not full H3/SDXL or
smaller-GPU qualification. A separate real tied-weight model asserts identical
Parameter objects inside its forward and requires the common native owner.

The fixture measures the actual first-pass working mapping peak and actual native
weight-region sizes. It derives two test capacities from those observations:
room for the complete model beside the observed working peak, and room for the
common weight plus two blocks beside that peak. Cold and warm calls use the public
`denoise_loop` and require exact expected output. Roomy calls retain every weight
and perform no second-pass source fill; the constrained calls must reclaim through
actual capacity pressure with zero `streaming_limit` eviction. Region addresses
remain stable. These capacities are fixture inputs, not production memory policy.

The same model is then retained through Runtime's actual `Binding.prepare`,
captured using `vt.graph`, and replayed with `vt.replay` inside the binding's real
`replay_use` consumer-fence scope. Native pressure cannot
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


## Actual GPU proof, 2026-11-01

Run **25019** passes all six Runtime groups on RTX4090 columbina, driver595.91.07,
Runtime **9b8d74e69 / 0.36.20+dev.g9b8d74e6**, TensorFS **9ccb36a24 /
0.17.16+dev.g9ccb36a2**, and fixture **f04438bb6**. Native clear-limit run **25017**
passes its three separate cells. Both full run records, structured results and an
independent assertion audit are in `evidence/2026-11-01/`.

- Actual measured working peak38MiB plus weight backing22MiB defines the60MiB
  roomy cap. The warm call records six native hits, no new fills, no store or host
  reads, and all22MiB weights stay resident at stable addresses.
- At48MiB, each pass reclaims12MiB through actual allocator pressure and retains
  10MiB. `streaming_limit` eviction remainszero; outputs remain exact.
- Actual Binding preparation/replay retains22MiB despite pressure. After graph
  and binding retirement, only the escaped4MiB weight remains pinned, and both
  weight and captured-output aliases remain correct. Dropping aliases permits
  complete ordinary weight reclaim.
- Changed source identity preserves the old alias while healthy new storage is
  filled. A failed Fill's escaped91-valued bytes stay unchanged through a healthy
  refill, with no published failed region or poison. The tied model retains the
  same Parameter object in both uses and its common owner.
- Live bytes return exactly to17,039,360 and mapped bytes to20,971,520 (the same
  two library workspace owners). Pending bytes, pool count and event handles are
  zero. Fixture code asserts zero resident weights after owner closure. No library
  handle reset/forced cleanup is used.

No second PID/request cleanup call is required for this fixture's advertised
contract: every model, binding, graph and escaped alias is retired within the
entry, and its componentless phases publish no request-owned leaf measurement
key. This does not claim independent request-end metadata lifetime qualification.

This is bounded retention/ownership proof, not full-H3/SDXL quality or speed,
smaller-GPU hardware, or complete P2/P3 qualification. The matched full-model arm
is tracked separately and was not replaced with these small cases.
