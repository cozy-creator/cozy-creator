# Qwen Image 2.1 phase-policy comparison

Same Margarette RTX3090 24GiB, Qwen Image 2.1 int8, 30 steps, 1024x1024. Before TensorFS766/Runtime75, after TensorFS302/Runtimec498, TensorD766 unchanged. Four distinct seeds per variant.

| Variant | Run | Seed | Execution s | CLI wall s | First CUDA step ms | Mean later step ms | Logical completed reads GB |
|---|---:|---:|---:|---:|---:|---:|---:|
| before cold | 17360 | 2026102511 | 170.863 | 174.265 | 981.392 | 429.137 | 17.134 |
| before warm-1 | 17364 | 2026102512 | 47.157 | unavailable | 1574.979 | 1519.194 | 211.176 |
| before warm-2 | 17370 | 2026102513 | 49.287 | 51.430 | 1518.643 | 1594.193 | 211.176 |
| before warm-3 | 17373 | 2026102514 | 48.800 | 50.474 | 1512.633 | 1579.006 | 211.176 |
| after cold | 17379 | 2026102515 | 29.469 | 57.428 | 2021.564 | 433.078 | 17.134 |
| after warm-1 | 17382 | 2026102516 | 14.110 | 17.244 | 446.076 | 433.014 | 0.000 |
| after warm-2 | 17383 | 2026102517 | 14.781 | 17.102 | 1114.459 | 434.570 | 6.165 |
| after warm-3 | 17384 | 2026102518 | 15.122 | 18.234 | 1391.167 | 432.914 | 6.165 |

Mean warm execution improves 48.415s → 14.671s (3.30×, 69.70% lower). Mean logical completed weight reads fall 211.176GB → 4.110GB (98.05% lower). Same GPU UUID, exact model manifest and package source identity verified.

After first call 17379 exercises text encoder → transformer → VAE and reads each component once. After warm 1 reads 0 bytes. After warm 2/3 each read 28 transformer blocks once (6.165GB), compared with 211.176GB total reads per baseline warm request. This removes within-request denoising churn; it does not eliminate all cross-request movement.

The unchanged opaque-argument graph limitation yields 0 captures/replays in all eight runs. This comparison measures the residency/phase policy improvement, with the existing eager model loop.

## Measurement boundaries

- Logical completed reads are source payload bytes, not physical PCIe or disk traffic; canceled speculation can be omitted.
- store.ns excludes asynchronous prefetch duration; prefetch.wait_ns is CPU settlement time, not exposed GPU starvation.
- First calls differ in source/disk/install cache state and are not pure cold-weight loading measurements.
- Before 17364 CLI wall is unavailable after interrupted controller; null is preserved, not inferred from worker/controller clocks. CLI averages have unequal sample counts and are not the headline comparison.
- Worker clock is offset from controller. Qualification uses worker execution/step/GPU clocks, fresh intervals and distinct requests.
- Seeds differ between variants, so image integrity/dimensions are verified but exact pixel parity is not claimed.
- All model graph counters are 0; speedup is not evidence of CUDA graph acceleration.
- Warm1 zero reads is not stable-state zero traffic: warm 2/3 each reload6.165GB once per request. No within30-step churn occurs.
- Three warm samples per variant are descriptive; source/installed proof is not release/deployment proof.

## Remaining work

- Avoid repeat partial transformer reloads across VAE where actual activation/workspace needs permit. Current traces do not distinguish all eviction causes, so 6.165GB warm 2/3 reads are not proven minimal.
- Eliminate speculative preparation refusals attributable to stale outgoing unpreferred cache without speculative eviction.
- Measure actual H2D bytes and exposed weight-ready stalls before claiming hidden transfer latency.

Raw receipts and per-component/per-block fills, phase records, graph counters, all 30 device-step samples and image hashes remain in `phase-comparison.json`.

## Additional fresh-prompt validation

Run 17385 completes a new prompt in 17.909s reported execution with 30 steps and a decoded 1024² PNG matching its recorded file digest. It records text encoder → transformer → VAE, with each component filled once (17.134GB total logical completed reads). The text-encoder phase returns 13.252GB of unused Torch reservations. No pure-cut retry was recorded. This result is separate correctness coverage and is excluded from the warm A/B averages. Exact image/RGB hashes and phase/fill records are in the JSON appendix.
