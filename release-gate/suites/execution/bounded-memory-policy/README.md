# Bounded memory policy: real CUDA cells

Run each mode through the ordinary default-home CLI on a root-coordinated candidate machine:

```sh
cozy run /absolute/worktree/release-gate/suites/execution/bounded-memory-policy/gpu \
  mode=prefetch seed=179701 --on local --gpus=1 --json
```

Modes are `prefetch`, `bounded`, and `capture`; each attempt needs a fresh seed. The executor uses
an actual 256 MiB local activation/weight cap, real TensorFS checkpoint files and native CUDA
backing. There are no fake allocators, exception injection, monkeypatches or private interpreters.

`prefetch` loads component A, retains its outgoing cached weights, and executes B with sufficient
free capacity. It requires real next-block reads, no loss of A's bytes, and zero reads on returning
to A. It then cancels a real next-block read and checks that no temporary preference survives.
Finally it physically evicts a preferred tail, fills the bounded streaming window with real bytes,
and requires speculative admission to respect the shortened native watermark without evicting
that window or the current block. One pending native producer remains the existing invariant.

`bounded` pins an escaped weight alias and calls the existing sized reclamation path. It requires
exactly one 4 MiB block to leave the cache, the alias to remain correct, and the next actual model
pass to reread only those 4 MiB. **This proves bounded reclamation, not the unexpected pure-cut
retry branch.** A successful underestimated-workspace OOM/retry receipt and its full-reclaim last
chance are still required; neither source inspection nor this mechanism cell substitutes for them.

`capture` requests a real allocation twice the physical device capacity during CUDA capture,
through a declared-pure cut. It requires a typed allocation refusal with unchanged native state,
no recovery/retry telemetry, checked graph teardown and healthy subsequent CUDA work.

These are correctness cells, not model benchmarks or evidence of overlapped PCIe transfers.
Source checks and actual GPU receipts are separate; all installed-cohort GPU results are pending.
