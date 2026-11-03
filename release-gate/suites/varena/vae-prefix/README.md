# H3 input projection alternative and exact rotary prefix

Runtime `native_vae_prefix.py` uses the existing queried biased Lt provider for
24->2048 proj_in and preserves token/coordinate/RoPE packing round points. The key
`h3-prefix-lt-fused-bias/1` explicitly identifies a projection alternative: default
stock strided 3D Linear uses matmul then separate FP16 bias, while Lt fuses bias.
Packing first also changes GEMM layout/accumulation. No projection bit-identity or
stock rounding claim is made. `TORCH_LINEAR_FLATTEN_3D` may change the stock path.

`host.py` records the actual pinned Torch operator path and quantifies both packed
mm+separate-bias and packed fused-bias alternatives. Its first exact-identity probe
failed and remains in `evidence/host/prefix-path-001.stderr`. Following the explicit
quality-equivalence instruction, the fixed component gate was set before rerunning:
rtol=0.002, atol=0.0001 and relative L2<=0.001, input scale 0.125, matrix scale 0.02,
bias range +/-0.05. The relative term covers roughly two FP16 ulps and the absolute
term near-zero cancellation. It does not qualify arbitrary model activations.

Six real CPU dtype/batch cases pass (matrix FP16/FP32; B=1/3/7). The complete prefix
client `packing_host.py` reports max projection abs 0.00006103515625 and relative L2
0.0002412058092886582. Register/zero packing, positions, both separate FP32 RoPE
multiplies, duplicated angles, cosine and sine are bit exact. Every destination
keeps its pointer/shape/stride; exact payload inventory and five refusals pass.
A real distinct-Storage/same-pointer frombuffer repro confirms the span check.

The actual GPU fixture imports, constructs the real Diffusers decoder projection,
registers and frequency module, and runs its stock reference on CPU. The first
zero-reference smoke found undefined 0/0 in the report helper; that failed receipt
is preserved. The helper now reports zero error for two exact zero outputs and
infinite relative error for a nonzero result against zero. Bounds are unchanged.
No CUDA initialized or driver loaded in these host checks.

## GPU cells and remaining gates (unrun)

The ordinary-child `prefix` entrypoint has 14 cells: six complete full/ragged prefix
comparisons, two alias refusals before writes, two native minimum refusals, two real
post-write managed allocation failures with exactly one execution, and two retained
Context close refusals. It checks actual queried workspace/algorithm, complete
allocation census, exact register/zero/rotary values, direct projection-row packing,
owned-stream completion and final allocator baseline. Same-child Request cleanup
is a separate endpoint. The reference is actual stock H3 modules and package prefix
operations under matched CUDA FP16 autocast, not a mocked provider.

Every buffer is preallocated after admission. Shared rotary shape is `[1,1797,1,48]`;
hidden shape is `[B,1797,2048]`. Scalar host metadata is separate from managed GPU
payloads. Source-reviewed same-device copy/arange/structured out paths predict no
additional managed GPU allocation during execute; only actual GPU census can
qualify that claim. Vendor resources, raw Source retirement, asynchronous CUDA-error
custody and full H3 quality/performance remain unrun gates.

## Operator commands (unrun)

Root first composes/builds the approved Runtime/TensorFS wheels and freezes the
ordinary resolved project lock. Use a new receipt directory and the assigned GPU:

```bash
python prepare.py \
  --runtime-wheel "$RUNTIME_WHEEL" --tensorfs-wheel "$TENSORFS_WHEEL" \
  --gpu-uuid "$GPU_UUID" --output "$NEW_RECEIPT_DIR"
cozy run "$NEW_RECEIPT_DIR/project/prefix" \
  --input "$NEW_RECEIPT_DIR/input.json" --on "$APPROVED_MACHINE" \
  --gpus=1 --out "$NEW_RECEIPT_DIR/run" --json
```

The freezer refuses dirty source or an existing output and derives strict identities
from actual wheels. Preserve failed attempts. For same-child cleanup, add reported
`provenance.pid` as `expected_pid` and reported `keys` to the original input:

```bash
cozy run "$NEW_RECEIPT_DIR/project/cleanup" \
  --input "$NEW_RECEIPT_DIR/cleanup.json" --on "$APPROVED_MACHINE" \
  --gpus=1 --out "$NEW_RECEIPT_DIR/cleanup" --json
```

No GPU, rental, install, master merge or release occurred in this lane. Root owns
outer admission and block factoring. Shared catch/phase/native_vae/package caller
and old chooser are unchanged. Full H3 remains 15 seconds, 362 frames, 1344x768,
30 steps and audio; component tolerances do not replace that model gate.
