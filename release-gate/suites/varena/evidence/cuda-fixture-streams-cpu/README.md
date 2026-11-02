# Explicit-stream fixture compilation

The manifest identifies both fixture-only commits. They replace unsafe pageable-copy
completion assumptions and unproved gate initialization with owned nonblocking streams
and checked stream completion. Cache helpers no longer use cuCtxSynchronize; setup
stream close is checked before a successful result is reported. Existing copy+commit
was safe and was replaced only to avoid a context-wide helper join.

Async/raw examples and cache-pages helper compile, one job/nice19. No production code or
public C/Rust example changed. Prior frozen sources/receipts stay preserved. No CUDA or
GPU rental was used locally, and none of the affected GPU modes is qualified by compilation.
The exact minimal diff is also saved in outputs/xs138-cuda-fixture-streams/fixture-only.patch.
