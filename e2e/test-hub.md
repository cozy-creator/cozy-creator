# Isolated publication tests

All CI verification reads and writes run only against a disposable local Hub such as `e2e/skew/run.sh`.
It owns a fresh Postgres container, MinIO store, Hub process and development account, and removes
the container/store when it exits. It receives no production device key.

Use a released client with `--client VERSION`, or a candidate with `--cozy-bin FILE`. Optional
`--runtime` and `--tensorfs` arguments select the rest of the candidate cohort. `--checks FILE`
sources additional fixture checks after setup. `--publication-log FILE` writes CLI argument
records for the coverage collector.

Before any synthetic write, `require_test_hub` checks the loopback destination and the fixture's
`COZY_TEST_HUB_ORIGIN`, then verifies the Hub's public resource and authorization-server origins.
A localhost proxy to `tensorhub.com` fails this check, as does an unset or production target.
Redirects are not followed. This guard belongs to test helpers; normal user publication is unchanged.

The gate and session entrypoints call `require_ci_test_hub` before any Hub data request. Production
device keys are absent from their CI jobs. A hardware job without its own verified Hub fails;
it does not report a skipped GPU check as a pass. Public dependency and release-artifact downloads
are normal build inputs and remain allowed.

Manual operator qualification outside CI is separate and may use an explicitly authorized account.
See [the hardware CI follow-up](../release-gate/ISOLATED-HARDWARE-CI.md) for the remaining runner requirements.

Run the guard regression checks without a Hub or credentials:

```sh
python3 -m unittest discover -s e2e -p test_hub_test.py
```
