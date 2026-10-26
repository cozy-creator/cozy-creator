# Current capability-integrated GPU cohort

All four checks passed with the exact Creator **51a**, Runtime **dfef**, TensorFS **51a**
and packages **a25** cohort on the local RTX 4070 Laptop (8 GiB). The source and artifact
identities are retained in the [compact receipt](51a-dfef-local-cohort.json), including
22 frozen input hashes and the raw run/restoration receipts.

| Check | Run | Outcome |
|---|---:|---|
| Resident component owners |17923|Passed ownership, eligibility and checked cleanup assertions.|
| Watched convolution capture |17924|Capture remained valid; exact output and watcher resumption passed.|
| Raw native loading |17926|Real stored input fed native GPU execution with checked completion.|
| Full SDXL, 1024×1024, 30 steps |17927|One eager callback plus a unique fresh metric of 29 native completed steps; no required metric gaps.|

The actual portable artifact hashes were:

- CLI: `9f0e3578fc2788d1c2220107a479aa4f9c8eb908e83fc099ea6b79a42b081650`.
- Runtime: `fa10c26fbe1bd1c845073818b652641cd39681fa4c1c1229c32822c2560fcbb3`.
- TensorFS: `e8dc43e2818bfcd1e8cfdec69d5d4fb9e28ec8549e06c15776341f675ada94ac`.

The full image passed the existing strict validator, decoded at the requested size, and
matched SHA256 `cd7c02b1c6dfb0fc565b3822adb30f84ebe8aeefa10b48b2b414785036bb59f1`.
The owner visually confirmed the requested lighthouse scene. Native per-step timings remain
unavailable; the recorded request and stage times are not a performance comparison. Host
load was approximately 42 and the request included first-use/preparation costs.

Normal restore **17928** returned the exact preexisting SDK hashes, then verified ready
state with no queued or running calls. This statement describes the saved restoration
record; another owner's session began afterward and was left untouched.

This proves a full native interval on this 8 GiB cohort. It does not isolate the cause of
the older run 17873 refusal, prove optimal memory movement, or qualify the subsequent
resident prefix/suffix implementation. No release, platform-wide, multi-rank or benchmark
claim follows from these correctness checks.
