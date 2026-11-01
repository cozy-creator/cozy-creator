# Native asynchronous Fill fixture

Implementation and fixture are source-only until the build/GPU operator records results.
Build the example from the exact same source as its private native core; preserve both
binary and dylib dependencies and hashes. No Python or Torch is loaded by the child.
Root owns builds, installation, GPU identity and rental/termination.

```sh
cargo build --locked --offline -p varena --example async_fill -j 1
# Invoke each mode in its own child process with the observed GPU UUID:
async_fill ordering GPU-... 0
async_fill abort GPU-... 0
async_fill capture GPU-... 0
async_fill abandoned GPU-... 0
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

Still separate/unexecuted gates: real driver failure during partial event recording, source
worker-start refusal, inherited post-CUDA Session refusal, last-Arena retirement concurrent
with another ticket, multiple-GPU isolation, and the TensorFS/public asynchronous Source
adapters. No error injection hooks or mocked CUDA completion exist in this fixture. Existing
synchronous API, checked joins and Source ABI remain unchanged by this native slice.
