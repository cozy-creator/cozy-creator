# Isolated hardware fixture

This lane uses its own PostgreSQL, MinIO, development Hub, account, private HOME and
machine store. It makes no provider order and has no production credential fallback.
Other release/provider workflows have their own explicit target and authorization rules.

`run.py` reuses `e2e/skew/run.sh --admin --command` and publishes one tiny probe. The
published package uses only the supported Runtime/Torch author surface. Its real CLI
CPU/GPU calls must return the exact arithmetic result, requested software cohort and
physical GPU UUID. Full publication qualification is separate from this focused lane.

Direct TensorFS `Writer`, `open`, native extension inspection and CUDA/DLPack checks are
operator SDK work. They run as a separate script in the owned installed candidate Python
environment, never inside the published author package. The SDK check writes a complete
2x2 F32 checkpoint, deletes its source cache, opens fresh local-only stores, and checks
matrix products `[8, 18]` on CPU/CUDA. Loaded native extension and Torch shim bytes must
match the actual candidate wheel ZIP entries, as well as the whole-wheel SHA256.

```sh
python3 e2e/hardware/run.py --hub-src /path/to/tensorhub --cozy-bin /path/to/candidate/cozy \
  --runtime 0.45.0 --tensorfs /path/to/tensorfs-VERSION-...whl \
  --device GPU-... --out /path/to/empty/receipt
```

Linux prerequisites are Docker, Go, uv, AWS CLI and an NVIDIA driver. Candidate wheel
URLs require an exact SHA256 fragment and are materialized into verified local files
before the release CLI update. The original URL/hash remains in the receipt. Public
build caches may live outside the disposable authentication HOME; fixture/model data
does not share that cache.

Live output is retained under the durable job workspace. Logging ends with the main
fixture process, even if an orphan watchdog retains stdout. Receipts are written before
teardown. Cleanup selects exact owned process births and container labels/IDs, and repairs
read-only directories only inside the allocated root with the same UID. Symlink targets,
public dependency caches and existing user services are preserved.

`hardware-isolated.yaml` runs source and real Runtime.describe contract checks on hosted
Linux. Hardware requires explicit workflow dispatch to a unique ephemeral runner label;
the runner has no default labels and takes one job. This tiny proof qualifies its stated
CLI/Runtime and native SDK paths, not inference quality/performance or provider lifecycle.
`--mode provider-sandbox` fails before startup while sandbox infrastructure is unavailable.
