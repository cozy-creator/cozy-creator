# Torch offload clients

Use an installed TensorFS package built from the offload branch. The CPU client needs
Torch and Diffusers; GPU clients additionally require the matching native Torch shim.
No client downloads weights. GPU execution belongs to the coordinator.

- `inventory_cpu.py`, `diffusers_inventory_cpu.py`: non-mutating discovery clients.
- `adapter_cpu.py`: public `vt.offload`, actual tied/direct/alias reads, immutable dtype
  versions, mutation refusals and real tiny VAE encode/decode on CPU Sources.
- `review_cpu.py`: real stock comparisons for ordinary writable outputs, all four
  floating-only convenience casts, and explicit complex dtype conversion.
- `intent.py`: real CPU forwards through repeated/declared nested boundaries,
  exception cleanup and dtype refresh. `--device 0` additionally observes the actual
  native phase view on the coordinator GPU; that mode remains unrun in this lane.
- `adapter_gpu.py --device 0`: prepared actual CUDA client for native typed reads,
  default and explicit streams, later consumers, ties, old aliases and tiny VAE.
- `sdxl_gpu.py /existing/local/model --device 0`: prepared stock SDXL baseline/adopted
  comparison with normal `force_upcast`, full image output and Parameter identities.
  Two steps are smoke only. It requires the existing model and normal pipeline deps.

Run CPU clients with `CUDA_VISIBLE_DEVICES='' OMP_NUM_THREADS=1 MKL_NUM_THREADS=1
HF_HUB_OFFLINE=1`, and `nice -n 19` on the shared host. CPU receipts do not qualify
native residency, Source publication, consumer fences or GPU model behavior.

[Torch adapter contract](../../../../crates/varena/TORCH-OFFLOAD.md) describes
supported behavior and refusals. The native failed-tail/producer/lifetime fixtures
remain in the adjacent `native-tensor-export` suite; no mocked allocator substitutes
for those GPU gates.
