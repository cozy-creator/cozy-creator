# CPU contract checkpoint

Source: `3dbe4281105a390acce8e4493cdcc3b754f7cc8f`. These receipts cover real
CPU Torch frontend behavior and non-mutating model inspection. `vt.offload` is
not implemented here. No native residency, Source, CUDA, or model-download proof
is claimed. The API/Source-owner contract remains pending.

| Receipt | What ran |
| --- | --- |
| `inventory.json` | Real tied Linear forwards, storage views, boundary discovery and refusals; values and Parameter identity remain unchanged. |
| `diffusers-inventory.json` | Tiny random AutoencoderKL encode/decode, pipeline components, unchanged output/identity and the stock VAE dtype cycle; existing offload hooks are refused. |
| `inventory-review-green.json` | Nine targeted real CPU checks after the preserved review findings were corrected. |
| `inventory-*-red.json` | Original failing preflight observations, retained as counterexamples. |
| `torch-*.json` | Real dispatcher/alias/dtype experiments, including unsafe counterexamples; a successful reproduction is not adapter acceptance. |

The two maintained clients are `../../inventory_cpu.py` and
`../../diffusers_inventory_cpu.py`. Exact environment and commands are in
[manifest.json](manifest.json). Diffusers warnings are retained in
`diffusers-inventory.stderr`.

[Torch feasibility](torch-feasibility.md) explains the experimental results and
their limits. The `feasibility-*.py` scripts are exact copies of the experiments,
not production adapter code. Their original names, commands and hashes are in
[feasibility-manifest.json](feasibility-manifest.json). To repeat an experiment,
copy it into an owned artifact directory first: it writes a JSON file beside
itself. Original `.task` receipts remain unchanged.

Before native adoption, the API/Source owners must supply typed storage-owned
exports, producer/consumer ordering and immutable host/dtype-derived Sources.
GPU/model qualification remains with the coordinator.
