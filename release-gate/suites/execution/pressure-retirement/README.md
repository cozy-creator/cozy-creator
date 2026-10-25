# Automatic external-pressure retirement

This local Linux gate reuses the real native loop and independent CUDA consumer from the
cooperative-pressure gate17243, but qualifies the current hard process-retirement contract.
Its managed workload runs through the default Cozy CLI. The foreign consumer is a finite child
of this controller, outside TensorD's managed family, representing another desktop application.
A second Cozy GPU request would not be foreign pressure. This controller is intentionally local;
it does not use SSH, another daemon, alternate homes or a private candidate execution path.

The GPU owner must first install the final candidate normally and own the shared local GPU.
The foreign Python only needs Torch/CUDA; it does not run Cozy package internals.

```sh
python3 external_pressure_gate.py \
  --foreign-python /absolute/torch-environment/bin/python \
  --expected-tensord FULL_COMMIT --max-foreign-mib EXPLICIT_BOUND \
  --seed FRESH_SEED --output /absolute/new/receipt-directory
```

The controller preinstalls its package before the observation window. It records the managed
request ID from Context (distinct from the Hub run ID), PID and Linux process birth, opens a
pidfd before creating foreign demand, and keeps that exact handle through cancellation.
The foreign consumer uses measured free capacity, has an explicit allocation bound, and keeps
at least384MiB headroom while growing; it refuses an unsafe/unreachable pressure setup.

Required proof:

- Driver-attributed foreign growth crosses the actual monitor's threshold and the machine
  automatically cancels the managed request as `machine:gpu_pressure`; no manual CLI cancellation
  may produce a pass.
- The exact original process exits, proven by its retained pidfd. No Python finally/cleanup log
  or Invoke statistics are required from a killed executor.
- At least256MiB becomes genuinely driver-free while the foreign application still holds its
  original buffers, and it successfully uses another128MiB with correct values.
- After foreign exit, the ordinary memory log shows admission cleared. A same-entrypoint successor
  then executes correct CUDA arithmetic in a different PID; no package work is replayed.
- The installed SDK/machine identity remains unchanged. Logs come from `cozy machine logs local
  memory|tensord`; run/terminal/output evidence comes from normal `cozy run` and `run show`.

The observer deadline only fails this proof and cleans up its own work. It never qualifies
reclamation based on elapsed time. The old cooperative17243 receipt remains historical evidence.
This cell does not cover an external allocation that never succeeds, Windows driver attribution,
multi-rank pressure, or the separate idle-pending-preflight offload race.

No live run is recorded yet. Source checks do not establish GPU qualification.
