# Shared automatic service: native lifetime checkpoint

Owner: `/root/p3_owner_restart`; branch `codex/xs138-p3-shared-service`.
Base: freshly fetched Creator `1b77958bdfb3f699db8340951c9b5cc1805d4243`,
merged with frozen native v3 `0657802458dfbf4fb7e66613e30c9baef1308e68`.
Integration commit: `e15d3e49a23930bd667aa14a13cafda7feddfe5b`.
Native product crates initially match 065 exactly; the fetched base adds only
`release-gate/analyze.py` and `release-gate/runs.py` relative to that product cohort.
Versions remain Creator 0.15.7 and TensorFS/Varena 0.17.16. Runtime 6c812 is unchanged.

Scope: one native cooperation service per process/device, shared host-only client
leases, final retirement, categorical configuration and owned request boundaries.
No C ABI/Python public binding adoption, package bundling, allocator byte-policy
change, ledger schema change, GPU execution, merge or release is included.

API for review before binding adoption:

- Keep `Owner::open` as exclusive creation.
- Add `Owner::get_or_open` with initial sharing/priority used only if creation is
  necessary. Existing service configuration is inherited unchanged.
- `close` releases this client exactly once. Only the final client requests native
  retirement; checked final close retains a retryable host ticket on refusal.
- A matching ready service is shared; Opening/Stopping refuses with Busy, permanent
  failures remain faults, and only proven retirement allows a fresh generation.
- Device descriptor, canonical ledger directory and Linux scope must match.
- Closing a client with an unfinished request abandons its measurement; it cannot
  complete another client's request, train H, or leave the surviving service Active.
- Each unreported live client preserves Unknown; one client's completed request
  cannot label every other client Idle. Active requests take precedence over Unknown.
  A client appearing during a request invalidates that window even if it closes
  before the next sample. No attribution is inferred for that interruption.
- A permanently failed service leaves only its original failure text in the registry,
  so losing the final client cannot reopen a quarantined Peer. Retryable Busy is not
  a failure tombstone. Normal completed retirement still permits a new generation.

The allocator registry and all clients retain only host Control. The existing
native worker still exclusively owns Peer, retained Arenas and GPU retirement.
No Arc strong-count heuristic, Python pump or second memory controller is added.

`Owner::configure` is an explicit process/device operation; it retains the existing
native serialization and Dedicated acknowledgement. `get_or_open` does not queue
Configure. Both automatic Runtime/Python acquisition and the external P4 bindings
must eventually use `get_or_open` for C-first and Runtime-first orders to work.
The existing exclusive Python binding is deliberately not changed before review.

`Facts.clients` counts application leases, and `Facts.closed` describes this handle.
`Facts.active` is false for a released handle even if another client keeps the service
active. Closed handles retain diagnostics and idempotent close; other operations
refuse. A successfully retired old close never consults a newer service's capture
or callback state. No cross-process commit lockstep or new ledger namespace exists.

## Proof

The real host harness is `release-gate/suites/varena/native-owner-cpu/prove.py`.
It appends visibility-local fixtures to an output-only exact source copy; no product
unit-test module or fake CUDA Peer is added. It exercises concurrent admission,
client close orders, real tmpfs row identity/configuration, final close retry under
actual capture counters, owned boundary cancellation and failure persistence. Its
host Ready/retired transitions do not stand in for GPU initialization or retirement.

Receipts are under `outputs/xs138-p3-shared-service` in the workspace. The initial
host-001 refusal of a non-tmpfs directory is preserved; subsequent runs use unique
owned 0700 `/dev/shm` directories. The expected intentional native-thread panics test
actual poisoned metadata. They are not GPU faults. Existing 065 GPU receipts remain
separate and do not qualify this source change. Parent owns all GPU work.

Still unrun: real first CUDA service creation/failure retry, mixed C/Rust/Python and
Runtime service adoption, physical donation after nonfinal closes, true capture/GIL
final retirement, and CUDA/Peer quarantine. These are required before GPU/release
qualification; the public API, wrapper teardown and packaging remain P4-owner work.
