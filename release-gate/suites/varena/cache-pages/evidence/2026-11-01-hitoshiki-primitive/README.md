# Native paged Cache primitive on Hitoshiki

Independent audit passed runs 25358, 25359 and 25360 using exact source e54525838,
binary 76e0bcd23b48514b6ac8bebf836b19c2f53b2002206ab5a2e2bcd0de2a623ba8 and
GPU-18d960a8-2283-19c9-c060-ef8a22fabe35. The raw run detail and extracted result were
compared exactly. Child PID/birth/pidfd, ELF hash, native library maps and parent SDK
provenance matched. The three native children loaded CUDA but neither Python nor Torch.

| Run | Actual result |
| --- | --- |
|25358 pages|2MiB granularity; middle-hole release/recommit preserves its base and full neighboring sentinel readbacks. Three actual blocked release events protect 6MiB, including a dropped pending owner. Working phase peak is 2MiB beside 8MiB of live Cache, not 10MiB. Local cap refusal changes no backing. Actual capture rejects Cache new/commit/release/poll/close with native working-driver counter 124→124. Final native Cache/working/pools/events/physical bytes are zero.|
|25359 rollback|A live 23,517,462,528-byte filler was fully read back. An impossible whole-device Cache commit produced 3 real physical OOMs and 963 memory calls: 192 successful page mappings were rolled back. Exact old 4MiB ranges, base and full readbacks survived. Normal final native physical ownership is zero.|
|25360 abandoned|Native last-owner drop without a release contract retains exactly 2MiB of live backing; trim makes no memory calls (4→4), the sentinel remains fully readable, and reclaimable/pending-release stay zero. The isolated child exits 0. This is retained-safety proof, not successful native close or inferred physical availability from process death.|

The parent used TensorFS 4dc and reported that the new Python Cache binding was absent.
That is intentional: these are separately linked native consumer tests. Python binding,
optimal split choice, callbacks/GIL progress and actual Runtime graph-owner retirement
remain separate gates. No full-model, release, deployment or provider-settlement claim
is made here. Root owns rentals and all GPU/SDK operations.
