# Native asynchronous Fill fixture

Implementation and fixture are source-only until the build/GPU operator records results.
Build the example from the exact same source as its private native core; preserve both
binary and dylib dependencies and hashes. No Python or Torch is loaded by the child.
Root owns builds, installation, GPU identity and rental/termination.

```sh
# Build the complete dependency cohort described in ../native-gpu/README.md.
# Do not overwrite its dependency core with a primary -p varena build.
# Invoke each mode in its own child process with the observed GPU UUID:
varena-async-fill ordering GPU-... 0
varena-async-fill abort GPU-... 0
varena-async-fill capture GPU-... 0
varena-async-fill abandoned GPU-... 0
varena-async-fill retirement_pending GPU-... 0
varena-async-fill arena_retirement GPU-... 0
```

`ordering` keeps one of two actual producer streams behind a CUDA memory-value gate,
queues real H2D copies, publishes while the gate is closed, and refuses every legacy
completed-only read route. Two independent consumers obtain native producer waits and
copy the full region. Their Pins release early with separate consumer fences; backing
must remain pinned by the producer. Opening the real device gate allows both outputs to
be read completely (first half41, second half73). Actual source destructor counters must
remain zero while pending and become exactly one only after checked completion. The same
VA remains ready until explicit reclamation, with no context join used for these Pins.

`abort` submits only the first half, declares failure/closed submissions and checks that
neither publication nor completion is invented. An escaped Fill Pin prevents reuse after
producer completion; its actual initialized half is read. After checked alias release,
the cold backing releases and an ordinary synchronous refill succeeds at the same VA.

`capture` brackets a real cuStreamBeginCapture/cuStreamEndCapture with Varena's capture
scope. begin refuses before destination exposure and retains resources/Prepared facts.
After capture ends, the same Session begins, excludes new capture while open, and performs
a declared no-copy abort. Producer completion and native resource retirement must finish.

`abandoned` is a separate process: real writes are made but the application never reports
submission closure. Dropping that Session must quarantine its actual source resources and
backing even though the fixture privately synchronized its own producers. The full bytes
remain readable and no resource destructor runs. This is intentional retention until child
exit, not a cleanup pass or recovered-capacity claim.

`retirement_pending` is the checked-contract successor's real resource test. A private
native Working allocation has actual gated CUDA work after the fill producers complete.
Its checked retirement returns Pending from cuStreamQuery, while the second source
resource really retires. The ticket must report completed data, one remaining resource
and unfinished retirement; the private bytes and capture exclusion remain owned. Opening
the device gate permits checked stream destruction/Working retirement and final readback.
No fabricated Busy flag or Python/GIL action supplies this result.

Still separate/unexecuted gates: real driver failure during partial event recording, source
worker-start refusal, inherited post-CUDA Session refusal, last-Arena retirement concurrent
with another ticket, multiple-GPU isolation, and the TensorFS/public asynchronous Source
adapters. No error injection hooks or mocked CUDA completion exist in this fixture. Existing
synchronous API, checked joins and Source ABI remain unchanged by this native slice.

Integration boundary: native Arena retirement49685f087 is composed in this follow-up.
Final Arena resources use completion-event queries without context/device synchronization,
and both Arena and Fill resources share one typed per-device retirement queue. The event
poller is independent. Source/host compilation does not qualify last-owner or cross-device
progress; the actual GPU fixtures remain unrun. Genuine producer/Region/Pin leases are
never detached as observers or inferred from Arc strong counts.

`arena_retirement` composes the two native ownership systems. It requests retirement
while a published Fill and an ordered consumer hold real owners. Separate device gates
retain producers and consumers independently. A second Arena completes full useful D2D
readbacks while the first is producing, finalizing, and fully retired. Producer completion
retires source resources but cannot reclaim the still-gated consumer's backing. Dropping
the final Pin/Region queues Arena resources; exact physical metadata stays visible until
the actual consumer fence completes. This mode is authored but unrun, and does not
claim shared peer-service observation or multi-GPU qualification.
