# Full SDXL native interval on RTX 3090

Run **17882 passed** the existing strict model validator: one eager denoising callback plus
one fresh retained `request_path.denoise_native_steps=29` observation accounts for all
30 requested steps. Required metric gaps were empty. A valid 1024×1024 image was delivered
and visually checked. This is one correctness run, not a matched benchmark.

The actual cohort was Creator `8499b11b2`, TensorFS `9e95ee586` / wheel `8c5b6f9a…`,
packages `a375591645`, and diagnostic Runtime **`3826fcf3b27dc62f6708e6129144b32989603699`** /
wheel `1db9ce7ecf19917d7f8818707c446a0e9228ffb7a168e5f823b943e55ba20abd`.
That Runtime adds only Python refusal-path diagnostics to production source `7d1e7854c`.
All four native module hashes matched 7d1 exactly; the refusal diagnostic did not run.
This does not qualify the newer db7 native diagnostic binary.

| Observation | Actual value |
|---|---:|
| Device | RTX 3090, SM86, 24 GiB |
| Requested denoising steps | 30 |
| Eager callbacks | 1 |
| Retained native completed steps | 29 |
| Reported execution time | 23.263s |
| Request creation to settlement | 37.999s |
| Denoise stage total | 10.303637s |
| Individual native-step timings | unavailable |

The delivered WebP is 1,305,448 bytes, SHA256
`131baba7c723b3e115e4501b678729affefc2d0520ad1cf92695b27e092ef7c0`.
Its decoded RGB hash is `4f6655a9a2d2de9cde19d2fa46257fa179bfcde3487e297b36f6d24e4b40ee5b`.
The image depicts the requested brass observatory in snowy mountains. No per-step average,
isolated cold-weight cost, GPU starvation time or physical transfer rate is inferred.

The [compact receipt](sdxl-17882-native29.json) preserves exact artifact/source identities,
the unique native metric, raw receipt hashes, image facts, and the validator reprocessing.
The `mikhail` rental was confirmed ended with zero running/queued calls. Reported Hub spend
was unmetered; the receipt does not assert provider settlement.

The successful 24 GiB SM86 run does **not** identify or fix the earlier 8 GiB SM89 graph
inspection refusal in run 17873 or the separate capacity refusal. Those failures remain
preserved. Hardware, driver and memory capacity differ, so this is not an isolated A/B
comparison, a universal graph eligibility claim or production release qualification.
