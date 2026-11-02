# Actual Torch offload clients

Ordinary Cozy accelerator entrypoints `intent`, `adapter`, and `stock_sdxl` own
fresh child processes by pidfd. The child uses stdlib-only file/package checks,
then executes the selected client without a wrapper Torch/TensorFS import or
warm-up. Original offload/install and actual stream selection stay in the clients.
Native core/std uniqueness, extension/shim hashes, loaded Python `samefile` and
hashes are checked afterward. Individual SDK file symlinks are supported.

Intent and adapter scripts are byte-identical to native source a247d24a3. The
stock successor adds an explicit variant and safetensors-only local loader, and
preserves baseline/adopted full `.npy` arrays, lossless8-bit WebP previews, and
separate generation-to-NumPy wall timings before the strict comparison. All
original assertion ASTs, fp16 dtype, seed1701, original run function and allclose
rtol/atol1e-3 remain unchanged. The original2-step default is correctness smoke
only. `stock-20-input.json` selects the same20 steps for both sequential passes;
one pair is not a performance qualification. Load/device-transfer/offload-setup
timings are separate from generation. Raw arrays are the numerical evidence;
previews only round/clip for display.

The parent returns real child files as ordinary Cozy File artifacts even if an
assertion fails; detail maps artifact indices to names and SHA256. Child stdout
and stderr are retained before its verdict. Cancellation terminates only its
owned child. No model or native artifact is downloaded/built by preparation.

The stock model is only the existing local18-file fp16 layout for
`stabilityai/stable-diffusion-xl-base-1.0` revision
`462165984030d82259a11f4367a4eed129e94a7b`. Every length, four safetensor SHA256 and
14 git-blob hashes are checked before loading; the model path is supplied in the
input. The loader uses `local_files_only=True`, `variant=fp16`,
`use_safetensors=True`, with offline Hugging Face settings. Files are not renamed.

Prepare from a clean frozen fixture with the actual paired wheels and the
operator's installed package versions:

```sh
python3 release-gate/suites/varena/torch-offload-client/prepare.py \
  --source-root "$FIXTURE_ROOT" --source-sha "$FIXTURE_SHA" \
  --tensorfs-wheel "$TENSORFS_WHEEL" --runtime-wheel "$RUNTIME_WHEEL" \
  --package-versions "$PACKAGE_VERSIONS_JSON" --gpu-uuid "$GPU_UUID" \
  --output "$NEW_OUTPUT"
```

`input.json` runs intent/adapter or stock2 smoke. Run stock20 only after the
operator's deterministic native/intent/adapter gates pass. Stock invocation is
subject to the rental owner's deadline and download authority. This source
preparation claims no GPU execution.
