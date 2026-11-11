# Isolated publication tests

Synthetic package/model publication runs only against the disposable Hub in `e2e/skew/run.sh`.
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

Production `release-gate` provider checks still rent, send the weightless fixture by content,
verify worker authentication and end the rental. They create no synthetic Hub repository.
Selecting the `hub` suite also runs the isolated publication job, whose success is required before
the gate can publish a baseline.

Run the guard regression checks without a Hub or credentials:

```sh
python3 -m unittest discover -s e2e -p test_hub_test.py
```
