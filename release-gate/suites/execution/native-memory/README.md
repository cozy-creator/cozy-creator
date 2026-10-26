# Native memory release gates

One package/controller consolidates the current real stage, repeated-range, placed raw/INT8,
source-failure, cold-kernel capture, shared-pool and bounded-prefetch criteria. These run actual
CUDA work and TensorFS reads through the ordinary default Cozy CLI. They install no SDK, rent
no machine, use no mocks and write no Runtime unit tests.

```sh
python3 release-gate/suites/execution/native-memory/run.py --groups stages,repeat
python3 release-gate/suites/execution/native-memory/run.py --execute --on local \
  --out /absolute/fresh/proof-directory --seed 3295001
```

The default dry run lists all53 cells. `--groups` selects comma-separated public endpoints:
`stages`, `repeat`, `model_stages`, `stored_roles`, `source_failure`, `cold_capture`,
`shared_pool`, `prefetch`. Each requires its real native capabilities; missing functionality
fails visibly rather than silently switching to an eager/mock implementation.

The caller owns the machine/SDK qualification window. The controller records source hashes,
exact commands, terminal run receipts, machine observations and result facts externally. Expected
host failures must preserve their reason, checked cleanup and a healthy successor in the same
executor PID. Timeout handling cancels only the exact run published by that command and waits
for its terminal state; it never kills an unrelated process or changes the machine's SDK.

Private source corruption affects only newly copied fixture-owned files. Stored INT8 gates
verify the actual native kernel before and after reference/capture/execution. Shared scratch
checks explicit persistent data edges and refuses escaped tensor views/storage or independent
stage launch. Prefetch tests preserve useful warm cache; zero reads are success, not a reason
to evict weights to force the optimization.

`SOURCE-EVIDENCE.json` records the historical functional receipts from which these current
cells were consolidated. That evidence does not automatically qualify this new packaged entrypoint
or an arbitrary new source/artifact combination. A fresh real CLI run is required. Historical
failed fixture versions and provider receipts remain in their original outputs directories.

These are correctness/ownership/capacity gates. They do not establish full-model performance,
CUDA-event copy/compute overlap, network-demand resumption, Windows/MPS inference, or multi-GPU
collective safety. Those require their dedicated ordinary workload gates.
