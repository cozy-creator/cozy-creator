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
