# Full Tail pair: mechanism works; no-worse timing/I/O gate FAILS

Control25112 and candidate25151 both completed the full15-second/362-frame/
1344×768/24fps/30-step/audio request. They use identical package9e7490c0 (all39
source files verified), input/model identities, native TensorFS9ccb36a24 and
workerafa6f922. Runtime is API-only control1aca1d6cd versus candidatefe5a783da.
Both use the worker's Legacy contract; this is not a P3 cooperative-owner test.

| Observed boundary | Control | Candidate | Change |
|---|---:|---:|---:|
| Denoise host stage | 961.913113 s | 970.467785 s | +0.8893% |
| Full execution | 994.340 s | 1002.785 s | +0.8493% |
| Recorded store-source bytes | 450,365,992,772 | 467,821,356,356 | +3.8758% |
| Kept-host bytes | 0 | 0 | unchanged |
| Working allocator driver calls | 44,691 | 45,094 | +0.9017% |
| Driver peak | 25,242,501,120 B | 25,234,112,512 B | -8 MiB |
| Rest-loop working backing peak | 17,328,766,976 B | 20,015,218,688 B | +2,686,451,712 B |

All1500 candidate native Tail scopes published completed observations. The control
has zero native Tail scopes and retains1500 legacy Tail calls. Both retain1450
whole-row native projection observations. Candidate Tail row choices are:

| Rows | Calls | Observed backing per Tail |
|---:|---:|---:|
| 109123 | 6 | 13,337,886,720 B |
| 54562 | 543 | 6,687,817,728 B |
| 27280 | 752 | 3,363,831,808 B |
| 6820 | 41 | 872,415,232 B |
| 3410 | 129 | 459,276,288 B |
| 1704 | 29 | 213,909,504 B |

The control mostly uses3307 rows(1166 calls) and1679 rows(332), with two smaller
calibration calls. Every additional store byte is in transformer fills1038→1081.
Requested reclaim229,638,144,000 B and weight-allocation reclaim3,510,632,448 B match;
allocator reclaim grows205,751,582,720→223,652,872,192 B. The source and measurements
are consistent with large working choices reducing retention of subsequent weights.
They do not establish an optimal hardcoded chunk. See
`outputs/xs138-p2-tail/full-h3/audit/choice-policy-gap.md`.

Actual model recipes match: cozy-int8 for text encoder, transformer and VAE;
transformer kitchen-sol-producer-int4qk-shared-qkv, VAE kitchen-int8, audio VAE sdpa.
The sm90-only FlashAttention-3 prebuilt refusal is present and unserved in BOTH
arms. Both apply the same GLUE topology identity and preserve the original compiled
kernel sources. No new selected-kernel fallback accounts for the comparison.

Prefetch attempts/declines are1092/1092 and1136/1136; admissions are zero. Each run
has two published owner samples, with successful_passes1. Source inspection proves
this eager path enters with active keyNone and only supplies its fingerprint on
exit. Native prefetch requires a known active key, so zero admissions are structural,
not simply inferred from the two final keys being different. No prior key was
substituted as a promise about unknown current execution.

Full362-frame comparison gives SSIM0.713864 / PSNR16.069232dB. Decoded PCM differs
(max delta585, quiet referenceRMS68.899, errorRMS64.837, SNR0.528dB). Public saved
latents have matching metadata and F16[1,24,107,48,84] geometry; all10,354,176 values
are finite, relativeL2=0.6913573 and cosine=0.7641461. These are genuine generated
scene differences, not bit-equality passes.

Sampled original frames at0/2.5/5/7.5/10/15s show similarly detailed golden-hour
workbench pans, with coherent object motion across the samples, focus/bokeh and
lighting. Layouts differ; no obvious corruption, blank frame or banding is visible.
This sampled visual review is not full-motion playback or human audio listening.
The user accepts roughly equal quality rather than bit identity, but the timing/
I/O gate above still fails and remains a blocker.

One sample per arm is not a performance distribution. Both reach80C with close mean
SM clocks2474/2480MHz and power-cap shares92.97/92.49%. OS/storage caches were not
independently reset. Source counters are not complete hardware PCIe/H2D, and working
counters cover the whole request rather than separately attributed Tail calls. The
older retention78cf/9b pair is separate evidence and is not mixed into this result.
No merge, release, or all-P2 acceptance follows from this pair.

Files: `pair-review.json` exact deltas, policy and phase facts; `comparison.json`
full-video/PCM results; `latents-comparison.json` full latent and per-time metrics;
`ssim.log`, `psnr.log`, `ffmpeg.log`; original sampled images in `frames/` and the two
`paired-frames-*.webp` sheets. Each arm retains exact run, scope-check and analysis JSON.
