# Conversion-time stale Plan retirement

Required product: `0af634eff39c1a28ea587e1b69516f9dc326d142`.
Context import: `8a18b3bf7efd3a066ac8537813a0182429d52495`.
**Transferable patch:** `3b7f30affda313560a2be01045988be431421410`.

The production change is 8 added lines / 1 changed line in `_offload.py`. Within the
owner lock, it prepares versions/buffers, tentatively rebinds Parameters, then closes
obsolete Plan custody before committing. A close refusal rolls Parameters back while
retaining the same Plan, old versions, buffers and prior intent state. A setter failure
precedes retirement. Successful version changes clear the old Plan and mark intent
for reconstruction; no-op dtype calls preserve it. Legitimate aliases and current
packed members retain their ordinary owners. No native, trim or byte-policy change.

## Executed proof

- `regression-red.json`: against the frozen exact 0af package, the corrected real
  regression client fails retirement/refusal/no-op checks; setter rollback already
  passes. Baseline did not call native close (explicitly recorded).
- `plan_retirement_cpu.json`: 4/4 pass. The close-refusal case calls the production
  native ABI with an actually absent handle and checks exact ownership rollback.
  A one-shot real Tensor `.data` setter refusal checks earlier publication rollback;
  no allocator doubles are used. Closed public Plan wrappers check host metadata;
  they do not pretend to be CUDA Plans.
- Eight existing CPU clients pass: packing, packing metadata, adapter, review, intent,
  inventory, Diffusers inventory and CLIP inventory. Existing packing reservation
  doubles remain explicitly metadata-only. Transformer/tiny VAE inference is real.
- Actual C/external Rust/internal Rust/Python clients pass native-first and Python-first
  load orders, sharing one mapped core, TLS errors and native request sequence. C was
  rebuilt against the exact 0af header/core; Rust used its exact prebuilt helper.
- Ruff check/format, Python AST and diff checks pass for the patch and new clients.

Torch 2.14.1+cpu, Transformers 5.22.1, Diffusers 0.41.0; one worker, no model downloads
or CUDA initialization. The package is an owned extracted 0af wheel plus only the
verified `_offload.py` overlay. This is source-package/host ABI proof, not fresh wheel
installation or a mixed-current-master native build. Manifest records exact source,
core, extension, wheel and dependency identity; all old receipts remain preserved.

## Coordinator GPU gate

`plan_retirement_gpu.py --device 0` is prepared, syntax-checked and **unrun** here.
It requires actual automatic old-Plan close, retirement of an unescaped packed group,
protection by an escaped ordinary native Tensor Pin, old managed-alias refill after
physical eviction, new dtype intent reconstruction and final alias retirement.
Explicit native trim in this fixture is a causal test control, not product behavior.

The earlier 725,614,592-byte census correlation is in
`outputs/xs138-p4-packing-review-20261102`; this CPU proof does not claim those GPU
bytes have already been physically freed. Retirement makes unneeded Regions eligible
for the existing native path; it does not force immediate unmapping. Native parts,
packing, stock-model and all actual GPU qualification remain coordinator-owned.
