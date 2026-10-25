# New native graph diagnostics: actual CUDA qualification

All three real cells **17892, 17893 and 17895 passed** on the local RTX 4070 Laptop
(SM89, driver 595.99.02), after normal update **17890**. The actual TensorD was **4178**,
Runtime was **db7a54bb57a95b59d85276bfa74a03d4ff5022e9**, and TensorFS was **9e95 / 0.15.8**.
This qualifies the new diagnostic module on these paths; it does not identify the cause of
the earlier full-SDXL graph refusal in run 17873.

| Artifact | SHA256 |
|---|---|
| Runtime wheel | `454fb69f1f6d37c8dfef2594a9a78358dd019cfba89ee07700698d0acbfc62a5` |
| New `_execution` module | `e10203af65acdc6a318affa1bc1f7b4057ab0fc02cce0865622bbf07cd378611` |
| TensorFS wheel | `8c5b6f9a7471db87c5193b7396cfff48c2eabe8cef3817741832bf642cfe683b` |

- **17892 — real pinned host copy:** warmup and capture both executed. The native inspector
  refused the actual 1,024-byte H2D graph before Context submission, reporting
  `stage=source reason=memcpy_layout_or_kind type=1 depth=0 local=1 total=1 copy_kind=1`.
  State was restored, then the same request completed a valid device-only plan with two
  GPU iterations, one native launch and two Python preparation calls. The alternative
  Default/Host pointer classification branch did not execute in this run.
- **17893 — watched capture:** capture status stayed `1→1`, the actual watcher resumed,
  maximum convolution error was zero, and two GPU iterations used one native launch.
- **17895 — raw load and execute:** the real 64 MiB checkpoint read completed before the
  native launch; thirty GPU iterations completed while Python was parked for three seconds.
  Completion was proven with one native launch and two Python preparation calls. Cleanup
  reported zero resident and pinned bytes, with no poison. The existing 64 MiB VA reservation
  and one region remained; this is not a claim of zero reservations.

Normal update **17896** restored the known healthy 4178/c1/e521 baseline. The final saved
machine record was ready with zero running or queued calls. The [compact receipt](db7-native-inspection-local.json)
preserves the exact installed/restored hashes, complete returned facts, frozen fixture hashes
and twelve raw-receipt hashes.

The unchanged encoded path, active cancellation, sticky device faults, full SDXL, Windows
and multi-rank execution were not rerun on this new module here. These are correctness and
lifetime checks, not a performance comparison, a graph-whitelist expansion or a release.
