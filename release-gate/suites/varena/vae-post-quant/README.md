# H3 post-quant provider qualification

This client qualifies a declared pointwise GEMM alternative to the actual biased
24-channel, 1x1x1 H3 Conv3d. It does not claim cuDNN algorithm/rounding equivalence.
The Runtime consumer is `internal/execution/native_vae_post_quant.py`; its prepared
dense dependency is frozen Runtime `5498a8f09e25672952abeb67ddccfc28205c3181`.
The coordinator composes shared exports/build wiring and exact native libraries.

The host client passed 24 actual CPU Conv3d comparisons (batches 1/3/7, tiny and
7x16x16 tile geometry, FP16/FP32 input and matrix storage, FP32 bias), exact payload
inventory, copy-only packing/unpacking dispatch, and 15 metadata/layout refusals.
Max absolute difference: 0.000244140625; comparison uses rtol=1e-3, atol=1e-3.
No CUDA initialization or driver load occurred. The actual GPU fixture imported and
constructed the real Diffusers H3 post-quant module on CPU successfully. Host
receipts under `evidence/host/` do not qualify CUDA query/execution or GPU allocation.

The ordinary Cozy child entrypoint `post_quant` extracts `post_quant_conv` from the
actual Diffusers `AutoencoderKLMiniMaxH3`, initializes only that module on device,
and uses the actual stock Conv3d reference under the same FP16 autocast policy.
Its 20 GPU cells comprise:

- 12 complete output comparisons: batch 1/3/7 and FP16/FP32 input/matrix storage.
- Two alias refusals before the operation's numerical writes.
- Two native minimum refusals before allocation or numerical writes.
- Two real allocator refusals after one completed output, with replay forbidden.
- Two actual Context-close refusals while a prepared Plan remains retained.

Preparation and execution allocator census, queried workspace, library/algorithm
identity, explicit stream completion, final allocator baseline and same-child Request
key retirement are checked. Failed receipts retain the original and cleanup errors.
A real managed allocation larger than the hard cap injects the post-write failure;
this is a fixture boundary, not an undeclared production allocation requirement.
Library-internal allocation closure needs separate driver evidence. Asynchronous
CUDA failure custody and raw Source retirement are additional integrated gates.

## Operator commands (unrun)

GPU/rental/install authority is not part of this source lane. The operator first
builds the composed Runtime with its existing prepared dense provider and new
consumer, and freezes a resolved project lock using the ordinary candidate workflow.
Use actual approved Runtime/TensorFS wheel paths and the assigned device UUID:

```bash
python prepare.py \
  --runtime-wheel "$RUNTIME_WHEEL" --tensorfs-wheel "$TENSORFS_WHEEL" \
  --gpu-uuid "$GPU_UUID" --output "$NEW_RECEIPT_DIR"
cozy run "$NEW_RECEIPT_DIR/project/post_quant" \
  --input "$NEW_RECEIPT_DIR/input.json" --on "$APPROVED_MACHINE" \
  --gpus=1 --out "$NEW_RECEIPT_DIR/run" --json
```

`prepare.py` refuses an existing output directory and dirty tracked/untracked source;
it derives identities from the exact wheels and freezes fixture file hashes. It
performs no installation or device work. Preserve each failed run and select a new
receipt directory for another attempt. Following the first run, create `cleanup.json`
from its original input plus the reported `provenance.pid` as `expected_pid` and
reported `keys`, and run against the same machine/child:

```bash
cozy run "$NEW_RECEIPT_DIR/project/cleanup" \
  --input "$NEW_RECEIPT_DIR/cleanup.json" --on "$APPROVED_MACHINE" \
  --gpus=1 --out "$NEW_RECEIPT_DIR/cleanup" --json
```

No GPU cell has run in this lane. The full outer VAE sequence still requires
LayerNorm, packing/assembly, prepared-block integration and whole-operation admission.
Keep the existing outer chooser and its `catch.uncommitted_scope` guard until that
coordinator-owned cutover is complete. The full H3 gate remains 15 seconds,
362 frames, 1344x768, 30 steps and audio; five seconds is smoke only.
