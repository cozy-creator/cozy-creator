# Native automatic owner: service checkpoint

This increment moves the automatic donor loop from Runtime's Python thread into
`varena::cooperative::Owner`. It retains the existing finite `Peer::publish` and
`Peer::service` operations and their 500 ms cadence, with immediate native wakes for
lifecycle notifications. The service never attaches to Python, invokes a Python
callback, or uses TensorFS's Python-reference cleanup thread.

The worker exclusively owns Peer and its retained arenas. Its lifecycle handle and
the per-device weak registry contain only host control. Dropping that handle requests
stop; it cannot destroy a Peer, synchronize a CUDA context or join the thread. The
worker attempts capture-aware retirement outside all native guards, retains the
intact Peer while capture/replay excludes retirement, and wakes explicit close after
retirement. Private-VM fork children reject inherited synchronization state.

Runtime retains categorical configuration, actual-use timestamps, status checks and
failure reporting at execution boundaries. Native failures are recorded and reported
without waiting for the GIL. CPU Hello requires the new native-service capability;
an older extension advertises no support to this Runtime. Actual Start still requires
an active native owner. The decoder correction from Runtime b206 remains included.

At this checkpoint allocation/free paths are unchanged. Allocation-driven asks are
the next increment, after the service parity boundary: distinguish an actual physical
allocation refusal, complete local reclaim/retry first, then coordinate outside all
guards and retry the real driver. Root approved a two-second monotonic coordination
budget per recovery episode; expiry is not donor death or released capacity. This
does not bound CUDA/event latency.

Validation so far: native varena/tensorfs-py checks and a release-gate native concurrency
harness covering blocked metadata cancellation, admitted versus queued calls, nonjoining
lifecycle Drop and a real metadata panic. Product source contains no unit tests. Actual
CPU Hello/Shutdown passes for the new native capability and refuses support with the
older native extension. The Linux task identities do not change across Hello; both
cohorts have 32 existing import-time native tasks and no Varena owner thread. GPU parity
remains a separate gate. Receipts are in `release-gate/suites/varena/evidence/native-owner-cpu`. The existing afa/b206 runs 25106/25107 qualify the preceding
Python-pump cohort, not this new native worker.

Before enabling allocation-driven asks, qualify native service while the donor's GIL
is held, overlapping requester/donor roles, stopped publication holders, fork and
capture-aware retirement. Full cache-owner shrink, foreign-pressure/game coexistence
and the draft C consumer API remain outside this checkpoint.
