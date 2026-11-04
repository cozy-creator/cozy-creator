# Capture ownership follow-up

Feature3630938f9 corrects a reproduced new capture-binding mutation path:
`ordinary_out.set_(managed_weight); ordinary_out.zero_()` changed the prepared CPU
storage. Red receipt is preserved. Capture now refuses reassignment before mutation
and protects prepared ordinary weight aliases in the read-only guard.

C10 Storage weak tokens monitor each actual exported storage family, including
ordinary detached aliases. A still-live same Storage is retained strongly with its
native TensorExport/Pin, and the epoch refuses streamed classification. No ordinary
unpinned Tensor is manufactured and no reference count is forced. Native scalar
Pin census remains diagnostic/test evidence, not the atomic ownership capability.

All eight real CPU clients pass on the same verified native/feature overlay. No GPU
execution occurred. The actual enclosing native Plan suspend/restore observation was
added to graph_frontend_gpu.py but remains coordinator-owned/unrun. Unknown restore
state retains parent/epoch and the model's failed-intent stack. Host/nested replay
inside another capture/replay epoch refuses before new native work.

The pending scoped vt.graph helper amendment is still unapplied; torch.py and native
code are unchanged. All earlier evidence remains preserved.
