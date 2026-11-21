# xs160 merge-library results

**Automerge is the stronger fit when Cozy must expose unresolved prompt/approval alternatives directly.** Both real Rust libraries merged the common offline metadata model, preserved distinct version additions, saved/reloaded further offline edits, and converged with duplicate/reordered delivery. Loro's ordinary map reads select one LWW value; history recovers losing values, but identifying which historical writes remain unresolved requires additional domain work. This experiment does not implement that work.

Executed 2026-11-09 America/Denver (2026-11-10 UTC), native x86_64 Linux, Intel Core i9-13950HX, Rust 1.99.1, release profile, build jobs 2. Exact crates.io verification found Automerge 0.12.0 and Loro 1.17.1 current stable and not yanked. These are pinned with Cargo.lock. The final binary completed with exit 0 and no compiler warnings; every scenario below ran against the libraries, without mocks. The complete output is run.log/results.json.

| Case | Automerge 0.12.0 | Loro 1.17.1 |
| --- | --- | --- |
| Four replicas; disjoint title, description, asset-name edits | All three edits present; converged | All three edits present; converged |
| Two replicas add different version IDs to one existing asset | Existing version plus both new versions present | Existing version plus both new versions present |
| Three concurrent prompt writes: dawn, dusk, noon | get_all returns all three with actual operation IDs | get returns the peer-103 winner, noon |
| Three concurrent approval writes: approved, rejected, needs-review | get_all returns all three with operation IDs | get returns the peer-103 winner, needs-review |
| Conflict snapshot/reload and causal resolution | All three alternatives survive reload; later write reduces current get_all to one | All three old branch states recover via actual checkout after reload; latest read still selects one |
| Delete title at 101 vs write at 102 | Concurrent write survives | Write survives |
| Delete title at 103 vs write at 101 | Concurrent write survives; deletion is absent from get_all | Delete wins; title absent |
| Delete whole asset vs add version inside it concurrently | Asset absent from normal tree; new version readable through retained object ID | Asset absent from normal tree; new version readable through retained container ID |
| Save, reload, make more offline edits, exchange with still-offline replica | Title, later prompt, and other replica's added version converge | Same |
| Four different seven-packet orders, duplicates, dependent update before seed | Equal normalized state and equal causal heads | Equal normalized state and equal version vectors; pending statuses recorded |
| Save while a received update lacks causal dependencies, reload, then receive predecessors | Ordinary save/load retains the queued update in this case | Ordinary snapshot loses the queued update; snapshot plus export_pending_updates preserves it |
| Concurrent creation under the same version ID | Both child maps visible through get_all (dawn and dusk) | Visible child is one LWW winner (dusk) |
| Mutate an existing version's prompt | Mutation succeeds | Mutation succeeds |

## What changes the choice

Automerge's plain JSON serialization also picks one visible value: Cozy must deliberately inspect get_all at the fields where alternatives matter. The tested prompts are 15@0000000000000065, 15@0000000000000066, and 15@0000000000000067; approvals are the corresponding operation 16 values. These alternatives remain inspectable after snapshot reload and disappear from the current multi-value read after an observing replica writes a resolution. This is the direct behavior required for a conflict UI. It does not implement semantic validation or keep separate field values paired into a business decision automatically.

Loro demonstrably retains losing values: loro-conflict-history.json contains all three two-op changes, each depending on 13@100 at Lamport 14, and actual checkout at Frontiers([1@101]), Frontiers([1@102]), and Frontiers([1@103]) returns the three original prompt/approval pairs. The saved frontiers in this experiment identify known branches; this is not an implementation of automatic conflict discovery for arbitrary histories. A later causal resolution is also retained alongside prior writes in loro-after-resolution-history.json. An application cannot label every old history value an unresolved conflict. Loro's get_last_editor reports the winner, not every concurrent candidate. Its documented map semantics use Lamport timestamps and peer ID to choose the winner. See the [official map documentation](https://www.loro.dev/docs/tutorial/map) and [pinned Rust API](https://docs.rs/loro/1.17.1/loro/struct.LoroDoc.html).

Neither library's mutable map enforces immutable asset versions. The experiment proves that unique additions coexist and also that the same ID can collide and existing version fields can change. Distributed-unique version IDs and the rule to add a version instead of rewriting one remain application responsibilities. Likewise, neither library makes the tested parent-delete/child-edit conflict visible in the ordinary asset tree. Even Automerge's same-slot delete/write test keeps only the write in get_all; delete intent is not a current alternative. If Cozy must present delete intent alongside edits, direct hard deletion is insufficient for that requirement and the domain representation must be chosen explicitly. No deletion policy was implemented here.

Loro's missing-dependency persistence behavior is material for an offline file checkpoint: import status must be inspected, and pending bytes persisted separately with a consistent snapshot, or retransmission must be guaranteed. The real test observes snapshot-only failure and snapshot-plus-pending success; this is recorded as a limitation, not changed into a pass. Automerge's save/load retained the pending change in the same experiment. This does not claim crash-atomic persistence or arbitrary transport recovery.

## Small local measurement

Three repetitions, 1,000 assets, two versions each, one seed commit, identical normalized logical contents. Medians below; all samples are in results.json. No warmup, affinity control, or isolated process was used. Timing boundaries are in-process operations and exclude JSON assertions, compilation, and disk I/O. The host was shared. These numbers do not choose the library.

| Operation | Automerge | Loro |
| --- | ---: | ---: |
| Create metadata (ms) | 192.271 | 15.239 |
| Save full snapshot (ms) | 1.828 | 9.557 |
| Reload snapshot (ms) | 8.924 | 0.472 |
| Import one-field delta (ms) | 0.080 | 0.093 |
| Full snapshot bytes, identical in all three runs | 22,845 | 209,854 |

The measured formats differ: Automerge's compressed snapshot versus Loro's state-plus-history snapshot. Metadata is highly repetitive and blobs are synthetic reference strings. Production content, long edit histories, text operations, incremental checkpoint policies, and concurrent workload were not measured. The binary also records replica-load-plus-delta preparation and delta size; raw Automerge change bytes and Loro encoded update bytes have different framing.

## Native build and qualification boundary

Both exact libraries compiled and ran natively with Rust 1.99.1. The initial dependency/release build took 2m 07s on this host with two build jobs; subsequent source rebuilds took seconds. A separate cargo +1.91.0 check --locked -j 2 also passed for the pinned lock (see rust-1.91-check.log); that is a type/build check, not a runtime execution with 1.91. The build traverses native build tooling (including cc/psm/stacker dependencies), so this does not establish a C-toolchain-free build or qualify every Creator target. No cross compilation, WASM, platform packaging, historical-format migration, or Creator integration was performed.

The experiment uses one scalar string per prompt to preserve full alternatives. It does not evaluate collaborative text edits, authorization, author identity, malicious updates, immutable content addressing, multi-document transactions, media storage, or compaction/shallow-history retention. It qualifies these pinned native Rust APIs and the shown synthetic cases only.

Recommendation: take Automerge forward for the next project-metadata prototype if direct unresolved-value inspection is a firm requirement. Keep deletion policy and immutable-version enforcement explicit. Loro remains viable if its LWW read semantics suit the domain or the application deliberately supplies semantic conflict tracking; this evaluation does not qualify such tracking.
