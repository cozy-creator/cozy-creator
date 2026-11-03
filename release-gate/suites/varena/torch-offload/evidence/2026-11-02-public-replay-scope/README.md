# Public replay-scope host proof

The agreed public wrapper constructs only the existing native replay guard, yields
and closes in finally. The adapter uses it instead of private _runtime.replay.
Actual native metadata guards verify nesting, exception cleanup and capture/replay
exclusion refusal without a CUDA device/graph or weight Pin. Real CPU binding-body
Torch/VAE checks also pass. This is not GPU completion, pool-tail or residency proof.
vt.replay and vt.graph source remain byte-identical; the separate scoped capture-helper
opt-out is still unapplied. Original receipts and frozen/canonical branches are intact.
