# Actual VAE duplicate-export span validation

Post-quant and output-tail bindings now reject overlapping contiguous byte ranges
in addition to Storage identity aliases. Prefix already has the range check. The
fix is in isolated Runtime consumer files; allocator/admission/native policy and
all frozen/root worktrees are unchanged.

`host.py` uses real frombuffer objects over shared bytes and actual CPU DLPack
exports. All three consumer predicates detect full/partial overlaps, preserve
ordinary Storage-alias rejection, and allow adjacent independent ranges. The
post-quant/tail helpers explicitly refuse noncontiguous or cross-device operands;
no synthetic noncontiguous bounds are calculated. Prefix's caller already requires
contiguous same-device bindings. Actual byte writes demonstrate the shared memory.
This CPU proof does not cover GPU binding, exports or asynchronous ownership.

The ordinary-child `spans` entrypoint is prepared and **unrun**. It obtains each
actual prepared provider, admits its inventory, makes an actual CUDA DLPack export,
and requires bind to reject before the consumer execute method is called:

1. Post-quant result aliases the original FP16 input through distinct Storage.
2. Tail pixel result aliases a prefix of the projected-row allocation through a
   distinct DLPack Storage.
3. Prefix sine aliases cosine through a distinct DLPack Storage.

The fixture requires different Storage identities and matching pointers so another
failure cannot masquerade as coverage. It verifies unchanged bytes after binding,
unpublished failed selections and final allocator cleanup. Sentinel writes are
fixture initialization before the attempted bind, not VAE operation writes.
No provider execute is invoked. The source objects retain their real export owners
through readback and scoped completion. Same-child Request cleanup is separate.

## Operator commands (unrun)

Root first composes all three edge consumers plus the span fix and builds approved
Runtime/TensorFS wheels. Freeze the ordinary resolved project lock. Then:

```bash
python prepare.py \
  --runtime-wheel "$RUNTIME_WHEEL" --tensorfs-wheel "$TENSORFS_WHEEL" \
  --gpu-uuid "$GPU_UUID" --output "$NEW_RECEIPT_DIR"
cozy run "$NEW_RECEIPT_DIR/project/spans" \
  --input "$NEW_RECEIPT_DIR/input.json" --on "$APPROVED_MACHINE" \
  --gpus=1 --out "$NEW_RECEIPT_DIR/run" --json
```

The freezer refuses dirty source/existing output and derives strict identities from
actual wheels, including all three consumer modules. Preserve failed attempts.
Use the reported `provenance.pid` as `expected_pid` and reported `keys`, added to the
original payload, for the same child:

```bash
cozy run "$NEW_RECEIPT_DIR/project/cleanup" \
  --input "$NEW_RECEIPT_DIR/cleanup.json" --on "$APPROVED_MACHINE" \
  --gpus=1 --out "$NEW_RECEIPT_DIR/cleanup" --json
```

No GPU, rental, install, master merge or release occurred in this lane. These three
negative GPU cases do not replace edge numerical/allocation qualification, raw
Source retirement, asynchronous CUDA-error custody or full H3 quality/performance.
Full H3 remains 15 seconds, 362 frames, 1344x768, 30 steps and audio.
