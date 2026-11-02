# Raw Source ownership/failure review

No additional concrete blocker was found in the inspected WIP. This is a source review,
not GPU qualification. [snapshot.json](snapshot.json) fixes every reviewed source hash;
local immutable copies are under snapshot/. The implementation owner remains
`/root/async_integration_review`; no product files were changed by this review.

- **Host closure:** `RawRead::submit` starts every Fill session before handing pointers to
  Engine. Engine joins the actual scoped CPU readers and records a closing event on each
  fixed engine stream before returning. Earlier ordinary errors also take the closing
  event path. A reader panic returns its real ring slots to retained engine custody;
  the enclosing RunHold and RawRead failure paths quarantine rather than certify closure.
- **Shared resources:** the Source, checkpoint, file/inline owners, staging and event set
  remain in one Shared Resources owner. Claim takes that owner out of the metadata
  mutex before host I/O, event queries or cleanup. A Prepared peer retirement sees
  Pending while submission owns it, and poll cannot certify completion until the
  owned read is host-closed and every recorded closing event completes. Returning one
  Resource wrapper does not drop another Fill's physical Pin.
- **Failure/cancellation:** partial I/O aborts Writing sessions; it does not publish
  partially read bytes. Ambiguous memcpy/event errors retain actual owners. Successful
  enqueues are counted before the next tracking-event record, and zero issued bytes
  is never a completion/no-work certificate. Prepared refusal returns untouched Fills,
  aborts them as unsubmitted, and preserves escaped Pins through ordinary Unready cleanup.
- **Completion versus retirement:** raw `poll_complete` means GPU completion, while
  final resource retirement remains separate. The current PreparedRead adapter now
  waits for both Shared resources and all submitted Fill tickets to retire before
  reporting success, preserving immediate capture after successful wait. Failed
  logical reads do not fabricate completed read-byte/time counters merely because
  their GPU work was safely drained. Failure still has no capture-ready promise.
- **GIL/joins:** Python start/wait/cancel boundaries detach from the interpreter; the
  raw source worker and native retirement path call no Python continuation or exporter.
  The new deferred end path does not call Engine recover, stream drain or context join.
  Staging ring reuse still synchronizes its own slot event before overwriting pinned
  host memory. That is an intentional event wait; this review does not claim a fully
  wait-free engine or removal of the other joined decode/export paths.
- **Existing consumers:** raw eligibility excludes decode, prepared decode, continuation,
  host export and event-ordered work. The only current lookahead setter feeds a composed
  continuation, so it cannot be silently swallowed by this raw-only branch.

Two execution scopes remain unproven by the current example inventory: cancellation
*after* a successful enqueue (its reader loop exits without the I/O-error `stopped`
path), and at least two Fills sharing one real Resource retirement owner. These are
qualification gaps, not inferred source defects. The implementation owner was notified
with a concrete gated-read recipe; root owns any GPU run.

Publication is per valid Region. A later sealing failure can follow an earlier valid
Fill publication, just as the prior Work path documents. No aggregate all-or-none
publication guarantee is implied by saying invalid partial source bytes never become
Ready. CUDA/library errors and host panic retention remain separately observable.
