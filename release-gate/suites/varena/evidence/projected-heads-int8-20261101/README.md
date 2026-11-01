# Full H3 INT8 Heads pair: observed denoise and source-traffic targets pass

The candidate executes the intended 120 additional native INT8 Heads calls, denoises
3.917% faster and reads 21.554% fewer weight-source bytes in this matched pair. Both
reviewers found comparable visual quality in eight paired samples. The generated scenes
are not identical; continuous-motion and listening-based audio equivalence are unproven.

## Matched execution

Control **25324** uses Runtime `cc49350d9`; candidate **25347** uses `3daaaeda9`.
Both use TensorFS `4dcf26183`, worker `afa6f922`, the same 41-file package `f1e8d2bb`,
model manifests, input and seed 849626408, and hitoshiki GPU UUID
`18d960a8-2283-19c9-c060-ef8a22fabe35`. Both execute all 30 steps without memo reuse and
produce 362 frames at 1344×768/24fps with 32kHz stereo audio. The independent script verifies
source files, input bytes, software, selected kernels, native observations and output hashes.

Both retain the selected transformer route `kitchen-sol-producer-int4qk-shared-qkv`,
the same dense/exact references, SDPA audio VAE and Kitchen INT8 video VAE. The control
retains native INT4 Heads and phase Plans and declines only the new INT8 admission.

| Metric | Control | Candidate | Change |
|---|---:|---:|---:|
| Denoise host span | 981,415.601 ms | 942,973.486 ms | **−3.91701%** |
| Denoise device sum | 981,391.087 ms | 942,949.952 ms | −3.91700% |
| Whole execution | 1,073,070 ms | 973,692 ms | −9.26109% |
| Recorded store bytes | 478,375,762,244 | 375,267,335,492 | **−21.55386%** |
| Recorded kept-host bytes | 0 | 0 | unchanged |
| Request working allocations | 360,956 | 320,536 | −11.19804% |
| Request memory-driver calls | 46,160 | 44,183 | −4.28293% |
| Request event waits | 831 | 498 | −40.07220% |
| Weight fills | 1,218 | 964 | −254 |
| Weight evictions | 1,136 | 884 | −252 |

Whole execution includes startup/loading/encoding; denoising is the performance gate.
All 103,108,426,752 saved source bytes are transformer transfers: 448,965,125,120→
345,856,698,368 bytes. Text/audio/video VAE bytes are unchanged. Source-fill counters do
not measure every physical PCIe transfer. Both requests have zero terminal allocator OOMs
and context joins. Mean SM clocks are 2,574/2,554MHz; maximum temperatures 65/66°C, with
power-cap throttling in both. No equal OS/storage-cache state or statistical confidence
interval is claimed from this one sequential sample per arm.

## Native adoption and remaining policy

Both arms publish 480 native INT4 Heads, 1,500 native Tail and 1,450 native output-projection
observations. Candidate additionally publishes 120 native INT8 Heads. Legacy H3 `_Heads`
calls fall 1,020→900; the remaining 900 are the sparse-producer migration. Nested attention calls under the old H3 Heads split
fall 313→0. Other choices change as memory becomes available:

| Heads per window | Control INT4 | Candidate INT4 | Candidate INT8 |
|---:|---:|---:|---:|
| 56 | 314 | 30 | 60 |
| 28 | 85 | 302 | 35 |
| 14 | 38 | 92 | 22 |
| 7 | 34 | 28 | 1 |
| 4 | 9 | 16 | 1 |
| 2 | 0 | 11 | 0 |
| 1 | 0 | 1 | 1 |

The native region Plan activates generation 1 with 53 transformer regions totaling
21,151,875,072 bytes. Last-recorded per-site choices report `preserves_phase_weights=false`;
the metadata does not prove preservation on every historical call. Prefetch remains 0 in
both arms (1,162/907 native declines), so this is not a successful lookahead-prefetch claim.
The source-traffic improvement is an observed whole-pipeline result, not an attribution to
one kernel or one policy field.

## Actual output evidence

Both videos contain 362 frames, 15.083333 seconds at 1344×768/24fps; both audio streams are
15.075 seconds at 32kHz stereo. Full-video comparison reports PSNR **17.046266dB**, SSIM
**0.750472**. Final normalized video latents share finite F16 metadata/shape
`[1,24,107,48,84]`; relative L2 **0.6332404**, cosine **0.8022459**, RMSE **0.6662421**.
10,348,912 of 10,354,176 values differ. These are substantial generated-trajectory distances,
not perceptual quality scores or per-operation errors. No audio latents are in this format.

Full decoded PCM contains 966,656 interleaved samples; the streams differ, with maximum
int16 delta 317 and comparison SNR 5.39289dB. Control/candidate RMS levels are −55.32/−58.01dBFS;
both are quiet and have zero clipped samples. No listening-based verdict is claimed.

Both `/root/core_restart` and `/root` inspected actual paired frames at 0, 2, 4, 6, 8, 10, 12, 14
seconds. The clips show coherent golden-hour workbenches, mechanical instruments and tools,
with comparable detail, light and sampled camera progression. Composition, object placement
and framing differ. Neither reviewer saw obvious candidate-only blackout, tiling, gross
corruption or collapsed texture. Eight samples cannot establish every intermediate frame,
continuous motion, audio quality, or a general quality distribution across prompts/seeds.
The separate root review is preserved as `root-visual-review.json`.

## Bounded proof and disposition

The same candidate/native cohort separately passes 17 actual INT8 mechanism cases and
same-PID cleanup 25314/25315: 25 numerical comparisons exact, complete allocation/copy
censuses, refusal boundaries, actual exact/prefix-heavy processors, and active Plan generation
with explicit Region reclamation. Projection 25317/25318 and Tail 25320/25321 also pass their
updated generation-forwarding regressions. These are separate functional/allocation proofs;
this full pair supplies real-model adoption, observed performance/traffic and scoped output
quality evidence. The later combined native/runtime cohort is qualified separately.
