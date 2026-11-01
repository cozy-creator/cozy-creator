# Varena ledger: real multiprocess CPU proof

This tests the scoped P3 metadata library on Linux tmpfs. It opens no CUDA context and allocates no GPU memory.
The numerical byte holdings are records supplied to the metadata API; they are not GPU-reclamation evidence.
Its private test UUID and `/dev/shm/varena-ledger-proof-*` directory isolate it from real participant tables.

```sh
nice -n 19 cargo build -p varena --example ledger_peer -j1
nice -n 19 python3 release-gate/suites/varena/ledger-cpu/prove.py \
  target/debug/examples/ledger_peer /absolute/new/receipt-directory
```

The proof uses actual library operations in independent processes and threads. It exercises:

- one shared inode, consistent publication by two threads, and 64 independent participant rows;
- explicit refusal of a 65th row and of a 257th registered descriptor, with recovery after close;
- concurrent asks joining one round, cumulative duplicate acknowledgements, and pending bytes excluded from
  completed release;
- a SIGSTOPped owner remaining registered, explicit owner death releasing its row, no fabricated returned bytes,
  and a new slot incarnation refusing the previous donor's acknowledgement;
- death of an active round's requester aborting that round without invented release, followed by a fresh sample
  and a new round; the old acknowledgement remains stale;
- parent death while its forked child survives, inherited-handle refusal, and numeric FD **and registry-slot**
  reuse before old Rust objects are dropped; child cleanup never unlocks the live parent's OFD row;
- 32 actual forks while another thread repeatedly opens/closes ledger connections, after single-threaded hook
  initialization, plus an exec child that inherits no ledger descriptions;
- interruption of the **actual library publisher** while it owns the publication lock and its inactive bank has
  newer row data than the committed bank. The controller stops and kills that exact child before the commit
  store, then verifies that readers retain the old committed bank and another writer can continue;
- rejection of an incompatible layout version. A real namespace transition is attempted separately, and is
  reported as unexecuted when the host denies user/mount namespace creation.

There are no mocked locks, replacement publication routines or fake CUDA drivers. Protocol deadlines fail the
test if a known child does not answer; the library never treats elapsed time or an old heartbeat as death.
The controller owns every process it stops/kills. All actors are closed or reaped, and every result goes into a
new directory; earlier failed receipts stay intact.

`ledger::initialize()` must run before concurrent fork producers start. The first hook registration cannot cover
a fork that was already in progress; this limitation is explicit, not hidden behind a general fork interposer.
Direct syscall fork/clone bypassing libc handlers is outside this checkpoint's supported contract. Every ledger
operation still rejects a changed PID/domain before touching inherited local locks.

The first domain is same UID, user/PID/mount namespaces and boot, using the same tmpfs file. Different backing
inodes are separate domains. Full cross-container/multi-UID support, GPU/MIG discovery, wake/recheck latency,
real donor release, allocator policy and TensorD migration remain unqualified by this CPU proof.

## Exhausted tmpfs initialization

The fixed backing is preallocated before mapping, on every compatible open. `ftruncate` alone can succeed in
a tiny/full tmpfs and then make initialization stores fault with SIGBUS. A failed preallocation can leave a
correctly sized sparse file, so a retry must reserve its backing again. Existing nonzero headers are validated
with `pread` before allocation; an incompatible layout is refused without repair.

Run this negative test only in its private Docker shared-memory mount, never by filling host `/dev/shm`:

```sh
python3 release-gate/suites/varena/ledger-cpu/tmpfs_capacity.py \
  target/debug/examples/ledger_peer CACHED_COMPATIBLE_IMAGE /absolute/new/receipt-directory
```

It uses cached images only, runc, private IPC, no network/GPU and no host shared-memory mount. The actual ledger
must return an I/O ENOSPC error on both first and repeated opens with 16 KiB shm. With 64 KiB it must open twice
without changing bytes. A correctly sized but incompatible sparse header must remain unchanged and report the
layout error before attempting backing allocation. The original reproduced SIGBUS and later fixed receipts are
kept separately; the test does not replace or mock mmap/fallocate.
