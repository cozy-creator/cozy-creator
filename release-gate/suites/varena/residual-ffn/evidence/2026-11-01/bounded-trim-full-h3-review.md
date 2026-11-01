# Bounded working-trim follow-up:25151 /25168

Both full H3 requests pass identical source/input/model/geometry checks. Runtimefe5,
package9e(all39 files), workerafa and the request are unchanged. TensorFS changes
9ccb→b20; root's source-diff receipt discloses additive protocol memo/version changes,
with TensorFS/Python/shim/quantization trees unchanged and only the bounded trim in
Varena's memory path. Both models ran in Legacy service mode.

| Observed boundary | 9ccb baseline25151 | b20 trim25168 | Change |
|---|---:|---:|---:|
| Denoise stage | 970.467785 s | 958.549156 s | -1.2281% |
| Full execution | 1002.785 s | 991.566 s | -1.1188% |
| Recorded store-source bytes | 467,821,356,356 | 468,227,295,044 | +0.08677% |
| Kept-host source bytes | 0 | 0 | unchanged |
| Working driver calls | 45,094 | 35,314 | -21.6880% |
| Driver peak | 25,234,112,512 B | 25,248,792,576 B | +14 MiB |

There is one extra transformer fill,405,938,688 B. Thus the strict store-source
no-worse result remains false; reduced driver churn does not constitute an I/O
improvement. All1500 native Tail observations and1450 native projections publish.
Tail choices remain mostly27280 rows764 times and54562 rows530 times; whole occurs
six times. Actual model kernel recipes and GLUE identity match. The phase-region
planning gap remains; this follow-up does not clear Tail/P2 acceptance.

Full-video SSIM0.649992/PSNR14.939696dB; PCM differs(SNR2.271dB). Public latents
have matching metadata/F16[1,24,107,48,84] geometry and are finite, with relative
L2=0.854354 and cosine=0.634389. Sampled0/2.5/5/7.5/10/15s original frames show a
different workbench layout/pan with comparable fine detail, lighting, bokeh and
coherent sampled continuity. No obvious corruption, blanks or banding. This is
not full-motion playback or human audio listening.

One sample per arm is not a timing distribution. Working counters cover the whole
request; store/host counters are not complete physical PCIe/H2D. OS/storage caches
were not independently reset. Against API-only25112, combined Tail+trim denoise is
0.350% lower but store bytes remain3.966% higher; that is a combined comparison,
not attribution of trim alone. Historical run records are used even though root
installed a subsequent P3 cohort after this model completed.

`pair-review.json` contains exact metrics, choices, source-diff disclosure and raw
hashes. `comparison.json`, `latents-comparison.json`, full-frame logs and paired
image sheets preserve the numerical and sampled visual evidence. Both full arm
receipts remain untouched. No merge/release or overall P2 claim follows.
