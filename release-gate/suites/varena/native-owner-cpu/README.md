# Native owner host-control stress

`prove.py --source REPO --target EXISTING_CARGO_TARGET --out NEW_OUTPUT_DIR`
compiles the exact native owner source in an output-only crate, appending the
visibility-local `lifecycle.rs` harness there. Product source contains no unit tests.
The receipt records all product source hashes and the harness hash.

This is a real native-thread/channel/ownership stress executable: 512 blocked
metadata callers cancelled by stop, admitted versus still-queued operations, Drop
while a worker cannot yet finish, and cancellation after an actual metadata panic.
It initializes no CUDA, substitutes no GPU implementation, and makes no GPU-owner
or donor-service qualification claim. The command timeout is a failing harness
guard, not an accepted allocator result.

Actual CPU Hello/Shutdown remains `../owner-cpu/hello.py`. It additionally records
Linux native task names so a hidden Rust thread cannot evade its CPU-parent check.
GPU owner parity, GIL independence, fork and capture-aware retirement remain real
release-gate runs through the ordinary Cozy CLI.

The harness also requires permanent retirement metadata faults to return from actual
`Owner::close` with `retired=false`, and forks with the actual control mutex held to
prove inherited operations refuse before touching it. These authorize no GPU release.

`portable.py OVERLAY OUTPUT_JSON`, run under isolated Python, checks the actual
portable native Owner API and metadata-only CPU-parent fork. It never constructs a
CUDA owner. Initialized GPU-owner fork/retirement remains a separate GPU gate.

The shared-service increment appends `shared.rs` and `pressure_host.rs` in the
same output-only crate. It adds 16-way first admission, 16 concurrent shared
acquisitions, 256 admission/last-release races, both arrival/close orders, real
tmpfs row/configuration identity, retryable final close under actual registered
capture counters, old-ticket/new-generation isolation, per-client abandonment,
unreported-client activity and a failure tombstone after the last handle disappears.
The temporary metadata directory is unique, mode 0700 and recorded in report.json.
No original ledger is reset or removed.

The proof marks real host Control ready/retired around real ledger participant
lifetimes; it does not create or replace a CUDA Peer. Pressure is the product's
host state with an unavailable sampler and no invented observations. It proves
unknown/abandoned windows never train H, not qualified NVML measurements or GPU
retirement. No CUDA or NVML mapping may appear in the process.
