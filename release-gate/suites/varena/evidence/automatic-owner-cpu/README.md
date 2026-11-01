# Automatic-owner CPU evidence

Native source is the P3 owner candidate; these reports prove CPU/local process behavior only. No automatic GPU donation has run.

- `nonblocking-before.json` preserves the stopped-publication failure; `nonblocking-after.json` uses the real OFD lock and a stopped process, with a live departing owner and zero capacity credit.
- `ledger-before.json` retains the old fixture assumption failure; `ledger-after.json` explicitly reaps only after exact child waits and proves row lock release. Namespace transition remains host-denied.
- `hello-new-native.json` and `hello-old-native.json` run actual Runtime Hello/Shutdown over the local framed socket with the paired and prior native bindings. They import no Torch, load no CUDA/NVML driver, and start no owner thread before Start.
- `wire.json` exercises the actual native command decoder/defaults; the first fixture's wrong dict argument is separately retained.
- `mode-race.json` executes the unchanged real categorical mode module with racing threads.
- Type and lint receipts compare the same baseline/candidate dependency environment, preserving existing findings with zero new ones.

Full commands/logs and source hashes remain under `outputs/xs138-p3-owner/`. The debug extension is task-local and was never installed as an SDK. The new GPU fixture is in `../../owner-runtime/`; ordinary paired TensorD/Runtime/TensorFS execution remains required.
