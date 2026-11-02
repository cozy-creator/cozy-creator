# Current source and companion Runtime artifact

Creator source `0a37d56a8ab4758e063f7470e6312922c177c730` matches native
`d3e8b36512a8456efb13b9efcbaa8d4f65de30a1` across Varena, TensorFS,
TensorFS-Py and external Rust client implementation/manifests. This includes the
scalar census and retired-backing reclaim/error-latch changes. `source-inputs.json`
records the same 18 differing Cargo/platform/protocol paths as the prior 118d
comparison. Updater and per-rank sender hashes are unchanged. No current worker
or TensorFS artifact was built in this lane.

Runtime source 88b7 now has a separately stamped full wheel; see
`runtime-wheel-summary.json` and paired Runtime `docs/evidence/current-wheel-88b7/`.
It preserves 581 native members and passes full RECORD/source/native closure,
packaged import/CLI, real refiner CPU contracts/semantic keys and strict framed
startup. It was checked with the existing native-lineage d3e8 TensorFS wheel,
not a new current-source private dependency artifact. No GPU or live install ran.

The next worker/TensorFS build must be coordinated separately and reuse the
matching exclusively owned cache with immutable output copies and the qualified
helpers-first/final-Maturin-last order. The updated command plan is
`../../current-cohort-build-plan.md`. No prior GPU proof is silently reassigned
to this current worker/Runtime pair.
