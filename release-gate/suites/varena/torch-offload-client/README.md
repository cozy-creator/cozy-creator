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

## Separate residency census diagnostic

`stock_sdxl_census` and `stock-census-20-input.json` select a separate, fixed two
baseline/two adopted20-step diagnostic. `stock_sdxl` and its strict script remain
unchanged. The diagnostic starts from the recovery fixed2+2 source
`17dc33458231aee6da1acc4bfcd529134b6d624fbb6a2b2892619fe1c7596a81`.
Its run function, seed1701, fp16 model/variant, normal VAE upcast/downcast, identities,
original comparison assertions and synchronization sites are preserved. Each pair
still records rtol/atol1e-3 failure and the final aggregate assertion still fails;
this diagnostic does not qualify numerical parity or performance.

Five private scalar snapshots are saved outside timers: after adoption, and
immediately before/after both existing adopted generations. No extra inference,
trim, GC, synchronization, per-operation log, or model download is added. The
unchanged native census helper maps current Versions with weak references and
existing Region handles. Old native owner attribution remains unknown.

Files `census-*.json` retain the full native/current-Version snapshots.
`census-partitions.json` is computed only from those serialized snapshots after
inference. It separates anchor current, noncurrent-ready, Retired, noncurrent-other,
other arenas, and finalizing physical bytes; requested/reserved/mapped totals stay
separate. Native Region identities are counted once. Finalizing logical/reserved
sizes remain unknown. Per-arena consistency labels and inventory generations are
retained; equal generations do not certify atomic device-wide state. Current ids
missing from the sampled inventory or overlapping Retired state are reported.

The partitioner is also independently usable after ordinary artifact retrieval:

```sh
python3 "$PROJECT/ci_varena_offload/bin/partition_census.py" \
  "$ARTIFACTS"/census-after-adoption.json \
  "$ARTIFACTS"/census-adopted-1-before.json \
  "$ARTIFACTS"/census-adopted-1-after.json \
  "$ARTIFACTS"/census-adopted-2-before.json \
  "$ARTIFACTS"/census-adopted-2-after.json \
  --output "$NEW_PARTITION_REPORT"
```

Preparation hashes the diagnostic and both helpers along with the original scripts.
Use only the operator-closed matching native/PyO3 c561 census wheel and preserved
paired Runtime wheel. Binding its exact hashes must wait for that artifact closure.
Keep `--gpu-uuid UNBOUND` until a separately authorized existing execution target is
known; the former model/rental path is not evidence of live weights or authority.
No new model download or rental is authorized by this recipe.

Once the operator has verified matching installed bytes, existing local HF revision
462165984 fp16 weights, and target identity, the sole new GPU case is:

```sh
cozy run "$PREPARED/project/stock_sdxl_census" --on "$EXISTING_MACHINE" \
  --gpus=1 --input "$PREPARED/stock-census-20-input.json" --json --out "$NEW_OUTPUT"
```

The original controller still publishes child arrays, previews, census and
partition files through ordinary File artifacts even if the numerical assertion
fails. The runner checks all diagnostic/helper hashes before importing frameworks.
Source checks: `python3 -B check_census_source.py` checks the frozen AST contract,
fixed call/timing structure and a synthetic scalar partition without importing
Torch/TensorFS or running a model. No GPU diagnostic has been run by this lane.
