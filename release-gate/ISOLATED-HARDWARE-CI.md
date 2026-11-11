# Isolated hardware CI

CI verification must not query or mutate production TensorHub data. The `hub-fixture` job supplies
a disposable database, object store, Hub and development account for control-plane checks. It is
not physical GPU or provider qualification.

`release-gate.yml` hardware jobs and `pod-session.yaml` now fail before Hub access unless their
runner has a verified isolated Hub. They do not use the production owner device key. Manual
operator runs outside CI remain a separate, explicitly authorized workflow.

Before these hardware jobs can pass in CI:

1. Provision the same local Hub fixture per job, with a fresh database, local object store and test
   credentials. Seed required packages/models from versioned public artifact manifests; do not copy
   production accounts, catalog queries, billing records or database dumps.
2. Attach preallocated GPU workers over a private runner network, or use separately budgeted provider
   sandbox accounts tied only to the test Hub. The worker must reach the fixture's authenticated origin.
   A public staging deployment is not required.
3. Declare the selected suites' hardware needs and fail if the runner cannot provide them. Never turn
   an unavailable GPU/provider check into a passing control-plane fixture result.
4. Preserve real execution/output receipts, then destroy the test Hub's state and end only workers
   owned by that job. Verify cleanup independently.

Current verification paths

| Path | Boundary |
| --- | --- |
| `e2e/skew/run.sh` | Creates and validates its own local Hub before login or tests |
| `release-gate/gate.sh`, `hubgate.sh` | CI target guard before reads, account checks or rental operations |
| `release-gate/sessions/lib.sh` | CI target guard before session setup |
| `e2e/capabilities/run.sh` | CI target guard before account/capability probes |
| TensorHub `contract.yaml` | Fixed local database/storage/provider targets; extra settings cannot override them |
| Installers, wheels and source downloads | Public build inputs; no production-data assertions |

The pre-existing `ci-tfs-*` transfer/performance cells must use the isolated hardware setup when
run in CI. Their manual performance boundary has not been replaced by a mock or local-only timing.

## Preallocated local GPU fixture

`e2e/hardware/run.py` reuses `e2e/skew/run.sh` with a fresh private HOME, machine root,
TensorFS store and sanitized environment. The same owned PostgreSQL/MinIO/development Hub
first runs its existing publication checks, then seeds a tiny package into its own catalog and
runs CPU and CUDA arithmetic through the selected CLI/Runtime/TensorFS. It requires an explicit
idle NVIDIA GPU UUID and fails when another compute process is using it. No user service is
installed or restarted: a second HOME has no login service manager.

```sh
python3 e2e/hardware/run.py --hub-src /path/to/tensorhub --cozy-bin /path/to/candidate/cozy \
  --runtime 0.45.0 --tensorfs 0.24.0 --device GPU-... --out /path/to/empty/receipt
```

Linux prerequisites are Docker, Go, uv, AWS CLI and an NVIDIA driver. The source checkout and
candidate CLI are explicit; public dependency/compiler caches are reusable build inputs.
Provider keys, proxy settings, production Hub URLs, device credentials and service environment
are excluded. The outer receipt records the binary digest, cohort, device, owned process births,
container cleanup and preservation of existing Cozy processes. The private HOME is removed;
scrubbed execution records remain in the output directory.

`hardware-isolated.yaml` selects the dedicated `cozy-isolated-gpu` self-hosted runner label.
Its release tag, Hub commit, Runtime/TensorFS versions and reserved GPU UUID are explicit inputs.
The runner must be preallocated; the workflow creates no paid allocation. This tiny proof is
physical local CUDA qualification, not inference quality/performance, provider lifecycle, or the
full release-gate suite. Existing release/provider gates remain red without their own hardware.

`--mode provider-sandbox` deliberately fails before fixture startup: no provider sandbox account,
budget or reachable test allocation is currently configured. It has no production-credential
fallback. A Docker provider control-plane pass or local CUDA pass cannot satisfy that gate.
