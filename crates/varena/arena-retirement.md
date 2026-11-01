# Native Arena retirement and persistent observer custody

Owner: `/root/p3_owner_restart`, branch `codex/xs138-p4-arena-retirement`.
Fetched master: `ea521be3c` (cl-714). Native base:
`7a653de293d41200a3ebe5f5d8778e01b95d112e`, including coordinator106581.
Integration `bccf2ac48a5f480cb28bacd8a63f115167147c95` changes only upstream CLI/rental
files relative to7a; native source initially matches7a. Frozen cohorts stay untouched.

## Approved ownership direction; API to freeze before frontend adoption

```rust
Arena::request_retirement(self: Arc<Self>)
    -> std::result::Result<ArenaRetirement, (Arc<Self>, Error)>;
ArenaRetirement::facts(&self) -> Result<ArenaRetirementFacts>;
ArenaRetirement::try_complete(&self) -> Result<bool>;
```

A successful request transfers native ownership to a library job. The returned
ticket contains host state only; dropping it never destroys or loses the GPU owner.
Failure to start/enqueue preserves the input Arc. Duplicate requests coalesce by
stable Arena ID. Pending/capture/busy cannot become successful release or death.

Peer and cache-worker persistent observation roots become explicit detachable
observer tokens. Registration of new persistent roots closes when retirement is
requested. The native job detaches only those roots, under per-device retirement
exclusion and outside token/global/arena/working/ledger locks. Actual Region, Pin,
Fill, Plan, producer, source/staging and transient admission references remain
genuine owners. No reference-count subtraction or forced drop is permitted.

Observers continue taking temporary current leases for accounting/reclaim. Host
driver counters survive observer GPU detachment, so instrumentation and pressure
measurement epochs cannot decrease or accidentally match across retirement.

Final Arena Drop transfers its owned State and shared Driver into a native resource
job. Registration distinguishes Active from an accepted Finalizing handoff. An
Active entry with an un-upgradable Weak but no accepted handoff remains Busy. A
Finalizing entry exposes exact host-only remaining physical bytes, conservatively
nonreclaimable, so an unrelated live arena can continue. No queued disposal or
unknown completion is capacity credit. Counters decrease only after actual release.

The per-device worker holds retirement exclusion through cleanup, but no global
capture/arena/working/ledger/ABI-registry mutex across GPU waits or destructors.
Known consumer events are queried, not context-joined. Unjoined/unproven consumers
are explicitly refused/quarantined with intact backing/accounting; there is no
hidden context/device synchronization fallback. Active consumers still producing
unjoined releases must migrate and qualify real stream/event completion.

The private shared queue starts with owned Arena jobs:

```rust
retirement::enqueue(device: i32, job: Job)
    -> std::result::Result<(), (Error, Job)>;
```

The async-fill owner will add the typed FillResources variant in a recorded
follow-up, retaining its separate completion-event poller. The integrated cohort
must have one per-device final-resource queue, not competing finalizer services.
Checked resource-retirement errors must feed quarantine without dropping remaining
producer/source owners or claiming physical cleanup.

## Required proof

Real GPU cells must retain both persistent observer stores, keep an unrelated cache
owner/service alive, and close an observed Arena without closing those services.
Hold a genuine Region/Pin/Plan/transient lease and prove retirement stays pending;
then release it behind actual completion and verify contents, accounting and final
release. A new observer arriving after the request must still account physical
bytes without taking a new persistent GPU root. Live Arena A must continue while
unrelated Arena B is Finalizing. A second-device cell must make progress while the
first has a pending event. Actual capture, request failure/drop/abandon and explicit
unproven-completion quarantine are separate cases. Source/host checks are not GPU
qualification. Parent owns every GPU/install/rental action.

The user's pre-launch hard cut is recorded in tracker40cd9957. Removing old-layout
migration is a separate successor delta: this retirement change adds no compatibility
and never resets or reinterprets a live table.
