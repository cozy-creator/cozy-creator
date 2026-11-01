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

Proposed API (review before binding adoption):

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

The allocator registry and all clients retain only host Control. The existing
native worker still exclusively owns Peer, retained Arenas and GPU retirement.
No Arc strong-count heuristic, Python pump or second memory controller is added.

## Proof

Pending real host concurrency/lifetime harness in release-gate. Existing 065 GPU
receipts are preserved and do not qualify this source change. Parent owns GPU work.
