# Native SVDQ Writer fitting and adopted GPU consumption

The current gate requires native `svdq-nvfp4-r32-faer/1`, the Runtime adapter and the
new TensorFS artifact together. It first runs three ordinary CPU invocations against real
stored weights/calibration checkpoints, then the existing model-calibration and held-out
GPU consumer sequence. Nothing here installs an SDK, rents a machine or changes the daemon.
Use the primary Varena owner's coordinated validation window and default `~/.cozy` CLI:

```sh
python3 release-gate/suites/tensorfs/svdq-calibration/run.py \
  --machine OWNED_MACHINE --model fidika/unique-native-svdq-proof \
  --seed 256001 --out /absolute/durable/receipts
```

CPU cells cover F32/F16/BF16, zero/rank-deficient/outlier matrices, exact selected-residual
NVFP4 bytes, independent stored-value decoding, small exact-SVD reconstruction quality,
frozen Torch quality comparison, deterministic repeat under two workspace grants, actual
cancellation and same-Writer reopen/resume, changed calibration/seed refusal, finite/shape/
role/budget failures, and no tensor publication on quality failure. The component cell uses actual model hooks and the real Output API to fit component A
then B under one full-plan identity, without publishing B early. It also verifies finished
Output reuse and changed fitted/noncalibrated identity refusal. The fixture explicitly
grants 64 MiB. The adapter's representative Runtime allowance is 512 MiB, selected as an
explicit argument to Output preparation; neither is inferred from host RAM. The native
result reports required workspace and separate source/calibration logical read counts.
A faer call is not internally interruptible; the cancellation cell records checked join latency.

The GPU calibration cell retains real Calibration hooks and serializes completed statistics
through an ordinary temporary TensorFS checkpoint. Public Output conversion uses the native
Writer. The calibration digest, full resolved selection and native implementation hash enter
the derivation identity before finished-output lookup; the Writer independently guards resume.
The gate compares its result with the frozen pre-cutover Torch fitter, then verifies exact
residual packing for the factors actually selected. It publishes/adopts only the native result.
A separately loaded GPU model uses two held-out batches to compare the artifact with the
stored value law and original BF16 model. Existing decoded-weight and native-SM100 tolerance
boundaries remain unchanged; `--require-native` demands the latter. All claims require actual
successful receipts; a build or gate source check proves neither execution nor model quality.

The prior producer evidence below is retained as a historical baseline, not native fitting proof.

Native CPU Writer proof passed on 2026-10-25 with source76cfdcd34, fixture07012ed55,
TensorFS wheel SHA256e9679c401bc196749f52bbc76e3746626b1420ba66dbee89bfa6d4ab07f5af04,
Runtime237 and Creator51a. [The retained receipt](evidence/native-76c-18051-18053.json)
records exact artifacts and ordinary default-home runs18051/18052/18053. Run18047
first reproduced the binding's incorrect rejection of empty NumPy DLPack strides;
18051 passes empty write/reopen while retaining nonempty-strided refusal. Basic18052
now writes the empty[0,256] calibration and requires its intended native refusal.

Basic18052 passed six128×256 matrices under64/128MiB grants with identical bytes,
independent packing/reconstruction/served-score checks and15 refusal cases. The first
six-matrix fit/write call took0.636s and required1,047,111 conversion-buffer bytes.
For its F32 fixture, calibration error was0.08255654 versus plainNVFP40.13347861 and
frozenTorch0.08354279. Cancellation18053 joined in4.174ms, refused changed calibration
and seed, then reused its first tensor and fitted1024×1024 in2.734s with11,678,791
required buffer bytes. Process high-water RSS was390,012,928/478,683,136 bytes for
basic/cancel, including imports and independent reference work; it is not fitter-only
memory. Restore18054 verified the actual observed SDK and ready/idle machine.

These are CPU correctness/cost samples. The refreshed Runtime cutover, public finished
Output reuse and held-out GPU consumer still require the combined cohort's own receipts.

---

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
floating output. The decoded bounds passed the component run below; the native bound still
awaits execution. Neither is a universal SVDQ quality guarantee. Absolute error and exact
output hashes are also retained.

On older CUDA GPUs, decoded consumption of the adopted SVDQ artifact is legitimate component
qualification and is labeled `decoded-weight-gemm`. It must not be described as native SVDQ.
Use `--require-native` separately on an SM100+ target to require the actual
`cozy.svdq-nvfp4.native-leaf/1` provider. This task provisions no Blackwell hardware.

Actual local runs **17846–17850 passed** on 2026-10-25 after ordinary update 17843 installed
Creator 8499, Runtime 7d1 and TensorFS 9e95/0.15.8. The [compact receipt](evidence/8499-7d1-9e95-local.json)
retains exact artifact/fixture hashes, private checkpoint references and numerical facts.
Calibration 17848 collected 128 rows, selected alpha 0.6 and exactly matched the independent
residual encoding and all published parts. The public conversion read 65,536 source bytes
and wrote 44,040 new bytes; missing calibration refused before publication. Its calibration
error was 0.084519863 versus 0.137498856 for plain NVFP4 in this fixture.

The separately loaded consumer 17850 passed two held-out GPU batches on the RTX 4070 Laptop,
using `ordinary-linear` / `decoded-weight-gemm`. Relative errors against the stored value
law were 0.002347516 and 0.002345533; errors against the original floating output were
0.057906306 and 0.058644236. This does **not** qualify the native SM100 SVDQ provider.
The prediction request reported 23.133s of Triton `_table_kernel` compilation work, so these
runs do not establish steady-state performance. The separate recipe source 17844 failed
before its handler on a uv cache lock timeout and is not counted as passing recipe proof.

This is real component qualification, not full H3/image-model inference, a benchmark,
cancellation qualification or a release claim. Normal SDK restoration is owned and recorded
by the parent controller separately from the successful component receipts.
