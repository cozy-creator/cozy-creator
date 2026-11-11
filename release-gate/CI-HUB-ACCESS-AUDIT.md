# CI Hub access audit

CI verification reads and writes must target an isolated local test Hub. Public source, installer,
wheel and model-artifact downloads remain build inputs. A production account, catalog, billing
record or database is not a test fixture.

| Entry point | Previous path | Current boundary |
| --- | --- | --- |
| `release-gate.yml` / `gate.sh` | Production device key, account/catalog reads and rentals | No production key; early verified-fixture guard; missing hardware infrastructure fails |
| `pod-session.yaml` / `sessions/lib.sh` | Production owner login and provider sessions | No production key; guard runs before any session data request |
| `hubgate.sh` / Hub suite | Production-default read checks and synthetic repositories | CI guard; synthetic writes always require the fixture; manual provider flow remains separate |
| `e2e/skew/run.sh` | Local disposable Hub for released-client skew | Same fixture, now also accepts current binaries/cohorts and synthetic extension checks |
| Capability and hubcheck tools | Implicit production target | Explicit target required; direct Go entrypoints reject non-loopback CI targets; outer guard verifies proxy resource/issuer |
| TensorHub `contract.yaml` | Local services, but extra settings could override their targets | Local data/provider/listener targets cannot be overridden |

The main `e2e.yaml`, `e2e/lib.sh` and release-suite split are coordinated in the parent task.
Their control-plane fixture must not be reported as real GPU qualification. The same early guard
covers inherited CLI/import/CPU/MPS paths before they can fall back to production.

Hardware follow-up is explicit in [ISOLATED-HARDWARE-CI.md](ISOLATED-HARDWARE-CI.md). Until private
GPU/provider fixtures exist, a selected hardware CI job must fail instead of using production or
claiming a skipped run passed. No public staging deployment is needed.

Verification used local unit fixtures only. It did not rent machines, run models, publish packages,
or call production data APIs.
