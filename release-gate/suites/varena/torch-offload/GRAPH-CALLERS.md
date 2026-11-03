# Capture preparation caller audit

Read-only source audit before replacing `vt.graph`'s device-wide barrier. This is
ordering review, not GPU qualification. No Runtime/coordinator/frozen source was edited.

Audited source identities:

- Active Runtime integration: `7adc1e82c39e451a311ec67eb0b640cf1e4d7bca`.
- Preferred audited native/SDK fixture source: `b40f91f736a8a713229465c631e3b475428c6fb5`.
- Runtime primary: `44f17aedfecffada88c00de1346ff27d552471ed`; its current graph
  capture uses direct Torch, so this helper change does not alter those callsites.
- This graph lane: `fb3eb7a1efa3a1dd7b50b4411a8085dd0334fc12` before this amendment.

The active Runtime integration has three calls:

| Runtime caller | Preparation dependencies already present |
| --- | --- |
| `graphs/__init__.py:89`, capture | Control waits caller and work streams before helper entry. Fork/join events also connect the work stream during capture; caller joins both afterward. |
| `graphs/domain.py:146`, initialize | Event-only owner graph on the explicit domain stream, with no static-data/kernel dependency on the other control stream. Helper's caller join preserves preparation ordering. |
| `internal/loop.py:1662`, stage begin | Static index is initialized on caller; work waits caller and control waits work before helper entry. Capture forks/joins control and work explicitly. |

The audited SDK has nineteen direct calls in release-gate clients. The line numbers
below refer to the exact audited source above, not the amended graph lane.

| Direct caller | Data/readiness dependency |
| --- | --- |
| `cache-pages/.../runtime_owners.py:282,352` | First case explicitly synchronizes before capture; second creates index/state on caller. Caller -> capture join preserves those writes. |
| `committed-linear/.../protocol.py:169` | Capture-only scalar created on caller; no unjoined side producer. |
| `native-tensor-export/deterministic_gpu.py:392` | Selected Source gate is opened, producer stream synchronized and native retirement settled before capture. Scratch is caller-prepared; unrelated sentinel is armed afterward. Its comment mentioning the old helper device sync is historical, not a dependency. |
| `native-tensor-export/failed_tail_gpu.py:41` | Weight/copy/scratch are prepared on explicit stream, synchronized, and Source retirement settled before capture. |
| `native-tensor-export/parts_gpu.py:161` | Old/new sources are materialized, exact operation warmed on explicit stream, stream synchronized and native retirement settled. |
| `native-tensor-export/use_ready_gpu.py:212` | Selected producer gate opened and actual `Use.wait_ready` succeeds before capture. Scratch initialization is on caller; helper adds caller -> capture ordering. |
| `owner-runtime/.../__init__.py:209`, `pressure.py:152` | Capture/refusal/service observations with caller-prepared data or no captured input; no data dependency on an unjoined side producer. |
| `p0-working/.../cooperative.py:261,341` | Service/cleanup controls, with no captured side-stream data read. Legacy checkpoint read is ready before exposure. |
| `p0-working/.../probe.py:57` helper and `:331,686,766` | Scalar/working inputs initialized on caller or explicit capture stream. Retained/shared/distinct-pool and hook cases explicitly settle before capture; remap case explicitly joins caller. cuDNN trial/warmup is on the same explicit stream, with checked CPU readback. Native checkpoint read is ready before exposure. |
| `p0-working/.../raw_prefetch.py:38` | LoadSlot exposes already initialized capture-only storage. Native `load.rs` documents engine-stream initialization joins before export; output/index are created on caller. It is not a pending asynchronous public Source publication. |
| `p0-working/.../replay_policy.py:34,147` | Output/index initialization is on caller; second is intentional capture-body failure. |
| `stock-memory/.../native.py:227` | Fill is committed on caller; exact operation warmed and synchronized on explicit stream. New caller -> capture join also preserves the committed caller writes. |

Legacy `_VarenaUse.__dlpack__` documents Ready fills as context-synchronized and
empty fills as producer-free (`src/varena/mod.rs:575`). LoadSlot's initialization
explicitly joins its engine streams (`src/varena/load.rs:215`). Neither fact is a
new guarantee for asynchronous public Source: its callers still need scoped
`Use.wait_ready` before capture, plus explicit dependencies for other input streams.

The new adapter already joins the caller, acquires/exports/waits selected Sources,
and prepares/warmups static inputs on its explicit stream before helper entry.
Its old opt-out feature checks are removed. The new GPU fixture checks real
three-stream ordering, unrelated-producer refusal on the already prepared exact
pool, capture-body failure cleanup and subsequent successful replay.

No active source caller was found relying on the helper to complete an otherwise
unjoined data producer. No caller migration is required by this bounded audit.
Unknown external callers must adopt the hard-cut contract: explicitly join their
producer streams. The helper still prepares allocation/pools outside capture,
may wait/recover there under pressure, and atomically refuses unrelated active
native producers at capture admission. No globally nonblocking promise is made.
