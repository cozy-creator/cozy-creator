# Scoped readiness host boundary

Root approved `vr_v1_use_wait_ready`, Rust `Use::wait_ready` and Python `Use.wait_ready` for generic streamed graph capture. Native context is the reviewed0af/3c cohort plus the isolated zero-span/packing cleanup. Context-only `4463a4fd94ac2510a27d3d14d47165d806e0cf4d` narrowly preserves current-master safety guards/build workflow/executor protocol; it is not an adoption commit. Readiness feature implementation begins at `186165d9fa07af7c27583d179ce2e24fe8b92c98`.

Native no-default checking, default extension build and Rust client build pass with max one CPU and task-private targets/cache. The actual C ABI host client rejects invalid Use handles with CUDA unloaded; production bridge host checks and actual Python Invalid propagation pass. The actual C Source provider compiles with warnings as errors and its original host contract passes without loading CUDA. AST/Ruff and feature unsafe-file inventory delta checks pass.

GPU modes in `use_ready_gpu.py` remain prepared/unrun. Host invalid-handle tests do not prove native capture readiness, blocking/GIL behavior, concurrent-close ordering or error/resource retirement on GPU. The real provider mutex controls supply those coordinator-owned tests. No native-resource mocks/source unit tests are used as product qualification. Runnable commands and exact guarantees/limits are in `../../USE-READY.md`.

The feature modifies existing ABI/async-fill/client files, with no new unsafe Rust path, no changed unsafe allow, and no edits to preserved master guard/executor files. Full root reconciliation/advisory qualification remains separate. Both frozen worktrees remain unchanged. No policy, GPU execution, rental, merge, release or operator artifact/cache write is included.
