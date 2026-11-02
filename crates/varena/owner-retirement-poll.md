# Polling a native service's retirement

Owner: `/root/p3_owner_restart`; branch `codex/xs138-p4-owner-try-close`.
Frozen dependency: `1db60d8492b4cd65268e4cab2ba0243bc82fbd12`.

`Owner::try_close(&self) -> cooperative::Result<bool>` releases exactly this
client and observes only its original host Control. Nonfinal closed clients and
successfully retired old final tickets return true. Final retirement in progress
returns false. A held host mutex returns typed Busy without waiting; a permanent
fault returns the original failure while the handle/native custody remain live.
Inherited PID rejects before any lock. No CUDA, capture/dispatcher check,
replacement-service lookup, Condvar wait or new queue occurs.

The existing checked `close` behavior and public Owner Drop are unchanged.
They share only the existing client-release state transition with the poller.
The public ABI worker can hold its Owner through Arena-ticket completion, then
poll service retirement without blocking work for another device. A successful
Arena ticket alone does not prove the final participant is gone.

Real host lifetime/fork/concurrency and pending real-service fixture checks are
staged separately. Source compilation is not GPU qualification.

Native plus the existing shared-cohort Cache GPU helper compiled offline with
Rust1.99.1, nice19 and one job in2.73 seconds. The exact-source host proof passed,
including held-Control Busy without releasing the client, nonfinal idempotence,
final pending while the global capture mutex is held, inherited locked Control,
completed old tickets across a new service/domain, and original poisoned failure.
Receipts: `outputs/xs138-p4-owner-try-close/host-001/report.json`.

The ordinary Cache-controller `/owner_try_close` entrypoint is staged but unrun.
Its native child opens a real service, proves nonfinal close, then observes final
pending twice under actual CUDA stream capture, zero native driver calls, and a
live participant row. After capture ends it polls real retirement/row removal,
opens another service generation, and polls the old completed ticket during a
second actual capture. The manual Peer serves only as instrumentation. It makes
no default-discovery, physical donation or broader GPU-memory qualification claim.

Open custody boundary: global Working backing and the installed Torch allocator
can outlive individual public Gpu/Allocation handles. This per-client poller does
not add a process-lifetime keepalive or claim that all such backing was released.
The parent owns the separate keepalive contract review before full lifecycle
qualification; no new policy or premature default-service creation is included.
