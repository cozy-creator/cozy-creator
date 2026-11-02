# P4 host and installed-wheel evidence, 2026-11-01

Product source: **bf5637f10a9dc623f796b6700a8cfbf31a8bf21f**. Subsequent evidence commit
adds these receipts and whitespace formatting only. Integration snapshot and both
coordinator checkpoints are named in `manifest.json`. No policy, ledger or graph
reclamation implementation was changed by P4.

Passed:

- `prove-host.sh .task/final-host-bf5637`: full TensorFS extension and native `tfs`
  build, C11 engine/client with `-Wall -Wextra -Werror`, C++17 header compilation,
  safe Rust example, actual C/Rust/internal-Rust/Python clients in both load orders.
  All five identities match inside each process, exactly one core is mapped, a C
  failure is visible through Python's shared TLS, threads have independent errors,
  and the native request sequence advances. Refused constructors preserve Source
  ownership and zero outputs; incorrect stats layout size is rejected.
- Both standalone C/Rust **host paths**, without initializing CUDA, and `tfs version`.
- Maturin **debug**, `--compatibility linux --auditwheel skip`, followed by `bundle.py`;
  installed through `uv pip` in an isolated CPython 3.12.12 environment. Python import
  and the installed native `tfs` run with `LD_LIBRARY_PATH`/`PYTHONPATH` removed.
  Both mixed-binding load orders pass against the installed shared libraries.
- An independent Cargo project depends only on `varena-client`, links the installed
  C ABI, and runs its typed Invalid/ABI checks. It has no dependency on the core crate.
- All **28** declared `vr_v1_*` header functions match actual exported symbols.
- Targeted rustfmt, Ruff and shell syntax checks; `git diff --check`.

The wheel hash is in `final-wheel.sha256`. `final-wheel-bundle.json` hashes the core
before RPATH relocation; `manifest.json` hashes the actual installed core. SDK library
hashes are in `sdk-artifacts.sha256`. These are local Linux x86_64 development
artifacts, not portable/manylinux-qualified or published packages. No Torch distribution
was available in the checked local Python environments; importing the Varena Torch
adapter is not execution of Torch or its allocator shim.

GPU examples and `mixed.py --gpu` are built/prepared but **unrun**. Allocation sharing,
physical reclamation, exact GPU contents/refill, stream/source failures, cache holes,
declined/allocator-triggered shrink and GIL behavior need the coordinating agent's GPU
lane. General offload, 8 GB Diffusers and full 2/4-GPU H3 remain open in xs-138.

The initial primary-core link failure and external-client build-order failure are
retained as compressed logs. The corrected builder packages the exact dependency
dylib and completes that build before linking a client with only an external C link.
Earlier SDKs, wheel, failed logs and final artifacts remain under this worktree's
`.task/`; no source worktrees, rentals or machine records were removed or modified.

The existing native `cozy`/TensorD archives and direct-maturin release paths still
need shared-library bundle integration before release. This draft is not merge-ready.
