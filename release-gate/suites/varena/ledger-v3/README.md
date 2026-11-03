> Historical migration proof only: run against native commit
> `0657802458dfbf4fb7e66613e30c9baef1308e68`, whose receipts remain preserved.
> Current Varena deliberately refuses older layouts; use the ledger-current-format
> gate for the active contract. Do not apply this migration fixture to current source.

# Real v2/v3 metadata gate

`build.py` copies the exact old committed and current ledger modules into output-only
crates. It appends a visibility-local migration gate and a deliberately unsafe
rename-only control; the product source is unchanged by the harness. Both binaries
use actual Linux shared mappings, independent OFD locks and processes. No CUDA backend
is mocked or initialized. All hashes and commands are retained.

```bash
python3 build.py --source /absolute/creator-worktree --target /absolute/shared-target \
  --out /absolute/new-build-output
python3 prove.py --builds /absolute/new-build-output/builds.json --out /absolute/new-proof-output
```

The controller owns every child through Popen/pidfd. `/proc/PID/syscall` records the
actual old F_OFD_SETLKW waiter and its old inode before the migration proceeds.
The rename-only control must reproduce the stale registration/ghost row lock; the
product tombstone must reject it. Other cells cover live/SIGSTOPed old participants,
old opens, post-tombstone crash recovery, a real ENOENT rename rollback, activity and
consumer priority, unknown H preservation, staged owner requests, stale stage acks,
all-idle intent and per-donor caps. Owner exit advances without any capacity credit.

Declared CPU byte samples/acks prove metadata behavior only. They do not qualify
NVML attribution, cache callbacks, physical donation, GPU data or model performance.
