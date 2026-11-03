# f574 review corrections

- CPU fix: `20b45ab9837c6d56c78aafc997de24625dac1e8a`.
- Intent wiring: `5e1fe22227e0e65bb0c8a3fc441a3ed80593ec74`.

The original review/reproduction and our real public-package red receipts are retained.
`regressions-green.json` then passes ordinary writable outputs, all four floating-only
convenience casts, and both explicit complex-conversion controls (7/7). Intent's CPU
client runs six real declared/repeated boundaries through nested and direct forwards,
parent restoration, exceptions, conversion refresh and passive diagnostics. CPU mode
explicitly records `native=false`; no fake GPU/allocator is used.

The original Transformer/tiny VAE adapter client, native metadata/refusal client and
both actual C/external Rust/internal Rust/Python load orders pass. The mixed C client
also calls the two new intent symbols with an actually absent handle, verifying the
production refusal boundary without CUDA. The one mapped core, shared errors and
native request sequence remain observable in both orders.

Fresh native builds passed. Ruff check/format and diff checks passed. Whole-file
Rustfmt checks retain inherited differences: `rust-format-audit.json` compares frozen
f574 with current files and verifies no formatting hunk touches added Rust lines.
No unrelated Rust formatting was applied. Warnings from intentional stock explicit
complex-to-real conversion and Torch's complex-module notice are retained.

Manifest commands use a staged production package at `.task/review-host-package`,
not a wheel installation. All checks ran on real Torch 2.14.1+cpu, with one worker and
no model downloads or CUDA execution. Earlier receipt directories remain unchanged.

Native change: additive ABI v1 `vr_v1_plan_activate` / `vr_v1_plan_deactivate` plus safe
Rust and Python bindings. Both delegate to existing native Plan intent; no allocation
policy, ledger, graph reclamation, compute key or successful H sample was added.
Module conversion refreshes the registered immutable versions before next entry;
failed cleanup retains the exact owner for explicit close/retry.

`intent.py --device 0` is prepared for the coordinator. It verifies actual native
phase names/Region bytes, nested restoration, exceptions, version replacement and
final deactivation on CUDA. That execution and the existing GPU/SDXL/residency gates
remain unrun by this lane.
