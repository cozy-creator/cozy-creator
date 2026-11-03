# Actual 8 GiB stock SDXL fixture

This separate fixture adapts the recorded adopted-only pressure client without
changing its `run()` AST,20steps,1024x1024 output, seed1701, prompt, fp16 model,
stock force-upcast VAE or external uncapped reference/tolerances. The source
pressure client SHA256 is `f91bd33f1fe8deff8fc03265d269986242aeb99c3f298983d46d0c32a79734e4`.
That original remains unchanged.

The previous client required more than8GiB free before imposing a6GiB managed
cap; it cannot qualify an actual8GiB card. This fixture removes that cap and
oversize-cap probe. It checks the actual GPU UUID, exact CUDA device name and
visible total bytes against the operator's bound hardware receipt, and requires
an8GiB-class device. The provider SKU is an external quote/receipt binding,
explicitly distinguished from CUDA's actual hardware identity.

Model construction remains CPU-only until `vt.offload`. There is no full-GPU
baseline preload, application retry, tiling substitution, reduced step count or
precision change. One existing18-file local SDXL directory is verified and reused;
no model download occurs in the client. Its13MiB external reference array is
hashed before inference. Both complete outputs are compared with `rtol=atol=1e-3`.

The client records hardware, first/warm generation-to-NumPy times, native Working
and weight counters including fills/evictions, full output arrays and lossless
WebP previews,200ms NVML samples and the exact failure stage. The sampled physical
peak is not an exact peak; H2D bytes/calls remain null when unavailable. Owner-close
failure remains a failed result, and the original controller checks child exit.
This is isolated Diffusers/API qualification, not full Runtime model execution,
a benchmark, or proof for all8GiB hardware.

`prepare_hardware.py` only reads local files, copies the needed client/reference
files, records source hashes and generates UNBOUND input. It preserves the existing
`stock_pressure` controller entrypoint in a separate project, with its exact
identity/library/child-custody checks. It does not build, install, download or run
CUDA. The paired sources are supplied explicitly; native artifacts, machine and
hardware fields must be bound by the operator before execution.

```bash
python3 prepare_hardware.py \
  --seed <preserved-pressure-6gib-873-001> \
  --creator <creator-repo> --creator-ref 49d1ef73f8393fc5fb277c198d3187688078caf5 \
  --runtime <runtime-repo> --runtime-ref 2e98d9c48f51fcfb22b11a7b9932687de17e05e8 \
  --output <new-preparation>
cozy run <new-preparation>/project/stock_pressure --on <owned-machine> --gpus=1 \
  --input <bound-stock-hardware-input.json> --json --out <new-results>
```

A capacity failure at the original settings is preserved with phase/accounting,
not replaced by a smaller request. Schema/dependency/setup failure is distinct
from model execution failure. A managed6GiB H100 result never substitutes for
actual3060/3070/8GiB qualification. Public SDXL/Qwen, H3, independent GPU scheduling,
foreign-pressure and Ulysses remain separate gates.
