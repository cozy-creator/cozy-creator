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

The CLI e2e shards call `require_ci_test_hub`: they run only against an isolated Hub (`e2e/cli/isolated.sh`).
Release-gate and pod-session jobs rent real machines, which only production can place: they call
`require_ci_rental_hub` with their dispatch's `hub=prod` or `hub=isolated`, and none has a default.
Public dependency and release-artifact downloads are normal build inputs and remain allowed.

Run the guard regression checks without a Hub or credentials:

```sh
python3 -m unittest discover -s e2e -p test_hub_test.py
```
