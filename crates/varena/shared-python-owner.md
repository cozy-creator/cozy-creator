# Shared Python automatic-owner leases

Owner: `/root/p3_owner_restart`; branch `codex/xs138-p4-shared-python`.
Fetched Creator base551980; native integration3df797f (product569649a, including
shared-service db701, multi-arena7a and retirement49685). This is a separate
successor; those frozen branches and external P4 work remain unchanged.

`_VarenaOwner` now calls native `Owner::get_or_open`. Its `shared` and `priority`
arguments are creation defaults only. A ready service in the same native verified
device/ledger domain is inherited without configuration changes. Explicit configure
still changes that one process/device service. `_VarenaPeer` remains a separate
explicit participant with its existing constructor/close lifecycle.

Bound automatic-owner facts include `owner` (slot/incarnation), `clients`, and
`closed`. `active` describes this lease; a closed nonfinal lease is inactive while
another client can keep the service alive. Closing this lease never closes another.
Final close retains the same native object for retry after capture exclusion ends.
Row identity after close is diagnostic, not a live-row or capacity promise.

Runtime's Config is explicit negotiated Start intent, not a generic library default.
The paired Runtime change reads its actual native row before publishing Memory,
checks row identity/PID and sharing/priority, and refuses mismatch without overwriting
the other consumer. Failure releases only the newly acquired lease and preserves
the original exception if cleanup also fails. It reports no successful request
boundary from failed setup, so failed Start cannot train H.

P4's eventual Torch lifetime root must inherit an already configured service or
receive the negotiated creation settings. Creating a contradictory default service
before Runtime Start is not silently repaired by this adapter. Internal PyArena
retirement-request adoption and public P4/ABI changes are separate work.

## Staged real proof

`owner-runtime/shared_python_clients` runs in an ordinary GPU request, checks the
actual Runtime owner, joins two Python leases with deliberately different creation
defaults, validates one shared row and unchanged settings, closes one lease, and
executes/reads real Torch output while the original service remains active.

Its fresh child uses the installed real native library/CUDA context, without a
permanent Torch allocator root. It proves two leases, successful same-config Runtime
adoption, fail-closed Runtime mismatch, unchanged measured-request count, continued
native service after nonfinal close, actual nonblocking-stream CUDA capture, final
close refusal with zero native driver calls, retry on the same object, and disappearance
of the original row in its actual table. The explicit manual Peer is a separate
counter witness. The child must exit normally; failure cleanup never turns it into
a positive result. No fake CUDA implementation is used.

The fixture records actual Torch/CUDA versions rather than hard-coding a historical
13.0 cohort. It remains unrun until the paired native/Runtime artifacts are built and
the coordinator executes it. Source/host checks do not qualify these GPU assertions.
