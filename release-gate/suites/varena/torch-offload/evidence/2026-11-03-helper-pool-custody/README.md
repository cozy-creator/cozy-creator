# Helper refusal: captured Working custody fixture amendment

Feature764afaaa311629c80574f91b6b3fbe64b142ba44 fixes a bounded proof gap, not an
observed production reclamation bug. The old graph copied only external weight/output
addresses; releasing unused pool backing could not affect that computation.

The corrected graph allocates actual temporary Working storage during capture and
uses it in copy/add/copy nodes. Its pointer differs from external output; a native
footprint must trace nonzero allocation in the explicit pool. Footprint backing facts
are historical capture metadata and are NOT claimed as current mapping/refcount proof.

The exact pool/reserve is warmed before arming the real unrelated Source gate. After
native capture admission refuses while that Source stays pending, immediate aggregate
Working pool/mapping counters must preserve the pre-refusal pool charge. Before ANY
subsequent pool preparation the cell checks the witness pointer/contents, drops its
Python Tensor, requires C10 Storage weak expiry, and completes its stream tail.
Existing outside-capture vt.trim must retain graph-owned mapping; vt.replay then
validates the native pool reference and executes the SAME captured-address graph with
exact output. No recreated unowned Tensor or mock resource is used. Only afterward is
another helper preparation permitted. After all graph resets, trim must release at
least reserved pool bytes as a positive reclamation control.

Counters are aggregate native Working facts in an isolated fresh child, with completed
unowned cache trimmed before the baseline. They are not mislabeled per-pool addresses
or GPU completion. Footprint identifies the actual capture pool/allocation; address,
contents, no surviving C10 Tensor ownership, trim behavior, native replay admission and
final release jointly test custody instead of incidental cached capacity.

Checks performed HERE: Ruff, formatting, Python syntax, source AST/dataflow/order,
git diff --check, and byte-identical product adapter/helper source. No CPU pool or
native substitute simulation; no native build/model suite rerun. The GPU cell remains
UNRUN and this receipt contains NO GPU/custody pass. Coordinator owns execution,
ordinary-Cozy run/deadline receipts and child cleanup. No machine/rental/native policy,
merge or release action was performed. Hard-cut helper remains7c60c40fc, without a
synchronize flag/probe/global fallback; no new native seam is required.

Rerun payload is the existing graph_capture_sync_gpu.py (or delegated graph_epochs_gpu
capture_refusal mode) with matched provider/library/device arguments, documented in
GRAPH-COZY.md. Preserve prior failed and unrun receipts.
