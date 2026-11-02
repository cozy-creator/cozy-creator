# Native update custody through Calcifer

The current CLI is a client of Calcifer's update endpoint. Local checkout builds
use the existing native bundler, and Calcifer measures the original runnable
binary's version before packing immutable private dependencies. The transport
archive's digest/length is what the ordinary stager uploads.

Temporary bundle directories are transferred into the existing blocking CAS and
SQLite journal stage. They remain live if the HTTP caller drops its future or a
concurrent same-key update loses the journal race. No duplicate staging or network
path is introduced. Generated directory names are removed from the submission
identity; original file paths and actual archive content digests remain.

The existing Shared/Dedicated category travels in MachineUpdate and the ordinary
update payload. Mode-only requests do not resolve Hub software targets. Mode causes
the existing drain/restart warning and must match live machine status afterward;
software `to` cannot establish mode for an unreachable machine. The native updater
and rollback implementation remain unchanged.

Host proof uses an actual private-linked cozy binary and its native libraries,
real CAS files and a real isolated SQLite journal. It tests bundle/version custody,
concurrent idempotency and cancellation while the real staging lock is held. The
proof's machine has no endpoint and no live machine is changed. Source/build/host
proof does not qualify a live restart or GPU propagation.

## Local result

All four real host proofs passed (313.90s), including canceled-caller custody and
two same-key submissions through the actual journal. The exact private-linked
cozy input reports 0.15.7 before and after debug-only stripping and archive staging;
its sidecar libraries remain unchanged. The complete calcifer/CLI/protocol
all-target check passed, then the final incremental check passed in 25.75s.
Receipts and compressed original logs are under `evidence/native-update`.

Activation and rollback source matches 0fbbc114f byte-for-byte; its earlier native
rollback receipt is preserved and **was not rerun** by this task. No live machine,
release or GPU was changed. Windows now requests the bundler's required `lief`
dependency; actual Windows packaging execution remains untested.
