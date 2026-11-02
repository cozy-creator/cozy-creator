# Graph frontend qualification

The implementation is a fixed-module graph primitive. It requires the native scoped
`Use.wait_ready` feature and the hard-cut scoped `vt.graph` helper in this lane.
Preparation joins the actual caller onto the capture stream and synchronizes only
that stream. Other producers need explicit caller dependencies; there is no global
barrier/opt-out. Graph/pool GPU behavior is not qualified by the host checks.

Host frontend client, using a matching built SDK plus the exact Python feature overlay:

```sh
CUDA_VISIBLE_DEVICES='' OMP_NUM_THREADS=1 MKL_NUM_THREADS=1 HF_HUB_OFFLINE=1 \
  python graph_bindings_cpu.py
CUDA_VISIBLE_DEVICES='' python replay_scope_host.py
```

This runs actual tied/strided Torch and tiny Diffusers VAE computation through the
capture binding table. It checks exact eager values, zero new residency materializations,
read-only weight/buffer/input behavior, signatures, old aliases and empty interior views.
It uses no graph/native allocator substitute and makes no CUDA claim.
The replay-scope host client exercises actual native metadata guards, including nesting,
exception cleanup and opposite-guard refusal, without constructing a CUDA graph or Pin.

The coordinator must bind the exact native SDK, Python files, helper/Runtime hashes,
GPU UUID and ordinary-Cozy caller before running isolated GPU children. No script
installs, rents, downloads model weights, merges or releases. Payload commands inside
that owned ordinary-Cozy fixture are:

```sh
python graph_frontend_gpu.py --device 0
python graph_epochs_gpu.py --device 0 --provider "$matched_source_helper" \
  --provider-python "$matched_provider_python" --mode tail --timeout-s 60
```

Run separate fresh children for epoch modes `reset`, `source_failure`, `capture_refusal`.
The helper is the existing real C public-Source provider; use its matched `provider.py`
and library, including the new readiness controls. The gate is mapped HOST memory,
not a timed CUDA sleep or a fabricated resource. The coordinator owns the child deadline
and cleanup. Keep all stdout/stderr/artifacts on failure and do not lower assertions.

- Frontend: multiple live definitions, weight Pins zero, same-VA trim/refill/replay,
  independent old outputs, an escaped native alias on a graph Region, and stale-version
  selection refusal with earlier output/version preservation.
  It also observes restoration of a known enclosing offload Plan after segment replay.
- Tail: after actual launch, gate the real copy consumer; prove native trim selected
  a last-consumer fence and stayed blocked before opening the gate. No early reclaim.
- Reset: reset the actual CUDA executable in its launch-end hook while work is gated;
  independently open the gate, verify output, refuse new selection and preserve cleanup.
  The report distinguishes a blocking reset from a reset that returns while work remains
  pending; a Python thread marker is not completion proof.
- Source failure: an explicit fixture adapter binds a byte-identical immutable CPU
  recipe to the existing real foreign Source Region. Its native preparation error must
  propagate before capture, with one attempt and no unproven ownership. This does not
  implement general public Source-map adoption.
- Capture refusal: keep an unrelated actual producer pending. Scoped readiness must
  settle only the selected Sources. `graph_epochs_gpu.py --mode capture_refusal`
  delegates to `graph_capture_sync_gpu.py`; ship both files. The helper cell creates
  and warms graphs, buffers/copy kernels and the exact explicit pool/reserve BEFORE
  arming the unrelated producer gate, then passes that same pool/reserve to the helper.
  It requires native admission refusal while the producer stays pending, ordinary caller
  compatibility and live-pool preservation after failed admission. A real gated
  producer -> caller -> capture control checks dependencies and exact output. A real
  body exception checks capture/routing cleanup before the gated admission control.
  This is a helper
  regression, not a claim that fresh high-level capture preparation cannot wait.

Native pool preparation can still allocate/recover/trim/wait. Callers must prepare/order
Working inputs, buffers, RNG/library/workspace and dependencies, in addition to selected
Source retirement readiness. No globally nonblocking or zero-cost preparation is claimed.
See GRAPH-CALLERS.md for the read-only active caller audit preceding the hard cut.

Private Working pools remain resident while definitions/static owners live. Shared
pools and automatic eligible-boundary/version-cache selection are unfinished. Close
unused/invalidated definitions; the initial primitive does not make whole-model/8 GB
or performance claims. Only the weight lease lifetime is separated from the graph cache.
