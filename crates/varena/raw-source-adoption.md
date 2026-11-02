# Owned raw TensorFS Source submission

Owner: `/root/async_integration_review`; branch `codex/xs138-p4-raw-source-adoption`.
Base: fetched master551980610 plus frozen shared Fill/Arena integration3df797f56.
Root approved the narrow plan before product edits. No external P4 ABI/worktree is edited.

## Scope

The first adopter is Work with raw File/Inline items, no decoder, no continuation and no
host-page export. Every input belongs to one GPU and the fixed TensorFS copy streams.
Current joined decode/continuation/host paths remain active until their own producer
contracts migrate; they are not old-version fallbacks. No new memory policy is added.

`Engine::run` currently joins its scoped CPU readers at lines759-764, then records and waits
for end events before releasing RunHold. The new path transfers actual ownership after
host submission closes and records end events without waiting. Per-copy Issued callbacks
remain progress notifications and cannot certify whole-fill closure or completion.

## Proposed native interface

```rust
// TensorFS native implementation surface; public P4 ABI spelling remains separately owned.
pub struct RawFill {
    pub fill: varena::Fill,
    pub items: Vec<Item>,
    pub origin: u64,
}
impl Source {
    // Before writes, validate every range, own the source/files/items and fixed engine.
    // A preparation failure returns the actual untouched input owners.
    pub unsafe fn prepare_raw(
        self: &Arc<Self>, inputs: Vec<RawFill>, threads: usize, mode: Mode,
    ) -> Result<RawRead, (Vec<RawFill>, Error)>;
}
impl RawRead {
    // Unique submitting owner. Every return closes all actual host submitters; errors
    // retain ownership and report their real publication/abort boundary.
    pub fn submit(&mut self, cancel: &ReadControl) -> Result<()>;
    pub fn facts(&self) -> Result<RawReadFacts>; // host-only observations
    pub fn poll_complete(&self) -> Result<Option<ReadStats>>; // native query, never a wait
}
```

These native signatures are implemented; no public compatibility alias is added.
The unsafe preparation contract forbids earlier unaccounted writers through the supplied
Fills and requires every escaped view to own its own Pin. Ordered consumers use the existing
Region::prepare_on / Pin::order_on contract after native publication.

## Ownership and failure rules

- Create shared read-resource custody before any FillSession exposes a destination. It owns
  Arc<Engine>, actual retained staging/native resources, unique timing/end events, owned
  Source/files/items and its checkpoint source context. The binding Work independently
  retains its complete Checkpoint through completion.
- A shared resource cannot trust one Session's completed flag: another Prepared Session may
  retire before a peer submits. It requires its own recorded host-closed state and complete
  end-event set. Before-submission failure explicitly closes the owned operation, so a
  runner that never starts cannot leave permanent Pending by accident.
- All host reader threads finish before publish/abort. Record failure or unproven submission
  retains the actual remaining resources. No new owned path calls io::drain, context sync
  or device sync. Bounded staging-slot reuse still checks its own previously recorded event.
- Claims of shared resources move exclusive owned state out of the metadata mutex before
  event queries, checked release or owner destruction. Pending restores the same owner;
  permanent error quarantines it. Metadata observers never become final GPU owners.
- A successful raw submission publishes whole initialized Regions; partial I/O/cancellation
  before publication seals aborts. Cancellation observed after successful publication cannot
  revoke an already ordered consumer or pretend that queued writes did not occur.
- Submitted/requested/issued byte and CPU-time observations remain distinct from completed
  ReadStats. busy_ns is measured from real completed CUDA timing events, never estimated.
  Existing PreparedRead.done retains checked-completion semantics. Its new raw path does
  not enter the older joined error cleanup after transferring Fill ownership.

## Validation

Preserve the six frozen native async cases. Add an ordinary real-checkpoint GPU fixture
covering file/inline bytes and padding, two independently ordered consumers, dropped source
handles, cancel/partial I/O failure, escaped aliases, another Arena's useful progress and
exact final physical accounting. Native source/binding checks are source evidence only;
root owns GPU installation, execution and rentals. Keep the PR draft.

## Implemented boundary

The engine transfers its real RunHold after scoped readers join. It records end events
without joining them and retains exact recording failures. Even early ordinary errors close
the fixed stream set; recorded start/end timing remains measured where work was issued.
ReaderRing preserves actual staging owners during unwinding. The new path refuses prior
unproven engine work instead of invoking the joined engine recovery path.

RawReadFacts includes the fixed producer stream identities as borrowed native diagnostics,
valid only while native resources remain retained. These are not exposed as a default Python
API, a mutable producer list or a gate method. Each read tracks its own successful enqueues;
a shared cancellation counter cannot misattribute another operation's bytes.

Current PreparedRead success waits actual submitted Fill retirement tickets before reporting
done, so its caller can immediately capture after producer exclusion ends. Native RawRead
completion and ordered consumers remain separately asynchronous. Error cleanup does not
credit a partial logical read, and untouched Fills returned by preparation refusal take the
existing checked abort/immediate-release path. Per-Region publication is valid and whole,
but multi-Region publication is not an atomic transaction.

The real Writer/Checkpoint host fixture passes full byte and validation checks without
loading CUDA. Seven GPU modes are authored in examples/raw_source and documented in
release-gate/suites/varena/raw-source/README.md; all remain unrun. Source compilation and
CPU bytes do not qualify GPU lifetime, GIL progress, performance or public P4 adoption.
