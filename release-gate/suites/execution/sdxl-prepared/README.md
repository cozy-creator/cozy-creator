# SDXL prepared Euler correctness

This gate runs the **actual candidate package function** against a small, real Diffusers
UNet with SDXL-style text/time conditioning. It creates genuine deterministic weights,
writes a TensorFS checkpoint, resolves its normal raw-weight plan, and materializes it
through Runtime's production Backend and Placement. It uses neither a model mock nor a
replacement allocator. No full SDXL checkpoint is downloaded to the controller.

`prepare.py` copies `_prepared.py` unchanged from the selected packages worktree into a
new fixture build directory and records its SHA256 and package commit. The numerical
oracle is Diffusers' ordinary `scale_model_input` / `step`, with the same real UNet,
initial latent, conditioning and schedule. It does not duplicate the candidate Euler math.

Build and run through the normal CLI, after the matching Runtime 0.32.2 and TensorFS
candidate have been installed by the GPU owner:

```sh
python3 release-gate/suites/execution/sdxl-prepared/prepare.py \
  /absolute/packages-worktree/sdxl /absolute/durable/output/sdxl-prepared-build
cozy run /absolute/durable/output/sdxl-prepared-build/gpu \
  seed=64401 steps=2 guidance=1.0 --on local --gpus=1 --json
```

Run six separate requests: `steps=2/7/30`, each with `guidance=1.0` and `guidance=7.0`.
Each invocation submits exactly one complete prepared plan; the Runtime's request ownership
contract is unchanged. Each also performs the nonresident and unsupported-mode refusal checks
without another submission. Preserve one receipt per step/guidance pair.

Each new run needs a new seed to avoid an application memo hit. This fixture changes no
SDK, rents nothing and starts no comparative performance benchmark.

The gate requires:

- Bitwise latent equality for 2, 7 and 30 total Euler steps, with CFG disabled and enabled.
  The candidate runs one eager step, then the actual native remaining-step function.
- A resident interval is accepted, and capture does not mutate the caller's eager latent.
- The prepared definition releases its weight pins before the next phase.
- Evicted weights are refused without reads to make them resident for capture.
- An unsupported Euler prediction mode takes the existing eager fallback.

The separate prepared-owner gate covers owner hooks, fill/kernel generation, cancellation,
and request phase tracking. This numerical gate does not replace full SDXL validation:
before merging the package change, run identical 30-step SDXL requests from the baseline
package and candidate package on the same installed SDK/device, with CFG on/off, a baked
LoRA, a per-step LoRA fallback, and an 8 GiB streaming/capacity case. Preserve outputs and
check `denoise_native_steps` rather than inferring native use from completion. SDXL's
Model declares no sequence-parallel degrees; its existing admission is one GPU per call.

Source checks are not GPU proof. No passing run is recorded yet.
