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
