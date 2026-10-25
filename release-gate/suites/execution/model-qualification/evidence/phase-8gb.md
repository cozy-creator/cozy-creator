# Phase policy: real 30-step Sadao comparison

Same Sadao RTX3070 8GiB; before TensorFS766/Runtime75, after TensorFS302/Runtimec498; TensorD766 unchanged. Thirty steps, 1024x1024, one first call plus 3 warm calls each. Fresh after seeds avoid output memoization.

## Results

| Model / variant | Run | Seed | Execution s | CLI wall s | First CUDA step ms | Mean later step ms | Logical completed reads GB |
|---|---:|---:|---:|---:|---:|---:|---:|
| sdxl before cold | 17343 | 730100 | 35.751 | 65.583 | 2670.009 | 556.579 | 22.447 |
| sdxl before warm-1 | 17348 | 730101 | 41.653 | 44.202 | 1371.473 | 1366.892 | 146.566 |
| sdxl before warm-2 | 17351 | 730102 | 43.344 | 45.317 | 1475.182 | 1421.104 | 151.673 |
| sdxl before warm-3 | 17353 | 730103 | 43.343 | 49.044 | 1498.998 | 1420.668 | 151.623 |
| sdxl after cold | 17362 | 730104 | 23.022 | 26.436 | 1779.992 | 469.179 | 5.242 |
| sdxl after warm-1 | 17363 | 730105 | 16.256 | 18.038 | 1850.741 | 470.766 | 5.147 |
| sdxl after warm-2 | 17365 | 730106 | 16.163 | 17.664 | 1783.245 | 470.927 | 5.210 |
| sdxl after warm-3 | 17366 | 730107 | 16.261 | 17.594 | 1842.136 | 471.089 | 5.147 |

sdxl: mean warm execution improves 42.780s → 16.227s (2.64×, 62.07% lower); logical completed reads fall 149.954GB → 5.168GB (96.55% lower). Same GPU UUID and exact model manifests verified.

Each after run reads UNet exactly 5,134,927,368 bytes in 88 fills: one full component load across 30 steps. Before warm requests read 146.448–151.556GB of UNet bytes. Warm phase entry returns 3.412–4.817GB of unused Torch reservations, and no repeated UNet block reloads appear in the trace.

Remaining optimization: after warm requests complete only 3/7/3 speculative fills and each skips 80 for capacity. First denoising steps remain 1.783–1.851s versus~0.471s subsequently. This shows front-loaded preparation cost; it does not isolate the exposed H2D stall.

## Measurement boundaries

- Logical completed weight reads are source payload bytes, not measured PCIe or physical disk traffic; canceled speculative transfers are omitted.
- store.ns measures synchronous host read_into wall time; successful speculative reads are recorded with 0 ns. prefetch.wait_ns measures host settlement/drain/cancel work, not GPU starvation.
- CUDA step elapsed time includes stream dependencies/launch gaps and is not isolated kernel compute. Exposed transfer stalls are unmeasured.
- First-call before/after coldness is not equivalent: before includes initial download/construction/text encoding; after may reuse source and prompt memo caches. Compare warm intervals primarily.
- After seeds differ from before seeds; output integrity and dimensions are checked, but exact image parity is not claimed.
- Three warm samples per variant are descriptive; no statistical generalization across GPUs or workloads.
- Native residency counters are process cumulative; trace and moved bytes are per request.

The attempted same-seed after run 17361 returned a cached output with no execution/measurement fields. Its rejected receipt remains under `after-sdxl/`; it contributes no performance result.

Machine/source/software identities, per-component fill counts, exact step series, graph counters, phase records and all image hashes are retained in `phase-comparison.json`.

## Anima int8 comparison

The after package adds its explicit four-component execution plan. Model content, int8 route, GPU and 30-step 1024² configuration match. Seeds differ.

| Variant | Run | Seed | Execution s | CLI wall s | First CUDA step ms | Mean later step ms | Logical completed reads GB |
|---|---:|---:|---:|---:|---:|---:|---:|
| before cold | 17355 | 730200 | 24.109 | 43.011 | 370.164 | 453.479 | 3.430 |
| before warm-1 | 17356 | 730201 | 19.751 | 21.927 | 606.258 | 623.265 | 64.009 |
| before warm-2 | 17357 | 730202 | 23.911 | 26.095 | 604.431 | 759.077 | 64.593 |
| before warm-3 | 17358 | 730203 | 23.932 | 27.673 | 608.937 | 760.615 | 64.593 |
| after cold | 17368 | 730204 | 22.159 | 39.177 | 863.730 | 453.332 | 3.430 |
| after warm-1 | 17369 | 730205 | 15.198 | 19.181 | 768.963 | 454.212 | 2.609 |
| after warm-2 | 17371 | 730206 | 15.210 | 17.172 | 763.958 | 455.008 | 2.609 |
| after warm-3 | 17372 | 730207 | 15.244 | 17.793 | 772.734 | 455.921 | 2.609 |

Mean warm execution improves 22.531s → 15.217s (1.48×, 32.46% lower); logical completed reads fall 64.399GB → 2.609GB (95.95% lower).

All after warm requests read only 26 transformer blocks once each (1.966GB), versus 28 blocks × 30 steps (63.508GB) before. Every after request records text encoder → text conditioner → transformer → VAE phase transitions; warm entry returns 5.805 GB of Torch cache.

Anima enables classifier-free guidance only for steps 5–21. Its CUDA step series therefore has a deliberately slower middle section; before/after comparisons must match step positions rather than interpret all timing variation as memory stalls.

## Remaining work

- Measure physical H2D traffic, canceled speculative bytes, and exposed weight-ready stalls before claiming transfer latency is hidden.
- Resolve phase-window prefetch capacity refusals without evicting useful outgoing cache solely to satisfy speculative limits.
- SDXL still reads its full UNet once each request. All VAE phase entries reclaimed 0 native bytes, but later VAE pressure includes real pure-cut allocation retries in warm1/3, whose unsized reclaim clears all idle backing. Current receipts cannot separate minimal physically necessary eviction from streaming-limit enforcement or broad fallback reclaim.
- Preserve partial denoiser cache across VAE where actual live activation/workspace needs permit; measure the next-request benefit rather than assume one full reload is optimal.

## Separate fresh-prompt SDXL handoff (c58 followup)

Run 17377, seed 730108, completes a new prompt at 1024² and 30 steps in 25.670s reported execution. The recorded phases are grouped text_encoder/text_encoder_2 → UNet → VAE. Both text towers read their weights once; output integrity/dimensions pass. This is a correctness appendix on the subsequent phase-window change, excluded from the c498 warm A/B averages.

Residual movement remains: UNet reads 15.626GB in 494 fills (3.043× its 5.135GB weight payload). Fourteen blocks (59,60,64–75), comprising 361,749,760 bytes, each reload 30 times; the other 74 blocks fill once. Total logical completed reads are 17.368GB. All three phase-entry native reclaim amounts are 0; no pure-cut retry was recorded. This identifies a remaining residency-policy issue after fresh text encoding and prevents an all-resident or optimal-movement claim.
