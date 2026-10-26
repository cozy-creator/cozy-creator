# Current libraries through the historical Mac CLI

[Workflow 40189558417](https://github.com/cozy-creator/cozy-creator/actions/runs/40189558417)
passed on macOS 15.7.9 / Apple M1 (Virtual). This explicitly selected library cohort uses
the historical Mac-capable CLI **7b5**, current Runtime **7d1**, and TensorFS **0.15.8** from
the six-platform matrix **365bb / 40188308767**. It does not enable or qualify local Mac
inference in the current CLI, whose platform admission policy remains unchanged.

| Input | Source | Actual SHA256 |
|---|---|---|
| Rebuilt native Mac CLI | `7b5f6d86ce5c23c9add24b816039489b637f64b3` | `fe181f031c2a564a69b70deda3de7e4f759f8ac19ee033b963fccfb028872799` |
| Pure Runtime wheel | `7d1e7854ca638c5d153c6601f2d2c2b169d0b133` | `177a1ba9a1e938ae4193162612ddf515d4a8e91e47a4b90fa97a73be2c127936` |
| Mac arm64 TensorFS wheel | `365bb9c9b831b8f39fecaeccf4bd4af07d275a30` | `1e46c7188adb17662278589eee839126e1c5d63986228456b0b65e82c85218df` |

Development packages were pinned to `a3755916454acff2e941644776f74c1e371766ca`. Normal
default-home CLI updates installed the exact wheel hashes recorded above. All three input
files, manifests and raw receipts were downloaded and preserved, including the rebuilt CLI.
Runtime's 192 Python source files and 221 hashed RECORD entries were verified; it is a pure
wheel with no CUDA native module. The Mac host also passed full TensorFS import, version,
control/Writer/pin/encoded capabilities and the measured ConvRot zero-value check.

- **Residency cell 4 passed:** real F32/F16/empty checkpoint loads, a 67,112,960-byte part
  crossing the 64 MiB read window, zero-byte warm reuse, escaped alias retention, release
  and refill of exactly 67,112,960 bytes. Torch owns MPS allocation and computation.
- **BiRefNet cell 6 passed:** actual MPS allocation was 448,878,336 bytes; recorded weight
  reads were 444,386,812 bytes. The delivered 500×500 WebP is 87,714 bytes, SHA256
  `4699bd9945760c6a64a5880cd4e0d839380e32dae110f76ab0ab8897558b4065`.
  Its white-background fraction was 0.912896; visual inspection confirmed the corgi remained
  on white. The 30.269s execution value is a single correctness run, not a benchmark.
- **SDXL cell 8 refused capacity:** `memory_exhausted` explicitly reported 6.5 GiB of weights
  against about 4.7 GiB available. This was the gate's permitted outcome. No 12-step image
  run completed, and no SDXL image was produced.

The [compact JSON](7b5-7d1-365bb-library.json) retains hardware, installed identities,
numerical/residency facts and sixteen raw-receipt hashes. The earlier
[historical proof](../PROOF.md) remains unchanged. This does not qualify CUDA Varena on MPS,
Windows native execution, pressure/cancellation, multi-GPU behavior, full SDXL performance
or a production release.
