# Isolated packing children

Prepare from the already proven ordinary-Cozy seed, preserving it in the new
output directory before adaptation:

```sh
python3 release-gate/suites/varena/torch-offload/prepare_packing.py \
  --seed /home/fidika/cozy_v2/outputs/xs138-offload-performance-followup/census-client-d3e8-001 \
  --source-ref "$packing_source_ref" \
  --output /home/fidika/cozy_v2/outputs/xs138-packing-cozy-20261102/unbound-001
```

The output is deliberately **UNBOUND**. Before execution, the coordinator must
bind the actual completed candidate's TensorFS version, native extension/core/
shim/std hashes, Python file hashes, unchanged Runtime version/file hashes and
actual GPU UUID. Bind the verified existing model directory only for the census.
Record the candidate closure in `ready.json` and the copied manifest; preserve the
original UNBOUND inputs. Neither this preparer nor the clients install or download.

Run only this sequence through ordinary Cozy after binding, saving each invocation
and all returned artifacts in a distinct directory:

```sh
cozy run "$packing_project/parts" --on "$packing_machine" --gpus=1 --input "$packing_bound/parts-input.json" --json --out "$packing_results/parts"
cozy run "$packing_project/packing" --on "$packing_machine" --gpus=1 --input "$packing_bound/packing-input.json" --json --out "$packing_results/packing"
cozy run "$packing_project/stock_sdxl_census" --on "$packing_machine" --gpus=1 --input "$packing_bound/stock-census-20-input.json" --json --out "$packing_results/stock-census-20"
```

The parent uses the proven `App`/`Context`/`Outputs` interface and launches each
unchanged script in a fresh `sys.executable -I -B` child. It preserves stdout,
stderr and output files on success or assertion failure, checks cancellation and
the existing explicit timeout, then proves child exit. Both parent and child
check Runtime identity; the child verifies the SDK before importing Torch and
checks loaded SDK files using `samefile`, native mappings/core identity, package
wheel RECORD hashes, exact Torch build, and the actual GPU UUID afterward.

Dependencies retain the seed's unchanged `pyproject.toml` and `uv.lock`:
Torch 2.14.0+cu130, torchvision 0.29.2, Diffusers 0.41.0, Transformers 5.22.1,
NumPy 2.5.4, safetensors 0.8.0, huggingface-hub 1.33.0 and Pillow 13.0.0, plus the
bound TensorFS and Runtime. `parts_gpu.py` and `packing.py` themselves need only
Torch/TensorFS and the standard library; neither needs model files.

The stock census script remains byte-identical: two baseline and two adopted
20-step calls, seed 1701, normal VAE upcast, five snapshots outside generation
timers, full arrays/WebP outputs and strict `rtol=atol=1e-3` checks. Only the
snapshot/partition helpers add packed-member observations. Native Region totals
remain deduplicated; `current_source_bytes` remains the original storage logical
total. A partial pack reports unknown old-member-or-gap bytes rather than inventing
alignment padding. Exact 256-byte gaps require complete current member coverage.

`packing_census_cpu.py` checks those metadata cases without importing any framework.
Prepared clients and CPU parsing are not GPU or physical-pressure qualification.

Use the exact composed candidate in `packing_source_ref`; keep the frozen product
and historical prepared artifacts unchanged. The maintained parts/packing clients
now contain five/seven actual GPU checks respectively, with no mocked native
resources. CPU packing checks exercise real Torch Sources/aliases and public
conversion refusals, not native allocation or injected publication rollback.

Additional public ABI clients can be built against that candidate's exact native
core and matching Rust runtime library directory (host commands below run no CUDA):

```sh
cc -std=c11 -Wall -Wextra -Werror -I crates/varena/include \
  release-gate/suites/varena/native-tensor-export/parts_contract.c \
  -L "$native_lib_dir" -Wl,-rpath,"$native_lib_dir" -lvarena -o "$client_dir/parts-contract"
LD_LIBRARY_PATH="$native_lib_dir:$rust_lib_dir" "$client_dir/parts-contract" host
VARENA_LIB_DIR="$native_lib_dir" cargo build --locked -p varena-client --example parts -j 1
```

The coordinator can run `parts-contract gpu 0` and the built Rust `parts` example
as native child payloads inside its ordinary Cozy gate. They are prepared, not run
here. The C GPU mode checks real constructor failures/unchanged outputs and valid
construction afterward; accepted close is explicitly not a final service-teardown
certificate. The Rust client keeps full byte/gap readback, immutable caller snapshots,
safe empty-input refusals, same-VA refill and final retirement checks. The maintained
Cozy `parts` entry exercises the corresponding actual C ABI negative cases plus
typed aliases, trailing empty stream ownership, graph/refill and native retirement.

Agent1's separate #2482 Plan-retirement fixture owns actual old-Plan refusal and
dtype publication rollback. Compose it and run its clients separately; the removed
mock checks and their preserved historical receipts do not qualify that path.
