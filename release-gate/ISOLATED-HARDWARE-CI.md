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
uses its focused `--admin --command` bootstrap, seeds one tiny package into its own catalog and
runs CPU and CUDA arithmetic through the selected CLI/Runtime/TensorFS. The complete publication
suite is separately qualified instead of repeated on every hardware run. It requires an explicit
idle NVIDIA GPU UUID and fails when another compute process is using it. No user service is
installed or restarted: a second HOME has no login service manager.

```sh
python3 e2e/hardware/run.py --hub-src /path/to/tensorhub --cozy-bin /path/to/candidate/cozy \
  --runtime 0.45.0 --tensorfs /path/to/tensorfs-0.24.1-...whl --device GPU-... --out /path/to/empty/receipt
```

Linux prerequisites are Docker, Go, uv, AWS CLI and an NVIDIA driver. The source checkout and
candidate CLI are explicit; public dependency/compiler caches are reusable build inputs.
Provider keys, proxy settings, production Hub URLs, device credentials and service environment
are excluded. The outer receipt records the binary digest, cohort, device, owned process births,
container cleanup and preservation of existing Cozy processes. The private HOME is removed;
scrubbed execution records remain in the output directory. Candidate wheel URLs require their
SHA256 fragment; the update journal must name that digest and both CPU/GPU measurements must
name the requested versions. Live output is streamed and saved. Actions stores receipts under
the durable job workspace, because its temporary directory is erased before a runner listener
returns. The receipt is written before teardown, including when an owned read-only tree cannot
be removed. Permission repair touches only directories with this job's UID inside its allocated
root; symlink targets and public dependency caches are excluded.

The native SDK cell writes a complete 2x2 F32 checkpoint, deletes its source cache, opens the
file with a fresh store and `local_files_only=True`, and uses public `tensors.read` plus DLPack
on CPU/CUDA. Matrix products must be `[8, 18]`. Its records include the loaded `_ext` and exact
Torch shim file hashes; matching version labels alone cannot qualify a different wheel payload.

`hardware-isolated.yaml` selects a unique `cozy-isolated-gpu-<16 hex digits>` runner label.
An operator registers an ephemeral runner without default labels for one trusted dispatch; this
prevents it consuming the repository's other self-hosted work. PR regression tests use hosted
Linux runners, and hardware execution runs only on explicit `workflow_dispatch`.
Its release tag, Hub commit, Runtime/TensorFS versions and reserved GPU UUID are explicit inputs.
The runner must be preallocated; the workflow creates no paid allocation. This tiny proof is
physical local CUDA qualification, not inference quality/performance, provider lifecycle, or the
full release-gate suite. Existing release/provider gates remain red without their own hardware.

`--mode provider-sandbox` deliberately fails before fixture startup: no provider sandbox account,
budget or reachable test allocation is currently configured. It has no production-credential
fallback. A Docker provider control-plane pass or local CUDA pass cannot satisfy that gate.
