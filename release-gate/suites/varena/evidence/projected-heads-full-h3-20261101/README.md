# Full H3 Heads pair: adoption passes, observed no-worse target fails

The new native Heads consumer is active in the real full model, but this single matched
pair is slightly slower in denoising and reads more weight bytes. The result does not
justify a performance promotion. Sampled visual quality appears comparable, with a
different generated composition; full temporal and audio perceptual equivalence is not proven.

## Matched execution

Control **25227** uses Runtime `0.36.21+dev.gab491232`; candidate **25264** uses
`0.36.21+dev.gdae97328`. Both use TensorFS `b20f699c`, worker `afa6f922`, the same
40-file package `3476cb1d`, model manifests, input/seed, and medusa GPU UUID
`9e7b1c5d-0754-e081-0ec2-45c94ce5c59c`. Both actually execute 30 denoising steps,
produce 362 frames at 1344×768/24fps and the same 32kHz stereo audio format, and are not memo hits.
`audit-pair.py` verifies these boundaries and all output hashes against immutable run records.

The selected transformer route remains `kitchen-sol-producer-int4qk-shared-qkv`, with
the same dense/exact/prefix references; audio VAE stays SDPA and video VAE Kitchen INT8.
GLUE identities/substitutions match. The only differing GLUE report field is its warm-up
time (24.3 versus 25.5ms). These are first requests on sequential fresh processes; equivalent
OS/storage cache state is not established.

| Metric | Control | Candidate | Change |
|---|---:|---:|---:|
| Denoise host span | 1,010,301.294 ms | 1,019,990.658 ms | **+0.95906%** |
| Denoise device sum | 1,010,268.710 ms | 1,019,954.451 ms | +0.95873% |
| Whole execution | 1,089,741 ms | 1,056,066 ms | −3.09018% |
| Recorded store bytes | 468,227,295,044 | 479,999,516,996 | **+2.51421%** |
| Recorded kept-host bytes | 0 | 0 | unchanged |
| Request working allocations | 404,621 | 360,877 | −10.81110% |
| Request memory-driver calls | 38,798 | 47,522 | **+22.48570%** |
| Request event waits | 999 | 844 | −15.51552% |

Whole execution includes startup/loading/encoding and must not override the denoise
regression. Store/kept-host counts describe completed weight-source fills, not total
physical PCIe traffic. The full request's driver count cannot assign cost to an individual
Heads kernel. Both arms have zero terminal allocator OOMs/context joins. Mean GPU clocks
are 2,536/2,540MHz; maximum temperatures 71/73°C, with power-cap throttling in both.

All additional source traffic is in the transformer: **29 extra fills**, +11,772,221,952
bytes. Text encoder, audio VAE and video VAE transfers are unchanged. Weight reclaim shifts
from 219.58 to 251.90GB under allocator pressure and 234.11 to 213.60GB under explicit requests;
streaming-limit reclamation remains zero in both.

## Actual native adoption and coupled choices

Candidate executes **480** completed/published native Heads scopes; control has none.

| Heads per native window | Calls | Measured backing per scope |
|---:|---:|---:|
| 56 | 314 | 8,462,008,320 B |
| 28 | 85 | 5,330,960,384 B |
| 14 | 38 | 3,766,484,992 B |
| 7 | 34 | 2,969,567,232 B |
| 4 | 9 | 2,648,702,976 B |

The existing 1500 native Tail scopes remain in both. The 1450 native output projections
also remain, but candidate splits 81 (55 half-row, 26 quarter-row), while control runs all
whole. Tail choices change too: candidate has more 54,562-row calls and also some much
smaller 52/212/426-row calls absent from control. These are real pipeline interactions,
not isolated-head timing measurements.

Legacy `_Heads` calls fall 1500→1020, exactly the 480 migrated calls. Remaining nested
legacy attention calls fall 620→310. The native head geometry entails 1034 executed head
windows; SOL dense-step/path counts rise 622→1346 while logical sparse calls remain 900.
This demonstrates a changed execution geometry, without proving which path caused every
extra mapping or the 9.7-second denoise difference. Detailed histograms, selected scopes,
transfer reasons and phase measurements are retained in the JSON analyses.

## Output evidence

Both video streams have 362 frames, 15.083333-second duration, identical dimensions/rate;
audio duration is 15.075 seconds in both. Entire decoded-video comparison gives PSNR
**15.381197dB** and SSIM **0.687227**. These distances describe different generated pixels,
not a perceptual quality score.

Final normalized video latents have identical metadata/shape `[1,24,107,48,84]`, finite
F16 values, relative L2 **0.7638899**, cosine **0.7055789**, and RMSE **0.8134118**.
10,350,302 of 10,354,176 values differ. This is substantial trajectory/output divergence;
it is not a per-operation error measurement. This format saves no audio latents.

Full decoded PCM comparison covers 966,656 interleaved samples. PCM differs, maximum
absolute int16 delta 466 and comparison SNR 3.6078dB. Both streams are quiet (RMS −57.80 and
−56.39dBFS) with zero clipped samples. Audio was checked numerically; this interface
cannot audition audio, so no listening-based quality verdict is claimed.

I inspected paired actual frames at 0, 2, 4, 6, 8, 10, 12, 14 seconds. Both show a coherent
golden-hour clockmaker workbench, tools, clock/mechanical apparatus, window and bokeh.
Composition/object placement and camera framing differ. No clear candidate-only blackout,
tiling, gross corruption or loss of scene detail is apparent in these samples. Eight frames
per clip do not rule out brief flicker, fine motion defects, or later quality differences.
See `frames/paired-early.png` and `frames/paired-late.png`.

## Limits and disposition

This is one sequential matched sample per arm, with no confidence interval. It qualifies
actual native adoption and completes the stated output scope. It **fails the observed
no-worse denoise/store target** and does not establish full perceptual equivalence.
Keep the bounded operation qualification and this full-model outcome separate. Continue
the declared phase/remaining-consumer work and requalify the final coherent implementation;
do not claim these measurements already demonstrate a speedup.
