# Owned committed eager execution

Initial consumer: Runtime's explicit single-native-linear phase. A native commit copies
the current immutable configuration, exact legal allocation menu and active Region Plan
generation, selects one choice before phase entry, and owns the resulting fingerprint.
The caller cannot supply a previous successful key as the current execution.

Source/refill and actual binding validation happen before commit/compute entry. An
execution token is not a raw observed key: it owns the fixed choice, rejects reuse or a
changed Region generation, and enters one nonnested eager phase. The operation begins
with that choice without calling the selector again. Completion uses actual stream
ownership; incomplete/aborted execution preserves earlier successful samples. Existing
external-callback invalidation remains authoritative.

Metadata preparation does not pin/map/fill Regions or allocate working tensors. The
first contract contains one declared native operation and no arbitrary compute callback.
General opaque loops and whole-H3 prefetch are not enabled by this foundation. Cache
owner callbacks and cross-process ownership remain in their existing modules/branches.

This is a source design checkpoint. Native implementation and CPU/GPU qualification are
separate gates; root owns all GPU execution and paid resources.
