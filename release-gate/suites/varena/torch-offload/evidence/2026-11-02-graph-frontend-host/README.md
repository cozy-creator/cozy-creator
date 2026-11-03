# Graph adapter host checkpoint

Feature: `5a4d51fde2a6df618cb608474adbd8337f66aa98`. Dependency/context commits are
separate so the coordinator can compose feature-only changes onto its native cohort.
Current-master `deny.toml`, `scripts/memory-safety.sh`, `unsafe-boundaries.txt` and
workflow guard remain byte-identical to this lane's fetched base; hashes are recorded.
No Rust/native or allocator policy change is part of this graph feature.

## Actual execution

All eight real CPU clients pass against an owned source overlay and the verified
Agent 2 scoped-readiness native build (copied read-only; no other owner's cache writes).
Torch is2.14.1+cpu, Transformers5.22.1 and Diffusers0.41.0, with one worker/thread and
no model downloads. `graph_bindings_cpu.json` specifically executes the SAME frozen
capture-binding dispatch used by the adapter, on actual tied/strided Torch and a tiny
Diffusers VAE. Values match eager exactly; a profiled positive control proves no new
residency materialization inside that binding scope. Managed aliases, ordinary output
copies, real mutation/signature refusals, old versions and empty interior views pass.
No CUDA or allocator substitute is involved.

`ready-api-host.json` calls actual production Python/C readiness entry points with
absent Use IDs and verifies one core identity. It is NOT a valid native Use wait,
blocking-GIL or resource-retirement proof. `scoped-helper-refusal.json` executes the
actual prepared frontend client's capability guard: the missing scoped helper refuses
before CUDA initialization or libcuda mapping.

Original adapter/Transformer/VAE, review, intent, inventory, Diffusers/CLIP and real-only
Plan transaction clients pass too. Ruff check/format, AST and diff checks pass. This is
source/host proof, not installation or a current-master native build qualification.

## Required remaining gates

The adapter requires scoped `Use.wait_ready` and the proposed agreed opt-out from
`vt.graph`'s existing context synchronization. The helper change is still awaiting
shared-file agreement; `torch.py` was not edited. There is no fallback global drain.
Source/retirement readiness before every epoch is deliberately conservative; no
fill/compute-overlap or latency win is claimed.

`graph_frontend_gpu.py` and all `graph_epochs_gpu.py` modes are prepared and UNRUN.
The coordinator must prove actual graph capture, multiple cached definitions with
weight Pins0/unjoined=false, same-VA trim/refill/replay, independent old outputs,
escaped native aliases, stale selection, actual Source failure, unrelated producer
refusal, delayed consumer and reset/copy tails.

Private Working pools retain normal backing while graph/static owners live. Shared
pools, automatic eligible-boundary caching, dtype-version reuse and stock-model/8GB/
performance/platform qualification remain open. This feature provides a segmented
primitive; it does not imply universal whole-model fit or streamed scratch pools.
Historical receipts, frozen2449 and coordinator/packing worktrees remain preserved.
