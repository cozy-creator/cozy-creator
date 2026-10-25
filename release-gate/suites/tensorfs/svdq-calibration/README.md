# Real SVDQ calibration and adopted component

This small real GPU gate uses an authored BF16 Linear with 128 outputs and 256 inputs,
actual Runtime Calibration hooks over eight known input batches, the existing SVDQ fit,
the public per-component Output/adoption path and a separately loaded encoded consumer.
It creates no mock, alternate allocator, private interpreter, SDK override or rental.

Run only after the measured TensorFS0.15.6 primitive and Runtime adapter have passed their
owner-reviewed gates. Use an existing owned machine and private model repository:

```sh
python3 release-gate/suites/tensorfs/svdq-calibration/run.py \
  --machine OWNED_MACHINE --model fidika/unique-svdq-component-proof \
  --seed 156001 --out /absolute/durable/receipts
```

The controller uses only the ordinary default-home CLI. It creates/adopts a source,
uploads the tiny private BF16 commit, invokes calibration with real model/weight grants,
uploads the adopted SVDQ result and runs two held-out GPU input batches. It preserves
records and private commits; it never deletes a model or publishes a release.

The calibration cell verifies measured input maxima and 128 captured rows. One actual
`svdq.fit` exposes its selected alpha and rounded BF16 factors. From those, the gate
reconstructs smoothing directly from known source weights and actual calibration maxima;
it never inverts rounded stored `pre_quant_scale`. The frozen independent NVFP4 producer
must exactly match the selected residual's payload and scale bytes. No SVDQ fit algorithm
is copied and no production function is patched or replaced.

The real public conversion repeats the deterministic fit into its granted output. Every
committed residual and companion byte must match that inspected fit, including declared
logical F4 geometry. Bias inherits unchanged. Missing calibration must refuse before
writing or adoption. The existing Runtime tripwires remain active. The fixture records
residual numerical facts separately from complete SVDQ calibration/weight errors.

The consumer decodes the stored value law independently in F64 and compares actual GPU
outputs both with that artifact and with the original floating linear. For these known
inputs the explicit acceptance bounds are relative error0.02 against the stored law for
decoded weight GEMM, or0.10 for native activation-quantized SVDQ, and0.20 against the original
floating output. These are proposed fixture bounds awaiting real execution/review, not a
universal SVDQ quality guarantee. Absolute error and exact output hashes are also retained.

On older CUDA GPUs, decoded consumption of the adopted SVDQ artifact is legitimate component
qualification and is labeled `decoded-weight-gemm`. It must not be described as native SVDQ.
Use `--require-native` separately on an SM100+ target to require the actual
`cozy.svdq-nvfp4.native-leaf/1` provider. This task provisions no Blackwell hardware.

This is source-only proposed qualification until actual CLI receipts pass. It is not full
H3/image-model inference, a benchmark, cancellation qualification or a release claim.
