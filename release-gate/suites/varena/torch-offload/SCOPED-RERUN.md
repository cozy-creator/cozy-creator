# Minimal scoped-capture rerun delta

Apply only the feature commit after the exact bound-fixture context commit.
No product/helper/native code changes or old snapshot adoption are included.

Fixed `graph_capture_sync_gpu.py` SHA256:
`137df5a475bdd3ed5a8b6f66dce51ce1ef7ae188735d2d850a32864284746801`.

The delta combines both concrete corrections:

- Both worker threads explicitly enter inference mode. Actual CPU Torch reproduces
  run26595's identical cross-thread inference-Tensor mutation error without it;
  `thread_inference_cpu.py` verifies exact copy and caller-state preservation with it.
- The existing warm graph actually allocates and consumes pool-backed Working storage.
  Footprint identifies a nonzero allocation in the exact pool, while pointer/data and
  immediate post-refusal native pool/mapping counters provide the remaining evidence.
  Before ANY later prepare, Python AND C10 Storage ownership expires, the stream tail
  completes, existing native trim runs, and the same graph must replay correctly.
  All graph resets followed by trim must release reserved pool bytes. This distinguishes
  graph custody from a retained Tensor or incidental cached capacity. Footprint alone
  is historical metadata, not a live mapping/refcount capability.

The pool/reserve, copy/add kernels and buffers are warmed before arming the unrelated
producer gate. This tests the existing have>=wanted preparation path and native atomic
refusal; it does not claim native preparation/recovery is globally nonblocking.
The matched SDK must include the already implemented hard-cut stream-local helper.
There is no synchronization boolean or global-device barrier.

Coordinator rerun payload, inside its owned ordinary-Cozy child:

```sh
python graph_capture_sync_gpu.py --device 0 --provider "$matched_source_helper" \
  --provider-python "$matched_provider_python" --timeout-s 60
```

Qualification is still REQUIRED. Original H10026595 (bound fixture2cdf0aa1) remains
failed before producer-refusal/custody checks. New pool witness764afaaa was absent from
that run; neither that failure nor CPU thread success qualifies the new CUDA cell.
Read-only inspected result SHA256:
`9c18a97e6a89f1b0331f4aa6b1645673c39cac81814ddeb9b400131f7cb689cf`.

Real small frontend H10026594 passes only its named normal/alias/refill/Plan/old-version/
Conv tuple scope; it is not this helper proof or the remaining epoch failure gates.
No native rebuild, GPU, rental, machine, merge or release action occurred in this lane.
