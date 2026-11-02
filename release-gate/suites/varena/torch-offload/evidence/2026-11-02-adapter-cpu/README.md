# Approved adapter: CPU and mixed-binding receipts

Implementation: `459a407f1a4473fb8b998b97771a4b79ab30641a`, on the approved native
exports plus coordinator correction `22586041aa1dfee93edd76d3ff3717f6e32409c7`.

- `adapter-cpu.json`: public `tensorfs.varena.torch.offload`, using the current
  Python package and freshly built production extension/core. Real CPU tied/direct
  reads, metadata and interior/escaped aliases, immutable dtype versions, refusals,
  TransformerEncoder inference and tiny Diffusers VAE encode/decode passed.
- `native-host.json`: actual production bridge metadata/refusal checks passed.
- `mixed-native-first.json`, `mixed-python-first.json`: actual C, external C-ABI Rust,
  internal Rust and Python bindings shared one mapped core, TLS errors and monotone
  native request sequence in both load orders. No GPU flag was supplied.
- `manifest.json`: exact source/import commits, commands and source/native hashes.
- Build logs retain both successful builds and the initial refused relative Python
  interpreter path. Earlier preparation receipts and all original `.task` artifacts
  remain preserved.

This is a built source-package check, using `PYTHONPATH=.task/host-package`; it is not
wheel installation evidence. No `LD_LIBRARY_PATH` was needed for the mixed clients.
Torch was `2.14.1+cpu`, Diffusers `0.41.0`, with one Torch/OMP/MKL thread and no CUDA
initialization or model downloads. The adapter CPU client verifies no mapped libcuda.

The authored `adapter_gpu.py` and `sdxl_gpu.py` remain unrun. Native Pin residency,
producer/consumer GPU fences, final reclamation, stock SDXL force_upcast/output and
separate constrained-memory/quality/performance qualification belong to the coordinator.
