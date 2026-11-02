# Independent review/adoption of the CLIP inventory fix

Upstream fix: `25cec3cb7179026e8bdc0c4b726aab9073c38915` (draft #2470), adopted as
`52efb89cbad7da8a752355b5cb27c0f4019a020c` on the P4 offload branch. No overlapping
product implementation was added. Review found no correctness blocker in the fix.

The one-line change accepts concrete `set` declarations alongside tuple/list. Member
validation, component scope, MRO matching, deterministic module traversal and existing
ownership/refusal guards remain unchanged. The imported regression checks malformed
containers/nonstring members and both initialized CLIP variants.

This review independently used **Transformers 5.22.1**, the version named in failed
stock SDXL run 26210, with Torch 2.14.1+cpu. The original CLIP class declaration is a
list, while the initialized model's declaration is a set. Both tiny model variants
reproduce the old refusal (`independent-original-red.json`). After adoption:

- `clip-inventory-5221.json`: CLIPTextModel and CLIPTextModelWithProjection retain
  exact forward output, Parameter/storage identities, flags and declared boundaries.
- `clip-adoption-5221.json`: actual public `vt.offload` CPU inference, including hidden
  states, exactly matches stock outputs. Identity, managed old aliases, half/float
  conversion and declared intent cleanup pass. `clip_adoption_cpu.py` is the exact
  runnable receipt client; it writes no files and downloads no weights.
- The maintained inventory, Diffusers inventory, adapter, out=/complex review and
  intent clients all pass. Their JSON and warnings are retained here.

These are actual CPU frontend/component checks using a staged production package at
`.task/review-host-package`; no native allocator simulation or CUDA was used. The
inventory fix requires no native rebuild. It does not qualify adopted full SDXL,
GPU output parity, residency or performance; the coordinator owns the stock rerun.

Commands, versions and source hashes are in `manifest.json`. Upstream's independent
Transformers 5.17.0 red/green evidence remains unchanged in the adjacent
`2026-11-02-clip-inventory` directory. Ruff lint passes; its imported CLIP client's
assertion wrapping differs from this lane's formatter, and was preserved as supplied.
All earlier receipts and source worktrees remain intact. #2449 stays draft.
