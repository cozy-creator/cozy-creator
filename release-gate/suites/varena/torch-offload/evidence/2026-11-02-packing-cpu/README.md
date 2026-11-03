# Packing frontend CPU checks

Source context: integrated Creator `d3e8b36512a8456efb13b9efcbaa8d4f65de30a1`.
Dependency: native `Gpu.region_parts` implementation
`e1be8f315d2ed48b769971f9d644a4302109c1fd` must be integrated and built for CUDA.

The five receipts were produced sequentially with Torch `2.14.1+cpu`, one
`nice -n 19` process at a time, and OMP/MKL/OPENBLAS threads fixed at one. Existing
installed native files were read through a task-local copied TensorFS package;
only this change's `_offload.py` replaced the copied Python frontend. No live
installation or original package was modified. Native factory calls in the CPU
transaction checks use explicitly inert metadata doubles.

Executed clients:

- `packing.py`: real Parameter/Source ties, aliases, partial casts, storage-local
  bounds, conversion rollback and metadata-only failed group publication.
- `packing_metadata_cpu.py`: metadata-only Plan rollback, stable old reservations,
  shared publication and Region deduplication.
- `adapter_cpu.py`: actual TransformerEncoder and tiny Diffusers VAE, identities,
  direct reads, mutation refusals and old escaped versions.
- `review_cpu.py`: actual writable outputs and floating/complex conversion cases.
- `intent.py`: actual nested/repeated declarations and exception lifetimes.

All passed. `review_cpu.py` emitted Torch's expected complex-to-real and experimental
complex-module warnings. No CUDA execution, native packing, native Source/Pin
lifetime, refill, pressure, model GPU output, or release is qualified here.

Prepared CUDA checks: `packing.py --device 0`, the unchanged `adapter_gpu.py`, and
the native owner's `native-tensor-export/parts_gpu.py`. Existing native pressure
fixtures remain required because CPU success does not qualify physical pressure.
